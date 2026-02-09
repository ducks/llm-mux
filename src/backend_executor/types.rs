#![allow(dead_code)]

//! Core types and traits for backend execution

use crate::config::BackendConfig;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Error types that can occur during backend execution
#[derive(Debug, Clone, Error)]
pub enum BackendError {
    /// Request timed out
    #[error("timeout after {elapsed:?}")]
    Timeout {
        elapsed: Duration,
        partial_output: Option<String>,
    },

    /// Rate limited by the provider
    #[error("rate limited, retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    /// Authentication failed
    #[error("authentication failed: {message}")]
    Auth { message: String },

    /// Network error
    #[error("network error: {message}")]
    Network { message: String },

    /// Failed to parse response
    #[error("parse error: {message}")]
    Parse { message: String },

    /// Command execution failed
    #[error("execution failed (exit code {exit_code:?}): {stderr}")]
    ExecutionFailed {
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },

    /// Backend unavailable
    #[error("backend unavailable: {message}")]
    Unavailable { message: String },

    /// Invalid configuration
    #[error("invalid configuration: {message}")]
    Config { message: String },
}

impl BackendError {
    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            BackendError::Timeout { .. }
                | BackendError::RateLimit { .. }
                | BackendError::Network { .. }
        )
    }

    /// Get suggested retry delay for rate limit errors
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            BackendError::RateLimit { retry_after } => *retry_after,
            _ => None,
        }
    }

    /// Create a timeout error
    pub fn timeout(elapsed: Duration, partial: Option<String>) -> Self {
        Self::Timeout {
            elapsed,
            partial_output: partial,
        }
    }

    /// Create a rate limit error
    pub fn rate_limit(retry_after: Option<Duration>) -> Self {
        Self::RateLimit { retry_after }
    }

    /// Create an auth error
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth {
            message: message.into(),
        }
    }

    /// Create a network error
    pub fn network(message: impl Into<String>) -> Self {
        Self::Network {
            message: message.into(),
        }
    }

    /// Create a parse error
    pub fn parse(message: impl Into<String>) -> Self {
        Self::Parse {
            message: message.into(),
        }
    }

    /// Create an execution failed error
    pub fn execution_failed(exit_code: Option<i32>, stdout: String, stderr: String) -> Self {
        Self::ExecutionFailed {
            exit_code,
            stdout,
            stderr,
        }
    }
}

/// Response from a backend execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendResponse {
    /// Raw text output from the backend
    pub text: String,

    /// Parsed structured output (if JSON was extracted)
    pub structured: Option<serde_json::Value>,

    /// Backend name that produced this response
    pub backend: String,

    /// Model used (if known)
    pub model: Option<String>,

    /// Time taken to execute
    pub duration: Duration,

    /// Token usage (if available)
    pub usage: Option<TokenUsage>,
}

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

impl BackendResponse {
    /// Create a new response with just text
    pub fn new(text: String, backend: String, duration: Duration) -> Self {
        Self {
            text,
            structured: None,
            backend,
            model: None,
            duration,
            usage: None,
        }
    }

    /// Add structured data to the response
    pub fn with_structured(mut self, structured: serde_json::Value) -> Self {
        self.structured = Some(structured);
        self
    }

    /// Add model info to the response
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Add usage info to the response
    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = Some(usage);
        self
    }
}

/// Request to execute against a backend
#[derive(Debug, Clone)]
pub struct BackendRequest {
    /// The prompt to send
    pub prompt: String,

    /// Context files to include (backend-specific handling)
    pub context_files: Vec<PathBuf>,

    /// Working directory for the request
    pub working_dir: Option<PathBuf>,

    /// Override timeout for this request
    pub timeout: Option<Duration>,

    /// System prompt (if supported)
    pub system_prompt: Option<String>,
}

impl BackendRequest {
    /// Create a simple request with just a prompt
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            context_files: Vec::new(),
            working_dir: None,
            timeout: None,
            system_prompt: None,
        }
    }

    /// Add context files
    pub fn with_context(mut self, files: Vec<PathBuf>) -> Self {
        self.context_files = files;
        self
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = Some(dir);
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
}

