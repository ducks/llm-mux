//! Backend execution module
//!
//! Provides executors for CLI and HTTP-based LLM backends with retry logic
//! and output parsing.
//!
//! # Example
//!
//! ```ignore
//! use llmux::backend_executor::{CliBackend, BackendExecutor, BackendRequest, with_default_retry};
//!
//! // Create a CLI backend
//! let backend = CliBackend::new("claude", "claude");
//!
//! // Wrap with retry logic
//! let backend = with_default_retry(backend);
//!
//! // Execute a request
//! let request = BackendRequest::new("Fix this bug in main.rs");
//! let response = backend.execute(&request).await?;
//!
//! println!("Output: {}", response.text);
//! ```

mod claude_backend;
mod cli_backend;
mod http_backend;
pub mod output_parser;
mod retry;
mod types;

pub use claude_backend::ClaudeBackend;
pub use cli_backend::CliBackend;
pub use http_backend::HttpBackend;
pub use retry::{RetryExecutor, with_retry};
#[allow(unused_imports)]
pub use types::{BackendError, BackendExecutor, BackendRequest, BackendResponse, RetryPolicy};

use crate::config::BackendConfig;

/// Create an appropriate executor for a backend config
pub fn create_executor(name: &str, config: &BackendConfig) -> Box<dyn BackendExecutor> {
    if config.is_claude_api() {
        match ClaudeBackend::from_config(name, config) {
            Ok(backend) => Box::new(backend),
            Err(e) => {
                eprintln!(
                    "[WARN] Failed to create Claude API backend: {}, falling back to CLI",
                    e
                );
                Box::new(CliBackend::from_config(name, config))
            }
        }
    } else if config.is_http() {
        Box::new(HttpBackend::from_config(name, config))
    } else {
        Box::new(CliBackend::from_config(name, config))
    }
}

/// Create an executor with retry logic
#[allow(dead_code)]
pub fn create_executor_with_retry(
    name: &str,
    config: &BackendConfig,
) -> RetryExecutor<Box<dyn BackendExecutor>> {
    let executor = create_executor(name, config);
    let policy = RetryPolicy::from_config(config);
    with_retry(executor, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_create_cli_executor() {
        let config = BackendConfig {
            command: "claude".into(),
            ..Default::default()
        };

        let executor = create_executor("claude", &config);
        assert_eq!(executor.name(), "claude");
    }

    #[test]
    fn test_create_http_executor() {
        let config = BackendConfig {
            command: "https://api.openai.com/v1".into(),
            ..Default::default()
        };

        let executor = create_executor("openai", &config);
        assert_eq!(executor.name(), "openai");
    }

    #[tokio::test]
    async fn test_backend_error_display() {
        let err = BackendError::timeout(Duration::from_secs(30), Some("partial".into()));
        assert!(err.to_string().contains("timeout"));

        let err = BackendError::rate_limit(Some(Duration::from_secs(60)));
        assert!(err.to_string().contains("rate limit"));

        let err = BackendError::execution_failed(Some(1), "".into(), "error".into());
        assert!(err.to_string().contains("exit code"));
    }
}
