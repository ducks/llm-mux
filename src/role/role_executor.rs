#![allow(dead_code)]

//! Execute roles across backends with different execution modes

use crate::backend_executor::{BackendExecutor, BackendRequest, create_executor};
use crate::config::{LlmuxConfig, RoleExecution, StepResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

use super::role_resolver::{ResolvedRole, RoleError};

/// Errors during role execution
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("role resolution failed: {0}")]
    RoleError(#[from] RoleError),

    #[error("all backends failed")]
    AllFailed { errors: HashMap<String, String> },

    #[error("insufficient successes: got {got}, needed {needed}")]
    InsufficientSuccesses {
        got: u32,
        needed: u32,
        outputs: HashMap<String, String>,
        errors: HashMap<String, String>,
    },

    #[error("backend '{backend}' error: {message}")]
    BackendError { backend: String, message: String },
}

/// Result of executing a role
#[derive(Debug, Clone)]
pub struct RoleResult {
    /// Combined output (for First/Fallback mode)
    pub output: Option<String>,

    /// Per-backend outputs (for Parallel mode)
    pub outputs: HashMap<String, String>,

    /// Backends that succeeded
    pub succeeded: Vec<String>,

    /// Backends that failed with error messages
    pub failed: HashMap<String, String>,

    /// Total execution time
    pub duration: Duration,

    /// Execution mode used
    pub execution_mode: RoleExecution,
}

impl RoleResult {
    /// Convert to a StepResult for workflow engine
    pub fn to_step_result(&self) -> StepResult {
        StepResult {
            output: self.output.clone(),
            outputs: self.outputs.clone(),
            failed: self.succeeded.is_empty(),
            error: if self.succeeded.is_empty() {
                Some(format!(
                    "all backends failed: {:?}",
                    self.failed.keys().collect::<Vec<_>>()
                ))
            } else {
                None
            },
            duration_ms: self.duration.as_millis() as u64,
            backend: self.succeeded.first().cloned(),
            backends: self.succeeded.clone(),
        }
    }
}

/// Execute roles across backends
pub struct RoleExecutor {
    config: Arc<LlmuxConfig>,
}

impl RoleExecutor {
    /// Create a new role executor
    pub fn new(config: Arc<LlmuxConfig>) -> Self {
        Self { config }
    }

    /// Execute a resolved role with a prompt
    pub async fn execute(
        &self,
        role: &ResolvedRole,
        request: &BackendRequest,
    ) -> Result<RoleResult, ExecutionError> {
        match role.execution {
            RoleExecution::First => self.execute_first(role, request).await,
            RoleExecution::Parallel => self.execute_parallel(role, request).await,
            RoleExecution::Fallback => self.execute_fallback(role, request).await,
        }
    }

    /// Execute with First mode: use first available backend
    async fn execute_first(
        &self,
        role: &ResolvedRole,
        request: &BackendRequest,
    ) -> Result<RoleResult, ExecutionError> {
        let start = Instant::now();
        let mut failed = HashMap::new();

        for backend_name in &role.backends {
            if let Some(backend_config) = self.config.backends.get(backend_name) {
                if !backend_config.enabled {
                    continue;
                }

                let executor = create_executor(backend_name, backend_config);

                match executor.execute(request).await {
                    Ok(response) => {
                        return Ok(RoleResult {
                            output: Some(response.text),
                            outputs: HashMap::new(),
                            succeeded: vec![backend_name.clone()],
                            failed,
                            duration: start.elapsed(),
                            execution_mode: RoleExecution::First,
                        });
                    }
                    Err(e) => {
                        failed.insert(backend_name.clone(), e.to_string());
                    }
                }
            }
        }

        eprintln!("[DEBUG role_executor] AllFailed errors: {:?}", failed);
        Err(ExecutionError::AllFailed { errors: failed })
    }

    /// Execute with Fallback mode: try each backend until success
    async fn execute_fallback(
        &self,
        role: &ResolvedRole,
        request: &BackendRequest,
    ) -> Result<RoleResult, ExecutionError> {
        // Fallback is the same as First, but semantically different
        // (First = take first available, Fallback = try until success)
        self.execute_first(role, request).await.map(|mut r| {
            r.execution_mode = RoleExecution::Fallback;
            r
        })
    }