/// Trait for backend executors
#[async_trait]
pub trait BackendExecutor: Send + Sync {
    /// Execute a request against this backend
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError>;

    /// Get the backend name
    fn name(&self) -> &str;

    /// Check if this backend is available
    async fn is_available(&self) -> bool {
        true
    }
}

/// Implement BackendExecutor for Box<dyn BackendExecutor>
#[async_trait]
impl BackendExecutor for Box<dyn BackendExecutor> {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        (**self).execute(request).await
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    async fn is_available(&self) -> bool {
        (**self).is_available().await
    }
}

/// Retry policy configuration
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retries
    pub max_retries: u32,

    /// Initial delay between retries
    pub initial_delay: Duration,

    /// Maximum delay between retries
    pub max_delay: Duration,

    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,

    /// Whether to add jitter to delays
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Create a policy from backend config
    pub fn from_config(config: &BackendConfig) -> Self {
        Self {
            max_retries: config.max_retries,
            initial_delay: Duration::from_millis(config.retry_delay),
            ..Default::default()
        }
    }

    /// Calculate delay for a given attempt number
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_delay =
            self.initial_delay.as_secs_f64() * self.backoff_multiplier.powi(attempt as i32);
        let capped_delay = base_delay.min(self.max_delay.as_secs_f64());

        let final_delay = if self.jitter {
            // Add up to 25% jitter
            let jitter = rand::random::<f64>() * 0.25 * capped_delay;
            capped_delay + jitter
        } else {
            capped_delay
        };

        Duration::from_secs_f64(final_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_error_retryable() {
        assert!(BackendError::timeout(Duration::from_secs(30), None).is_retryable());
        assert!(BackendError::rate_limit(None).is_retryable());
        assert!(BackendError::network("connection reset").is_retryable());

        assert!(!BackendError::auth("invalid token").is_retryable());
        assert!(!BackendError::parse("invalid json").is_retryable());
        assert!(!BackendError::execution_failed(Some(1), "".into(), "error".into()).is_retryable());
    }

    #[test]
    fn test_backend_response_builder() {
        let response = BackendResponse::new(
            "Hello, world!".into(),
            "claude".into(),
            Duration::from_secs(1),
        )
        .with_model("claude-3")
        .with_structured(serde_json::json!({"key": "value"}));

        assert_eq!(response.text, "Hello, world!");
        assert_eq!(response.backend, "claude");
        assert_eq!(response.model, Some("claude-3".into()));
        assert!(response.structured.is_some());
    }

    #[test]
    fn test_backend_request_builder() {
        let request = BackendRequest::new("Fix this bug")
            .with_context(vec![PathBuf::from("src/main.rs")])
            .with_timeout(Duration::from_secs(60))
            .with_system_prompt("You are a helpful assistant");

        assert_eq!(request.prompt, "Fix this bug");
        assert_eq!(request.context_files.len(), 1);
        assert!(request.timeout.is_some());
        assert!(request.system_prompt.is_some());
    }

    #[test]
    fn test_retry_policy_delays() {
        let policy = RetryPolicy {
            initial_delay: Duration::from_secs(1),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(30),
            jitter: false,
            ..Default::default()
        };

        // Without jitter, delays should be deterministic
        assert_eq!(policy.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(policy.delay_for_attempt(3), Duration::from_secs(8));
        // Should cap at max_delay
        assert_eq!(policy.delay_for_attempt(10), Duration::from_secs(30));
    }

    #[test]
    fn test_retry_policy_with_jitter() {
        let policy = RetryPolicy {
            initial_delay: Duration::from_secs(1),
            jitter: true,
            ..Default::default()
        };

        // With jitter, delay should be >= base delay
        let delay = policy.delay_for_attempt(0);
        assert!(delay >= Duration::from_secs(1));
        assert!(delay <= Duration::from_millis(1250)); // 1s + 25% jitter
    }
}
