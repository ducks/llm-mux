//! Retry wrapper with exponential backoff

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse, RetryPolicy};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::time::Instant;

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
    #[allow(dead_code)]
    pub fn with_defaults(inner: T) -> Self {
        Self::new(inner, RetryPolicy::default())
    }
}

#[async_trait]
impl<T: BackendExecutor + 'static> BackendExecutor for RetryExecutor<T> {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        let mut last_error = None;
        let total_timeout = request.timeout.unwrap_or(self.policy.total_timeout);
        let started = Instant::now();
        let deadline = started + total_timeout;

        for attempt in 0..=self.policy.max_retries {
            if request
                .cancellation
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                return Err(BackendError::Cancelled);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(BackendError::timeout(started.elapsed(), None));
            }
            let mut attempt_request = request.clone();
            attempt_request.timeout = Some(remaining);
            let execution = async {
                if let Some(cancellation) = attempt_request.cancellation.as_ref() {
                    tokio::select! {
                        _ = cancellation.cancelled() => Err(BackendError::Cancelled),
                        result = self.inner.execute(&attempt_request) => result,
                    }
                } else {
                    self.inner.execute(&attempt_request).await
                }
            };
            let result = match tokio::time::timeout_at(deadline, execution).await {
                Ok(result) => result,
                Err(_) => Err(BackendError::timeout(started.elapsed(), None)),
            };

            match result {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if !self.policy.allows_retry(&e) || attempt == self.policy.max_retries {
                        return Err(e);
                    }

                    let delay = if let Some(retry_after) = e.retry_after() {
                        retry_after.min(self.policy.max_delay)
                    } else {
                        self.policy.delay_for_attempt(attempt)
                    };

                    last_error = Some(e);

                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if delay >= remaining {
                        return Err(BackendError::timeout(started.elapsed(), None));
                    }
                    if let Some(cancellation) = request.cancellation.as_ref() {
                        tokio::select! {
                            _ = cancellation.cancelled() => return Err(BackendError::Cancelled),
                            _ = tokio::time::sleep(delay) => {}
                        }
                    } else {
                        tokio::time::sleep(delay).await;
                    }
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
#[allow(dead_code)]
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
        fail_count: Arc<AtomicU32>,
        fail_times: u32,
        error: BackendError,
    }

    impl MockBackend {
        fn new(fail_times: u32, error: BackendError) -> Self {
            Self {
                name: "mock".into(),
                fail_count: Arc::new(AtomicU32::new(0)),
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
    async fn test_timeout_and_rate_limit_flags_control_retries() {
        let timeout_backend =
            MockBackend::new(1, BackendError::timeout(Duration::from_millis(1), None));
        let timeout_attempts = timeout_backend.fail_count.clone();
        let timeout_executor = RetryExecutor::new(
            timeout_backend,
            RetryPolicy {
                max_retries: 3,
                initial_delay: Duration::from_millis(1),
                jitter: false,
                retry_timeout: false,
                ..Default::default()
            },
        );
        assert!(
            timeout_executor
                .execute(&BackendRequest::new("test"))
                .await
                .is_err()
        );
        assert_eq!(timeout_attempts.load(Ordering::SeqCst), 1);

        let rate_backend = MockBackend::retryable(1);
        let rate_attempts = rate_backend.fail_count.clone();
        let rate_executor = RetryExecutor::new(
            rate_backend,
            RetryPolicy {
                max_retries: 3,
                initial_delay: Duration::from_millis(1),
                jitter: false,
                retry_rate_limit: false,
                ..Default::default()
            },
        );
        assert!(
            rate_executor
                .execute(&BackendRequest::new("test"))
                .await
                .is_err()
        );
        assert_eq!(rate_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_backoff_observes_cancellation() {
        let backend = MockBackend::new(
            u32::MAX,
            BackendError::rate_limit(Some(Duration::from_secs(5))),
        );
        let executor = RetryExecutor::new(
            backend,
            RetryPolicy {
                total_timeout: Duration::from_secs(10),
                max_delay: Duration::from_secs(5),
                ..Default::default()
            },
        );
        let cancellation = tokio_util::sync::CancellationToken::new();
        let request = BackendRequest::new("test").with_cancellation(cancellation.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancellation.cancel();
        });

        let result = tokio::time::timeout(Duration::from_millis(200), executor.execute(&request))
            .await
            .expect("cancellation should interrupt retry backoff");
        assert!(matches!(result, Err(BackendError::Cancelled)));
    }

    #[tokio::test]
    async fn test_retry_budget_caps_all_attempts_and_backoff() {
        let backend = MockBackend::new(u32::MAX, BackendError::network("reset"));
        let executor = RetryExecutor::new(
            backend,
            RetryPolicy {
                max_retries: 10,
                initial_delay: Duration::from_millis(50),
                total_timeout: Duration::from_millis(10),
                jitter: false,
                ..Default::default()
            },
        );

        let started = Instant::now();
        let result = executor.execute(&BackendRequest::new("test")).await;
        assert!(matches!(result, Err(BackendError::Timeout { .. })));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn test_retry_after_is_capped() {
        let backend = MockBackend::new(1, BackendError::rate_limit(Some(Duration::from_secs(60))));
        let executor = RetryExecutor::new(
            backend,
            RetryPolicy {
                max_delay: Duration::from_millis(5),
                total_timeout: Duration::from_secs(1),
                jitter: false,
                ..Default::default()
            },
        );

        let started = Instant::now();
        assert!(executor.execute(&BackendRequest::new("test")).await.is_ok());
        assert!(started.elapsed() < Duration::from_millis(100));
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