    /// Execute with Parallel mode: run all backends concurrently
    async fn execute_parallel(
        &self,
        role: &ResolvedRole,
        request: &BackendRequest,
    ) -> Result<RoleResult, ExecutionError> {
        let start = Instant::now();

        // Create futures for all backends
        let mut handles = Vec::new();

        for backend_name in &role.backends {
            if let Some(backend_config) = self.config.backends.get(backend_name) {
                if !backend_config.enabled {
                    continue;
                }

                let executor = create_executor(backend_name, backend_config);
                let request = request.clone();
                let name = backend_name.clone();

                handles.push(tokio::spawn(async move {
                    let result = executor.execute(&request).await;
                    (name, result)
                }));
            }
        }

        // Wait for all to complete
        let mut outputs = HashMap::new();
        let mut succeeded = Vec::new();
        let mut failed = HashMap::new();

        for handle in handles {
            match handle.await {
                Ok((name, Ok(response))) => {
                    outputs.insert(name.clone(), response.text);
                    succeeded.push(name);
                }
                Ok((name, Err(e))) => {
                    failed.insert(name, e.to_string());
                }
                Err(e) => {
                    // Task panicked or was cancelled
                    failed.insert("unknown".into(), e.to_string());
                }
            }
        }

        let success_count = succeeded.len() as u32;

        if success_count < role.min_success {
            return Err(ExecutionError::InsufficientSuccesses {
                got: success_count,
                needed: role.min_success,
                outputs,
                errors: failed,
            });
        }

        // Combine outputs for the main output field
        let combined_output = if !outputs.is_empty() {
            Some(
                outputs
                    .iter()
                    .map(|(k, v)| format!("=== {} ===\n{}", k, v))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        } else {
            None
        };

        Ok(RoleResult {
            output: combined_output,
            outputs,
            succeeded,
            failed,
            duration: start.elapsed(),
            execution_mode: RoleExecution::Parallel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, RoleConfig};

    fn create_test_config() -> LlmuxConfig {
        let mut config = LlmuxConfig::default();

        // Add a simple echo backend for testing
        config.backends.insert(
            "echo".into(),
            BackendConfig {
                command: "echo".into(),
                enabled: true,
                ..Default::default()
            },
        );

        config.backends.insert(
            "echo2".into(),
            BackendConfig {
                command: "echo".into(),
                enabled: true,
                ..Default::default()
            },
        );

        config.backends.insert(
            "disabled".into(),
            BackendConfig {
                command: "echo".into(),
                enabled: false,
                ..Default::default()
            },
        );

        config.roles.insert(
            "test".into(),
            RoleConfig {
                backends: vec!["echo".into()],
                execution: RoleExecution::First,
                ..Default::default()
            },
        );

        config
    }

    #[tokio::test]
    async fn test_execute_first_mode() {
        let config = Arc::new(create_test_config());
        let executor = RoleExecutor::new(config);

        let role = ResolvedRole {
            name: "test".into(),
            backends: vec!["echo".into()],
            execution: RoleExecution::First,
            min_success: 1,
        };

        let request = BackendRequest::new("hello");
        let result = executor.execute(&role, &request).await.unwrap();

        assert!(result.output.is_some());
        assert!(result.output.unwrap().contains("hello"));
        assert_eq!(result.succeeded, vec!["echo"]);
        assert!(result.failed.is_empty());
    }

    #[tokio::test]
    async fn test_execute_parallel_mode() {
        let config = Arc::new(create_test_config());
        let executor = RoleExecutor::new(config);

        let role = ResolvedRole {
            name: "test".into(),
            backends: vec!["echo".into(), "echo2".into()],
            execution: RoleExecution::Parallel,
            min_success: 1,
        };

        let request = BackendRequest::new("parallel test");
        let result = executor.execute(&role, &request).await.unwrap();

        assert!(result.output.is_some());
        assert_eq!(result.outputs.len(), 2);
        assert!(result.outputs.contains_key("echo"));
        assert!(result.outputs.contains_key("echo2"));
        assert_eq!(result.succeeded.len(), 2);
    }

    #[tokio::test]
    async fn test_disabled_backend_skipped() {
        let config = Arc::new(create_test_config());
        let executor = RoleExecutor::new(config);

        let role = ResolvedRole {
            name: "test".into(),
            backends: vec!["disabled".into(), "echo".into()],
            execution: RoleExecution::First,
            min_success: 1,
        };

        let request = BackendRequest::new("test");
        let result = executor.execute(&role, &request).await.unwrap();

        // Should skip disabled and use echo
        assert_eq!(result.succeeded, vec!["echo"]);
    }

    #[tokio::test]
    async fn test_all_failed() {
        let config = Arc::new(create_test_config());
        let executor = RoleExecutor::new(config);

        let role = ResolvedRole {
            name: "test".into(),
            backends: vec!["nonexistent".into()],
            execution: RoleExecution::First,
            min_success: 1,
        };

        let request = BackendRequest::new("test");
        let result = executor.execute(&role, &request).await;

        assert!(matches!(result, Err(ExecutionError::AllFailed { .. })));
    }

    #[test]
    fn test_role_result_to_step_result() {
        let role_result = RoleResult {
            output: Some("test output".into()),
            outputs: HashMap::new(),
            succeeded: vec!["claude".into()],
            failed: HashMap::new(),
            duration: Duration::from_secs(1),
            execution_mode: RoleExecution::First,
        };

        let step_result = role_result.to_step_result();

        assert_eq!(step_result.output, Some("test output".into()));
        assert!(!step_result.failed);
        assert_eq!(step_result.backend, Some("claude".into()));
    }
}
