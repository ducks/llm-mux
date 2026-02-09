#![allow(dead_code)]

//! Error types for llmux

use std::time::Duration;
use thiserror::Error;

/// Categories of errors that can occur during execution
#[derive(Debug, Clone, Error)]
pub enum ErrorKind {
    // Retryable - transient failures
    #[error("rate limited, retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    #[error("timeout after {elapsed:?}")]
    Timeout { elapsed: Duration },

    #[error("network error: {message}")]
    NetworkError { message: String },

    #[error("backend unavailable: {backend}")]
    BackendUnavailable { backend: String },

    // Retryable with modification
    #[error("failed to parse output: expected {expected}")]
    OutputParseFailed { raw: String, expected: String },

    #[error("verification failed: {command}")]
    VerificationFailed { command: String, stderr: String },

    // Not retryable - permanent failures
    #[error("configuration error: {message}")]
    ConfigError { message: String },

    #[error("file not found: {path}")]
    FileNotFound { path: String },

    #[error("template error in {template}: {error}")]
    TemplateError { template: String, error: String },

    #[error("invalid workflow: {errors:?}")]
    InvalidWorkflow { errors: Vec<String> },

    #[error("authentication error for {backend}")]
    AuthError { backend: String },

    #[error("edit failed: {message}")]
    EditFailed { message: String },
}

impl ErrorKind {
    /// Returns true if this error type is potentially retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ErrorKind::RateLimit { .. }
                | ErrorKind::Timeout { .. }
                | ErrorKind::NetworkError { .. }
                | ErrorKind::BackendUnavailable { .. }
                | ErrorKind::OutputParseFailed { .. }
                | ErrorKind::VerificationFailed { .. }
        )
    }
}

/// Full error context for a step failure
#[derive(Debug, Clone)]
pub struct StepError {
    pub kind: ErrorKind,
    pub step: String,
    pub backend: Option<String>,

    // Timing
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub failed_at: chrono::DateTime<chrono::Utc>,
    pub duration_ms: u64,

    // What we sent
    pub prompt: Option<String>,
    pub command: Option<String>,

    // What we got back
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub http_status: Option<u16>,

    // Retry state
    pub attempt: u32,
    pub max_attempts: u32,
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "step '{}' failed: {}", self.step, self.kind)?;
        if let Some(ref backend) = self.backend {
            write!(f, " (backend: {})", backend)?;
        }
        write!(f, " [attempt {}/{}]", self.attempt, self.max_attempts)?;
        Ok(())
    }
}

impl std::error::Error for StepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.kind)
    }
}

impl StepError {
    pub fn new(kind: ErrorKind, step: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            kind,
            step: step.into(),
            backend: None,
            started_at: now,
            failed_at: now,
            duration_ms: 0,
            prompt: None,
            command: None,
            stdout: None,
            stderr: None,
            exit_code: None,
            http_status: None,
            attempt: 1,
            max_attempts: 1,
        }
    }

    pub fn with_backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = Some(backend.into());
        self
    }

    pub fn with_timing(
        mut self,
        started_at: chrono::DateTime<chrono::Utc>,
        duration_ms: u64,
    ) -> Self {
        self.started_at = started_at;
        self.failed_at = chrono::Utc::now();
        self.duration_ms = duration_ms;
        self
    }

    pub fn with_attempt(mut self, attempt: u32, max_attempts: u32) -> Self {
        self.attempt = attempt;
        self.max_attempts = max_attempts;
        self
    }

    pub fn with_output(mut self, stdout: Option<String>, stderr: Option<String>) -> Self {
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_kind_retryable() {
        assert!(ErrorKind::RateLimit { retry_after: None }.is_retryable());
        assert!(
            ErrorKind::Timeout {
                elapsed: Duration::from_secs(30)
            }
            .is_retryable()
        );
        assert!(
            !ErrorKind::ConfigError {
                message: "bad".into()
            }
            .is_retryable()
        );
        assert!(
            !ErrorKind::AuthError {
                backend: "claude".into()
            }
            .is_retryable()
        );
    }

    #[test]
    fn test_step_error_display() {
        let err = StepError::new(
            ErrorKind::Timeout {
                elapsed: Duration::from_secs(30),
            },
            "analyze",
        )
        .with_backend("codex")
        .with_attempt(2, 3);

        let display = format!("{}", err);
        assert!(display.contains("analyze"));
        assert!(display.contains("timeout"));
        assert!(display.contains("codex"));
        assert!(display.contains("2/3"));
    }
}
