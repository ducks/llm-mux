// TODO: Use RetryExecutor in workflow runner
#![allow(dead_code)]

//! Retry wrapper with exponential backoff

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse, RetryPolicy};
use async_trait::async_trait;
use std::sync::Arc;

/// Wrapper that adds retry logic to any backend executor
pub struct RetryExecutor<T: BackendExecutor> {
    inner: Arc<T>,
    policy: RetryPolicy,
}

impl<T: BackendExecutor> RetryExecutor<T> {
    /// Create a new retry executor
    pub fn new(inner: T, policy: RetryPolicy) -> Self {
        Self {
            inner: Arc::new(inner),
            policy,
        }
    }

    /// Create with default retry policy
    pub fn with_defaults(inner: T) -> Self {
        Self::new(inner, RetryPolicy::default())
    }
}

#[async_trait]
impl<T: BackendExecutor + 'static> BackendExecutor for RetryExecutor<T> {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        let mut last_error = None;

        for attempt in 0..=self.policy.max_retries {
            match self.inner.execute(request).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    // Check if error is retryable
                    if !e.is_retryable() || attempt == self.policy.max_retries {
                        return Err(e);
                    }

                    // Calculate delay
                    let delay = if let Some(retry_after) = e.retry_after() {
                        // Use server-specified retry-after if available
                        retry_after
                    } else {
                        self.policy.delay_for_attempt(attempt)
                    };

                    last_error = Some(e);

                    // Wait before retrying
                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Should never reach here, but just in case
        Err(last_error.unwrap_or_else(|| BackendError::Network {
            message: "unknown error after retries".into(),
        }))
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn is_available(&self) -> bool {
        self.inner.is_available().await
    }
}

/// Create a retry executor with custom policy
pub fn with_retry<T: BackendExecutor + 'static>(
    backend: T,
    policy: RetryPolicy,
) -> RetryExecutor<T> {
    RetryExecutor::new(backend, policy)
}

/// Create a retry executor with default policy
pub fn with_default_retry<T: BackendExecutor + 'static>(backend: T) -> RetryExecutor<T> {
    RetryExecutor::with_defaults(backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    /// Mock backend that fails a specified number of times before succeeding
    struct MockBackend {
        name: String,
        fail_count: AtomicU32,
        fail_times: u32,
        error: BackendError,
    }

    impl MockBackend {
        fn new(fail_times: u32, error: BackendError) -> Self {
            Self {
                name: "mock".into(),
                fail_count: AtomicU32::new(0),
                fail_times,
                error,
            }
        }

        fn retryable(fail_times: u32) -> Self {
            Self::new(fail_times, BackendError::rate_limit(None))
        }

        fn non_retryable(fail_times: u32) -> Self {
            Self::new(fail_times, BackendError::auth("invalid token"))
        }
    }

    #[async_trait]
    impl BackendExecutor for MockBackend {
        async fn execute(
            &self,
            _request: &BackendRequest,
        ) -> Result<BackendResponse, BackendError> {
            let count = self.fail_count.fetch_add(1, Ordering::SeqCst);
            if count < self.fail_times {
                Err(self.error.clone())
            } else {
                Ok(BackendResponse::new(
                    "success".into(),
                    self.name.clone(),
                    Duration::from_millis(100),
                ))
            }
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let backend = MockBackend::retryable(2); // Fail twice, succeed on third
        let policy = RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(1), // Fast for tests
            jitter: false,
            ..Default::default()
        };
        let executor = RetryExecutor::new(backend, policy);

        let result = executor.execute(&BackendRequest::new("test")).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let backend = MockBackend::retryable(10); // Always fail
        let policy = RetryPolicy {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            jitter: false,
            ..Default::default()
        };
        let executor = RetryExecutor::new(backend, policy);

        let result = executor.execute(&BackendRequest::new("test")).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BackendError::RateLimit { .. }
        ));
    }

    #[tokio::test]
    async fn test_no_retry_on_non_retryable() {
        let backend = MockBackend::non_retryable(10);
        let policy = RetryPolicy {
            max_retries: 5,
            initial_delay: Duration::from_millis(1),
            jitter: false,
            ..Default::default()
        };
        let executor = RetryExecutor::new(backend, policy);

        let result = executor.execute(&BackendRequest::new("test")).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BackendError::Auth { .. }));
    }

    #[tokio::test]
    async fn test_immediate_success() {
        let backend = MockBackend::retryable(0); // Never fail
        let executor = RetryExecutor::with_defaults(backend);

        let result = executor.execute(&BackendRequest::new("test")).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_helper_functions() {
        let backend = MockBackend::retryable(0);
        let _retry = with_retry(backend, RetryPolicy::default());

        let backend = MockBackend::retryable(0);
        let _retry = with_default_retry(backend);
    }
}
