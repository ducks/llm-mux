#![allow(dead_code, unused_variables)]

//! Step execution logic

use crate::apply_and_verify::RollbackStrategy;
use crate::apply_and_verify::{ApplyVerifyConfig, ApplyVerifyError, apply_and_verify, apply_only};
use crate::backend_executor::BackendRequest;
use crate::config::{LlmuxConfig, StepConfig, StepResult, StepType};
use crate::role::{RoleExecutor, resolve_role};
use crate::template::{TemplateContext, TemplateEngine, evaluate_condition};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// Errors during step execution
#[derive(Debug, Error)]
pub enum StepExecutionError {
    #[error("template error: {0}")]
    Template(#[from] crate::template::TemplateError),

    #[error("role error: {0}")]
    Role(#[from] crate::role::RoleError),

    #[error("execution error: {0}")]
    Execution(#[from] crate::role::ExecutionError),

    #[error("shell command failed: {message}")]
    ShellFailed {
        message: String,
        exit_code: Option<i32>,
    },

    #[error("step type not implemented: {step_type}")]
    NotImplemented { step_type: String },

    #[error("missing required field '{field}' for step '{step}'")]
    MissingField { step: String, field: String },

    #[error("source step '{source_step}' not found for apply step '{step}'")]
    SourceNotFound { step: String, source_step: String },

    #[error("apply-verify failed: {0}")]
    ApplyVerify(#[from] ApplyVerifyError),
}

/// Context for step execution
pub struct ExecutionContext {
    pub config: Arc<LlmuxConfig>,
    pub template_engine: TemplateEngine,
    pub role_executor: RoleExecutor,
}

impl ExecutionContext {
    pub fn new(config: Arc<LlmuxConfig>) -> Self {
        Self {
            role_executor: RoleExecutor::new(config.clone()),
            config,
            template_engine: TemplateEngine::new(),
        }
    }
}

/// Attempt to capture the exit code from a child process.
/// Tries non-blocking first, falls back to blocking wait if process hasn't exited.
async fn capture_exit_code(child: &mut Child) -> Option<i32> {
    match child.try_wait() {
        Ok(Some(status)) => status.code(),
        Ok(None) => child.wait().await.ok().and_then(|status| status.code()),
        Err(_) => None,
    }
}

/// Execute a single step
pub async fn execute_step(
    step: &StepConfig,
    ctx: &ExecutionContext,
    template_ctx: &TemplateContext,
    team: Option<&str>,
    working_dir: &std::path::Path,
) -> Result<StepResult, StepExecutionError> {
    let start = Instant::now();

    // Check condition
    if let Some(ref condition) = step.condition {
        let should_run = evaluate_condition(condition, template_ctx)?;
        if !should_run {
            return Ok(StepResult {
                output: None,
                outputs: std::collections::HashMap::new(),
                failed: false,
                error: Some("skipped: condition evaluated to false".into()),
                duration_ms: start.elapsed().as_millis() as u64,
                backend: None,
                backends: Vec::new(),
            });
        }
    }

    match step.step_type {
        StepType::Shell => execute_shell_step(step, ctx, template_ctx, working_dir).await,
        StepType::Query => execute_query_step(step, ctx, template_ctx, team).await,
        StepType::Apply => execute_apply_step(step, ctx, template_ctx, working_dir).await,
        StepType::Input => {
            // Input steps require user interaction
            Ok(StepResult {
                output: Some("input step not yet implemented".into()),
                outputs: std::collections::HashMap::new(),
                failed: false,
                error: None,
                duration_ms: start.elapsed().as_millis() as u64,
                backend: None,
                backends: Vec::new(),
            })
        }
    }
}

/// Execute a shell step
async fn execute_shell_step(
    step: &StepConfig,
    ctx: &ExecutionContext,
    template_ctx: &TemplateContext,
    working_dir: &std::path::Path,
) -> Result<StepResult, StepExecutionError> {
    let start = Instant::now();

    let command = step
        .run
        .as_ref()
        .ok_or_else(|| StepExecutionError::MissingField {
            step: step.name.clone(),
            field: "run".into(),
        })?;

    // Render template variables in command
    let rendered_command = ctx.template_engine.render(command, template_ctx)?;

    // Execute command
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&rendered_command)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| StepExecutionError::ShellFailed {
            message: format!("failed to spawn: {}", e),
            exit_code: None,
        })?;

    // Read output
    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(ref mut out) = child.stdout {
        if let Err(e) = out.read_to_string(&mut stdout).await {
            let exit_code = capture_exit_code(&mut child).await;
            return Err(StepExecutionError::ShellFailed {
                message: format!("failed to read stdout: {}", e),
                exit_code,
            });
        }
    }
    if let Some(ref mut err) = child.stderr {
        if let Err(e) = err.read_to_string(&mut stderr).await {
            let exit_code = capture_exit_code(&mut child).await;
            return Err(StepExecutionError::ShellFailed {
                message: format!("failed to read stderr: {}", e),
                exit_code,
            });
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| StepExecutionError::ShellFailed {
            message: format!("failed to wait: {}", e),
            exit_code: None,
        })?;

    let duration_ms = start.elapsed().as_millis() as u64;

    if status.success() {
        Ok(StepResult {
            output: Some(stdout.trim().to_string()),
            outputs: std::collections::HashMap::new(),
            failed: false,
            error: None,
            duration_ms,
            backend: Some("shell".into()),
            backends: vec!["shell".into()],
        })
    } else {
        let error_msg = if stderr.is_empty() {
            format!("command exited with code {:?}", status.code())
        } else {
            stderr.trim().to_string()
        };

        if step.continue_on_error {
            Ok(StepResult {
                output: Some(stdout.trim().to_string()),
                outputs: std::collections::HashMap::new(),
                failed: true,
                error: Some(error_msg),
                duration_ms,
                backend: Some("shell".into()),
                backends: vec!["shell".into()],
            })
        } else {
            Err(StepExecutionError::ShellFailed {
                message: error_msg,
                exit_code: status.code(),
            })
        }
    }
}

/// Execute a query step
async fn execute_query_step(
    step: &StepConfig,
    ctx: &ExecutionContext,
    template_ctx: &TemplateContext,
    team: Option<&str>,
) -> Result<StepResult, StepExecutionError> {
    let role_name = step
        .role
        .as_ref()
        .ok_or_else(|| StepExecutionError::MissingField {
            step: step.name.clone(),
            field: "role".into(),
        })?;

    let prompt = step
        .prompt
        .as_ref()
        .ok_or_else(|| StepExecutionError::MissingField {
            step: step.name.clone(),
            field: "prompt".into(),
        })?;

    // Render prompt template
    let rendered_prompt = ctx.template_engine.render(prompt, template_ctx)?;

    // Resolve role to backends
    let resolved_role = resolve_role(role_name, team, &ctx.config)?;

    // Create backend request
    let request = BackendRequest::new(rendered_prompt);

    // Execute
    let result = ctx.role_executor.execute(&resolved_role, &request).await?;

    Ok(result.to_step_result())
}

/// Execute an apply step
async fn execute_apply_step(
    step: &StepConfig,
    ctx: &ExecutionContext,
    template_ctx: &TemplateContext,
    working_dir: &std::path::Path,
) -> Result<StepResult, StepExecutionError> {
    let start = Instant::now();

    // Get source step name
    let source_step = step
        .source
        .as_ref()
        .ok_or_else(|| StepExecutionError::MissingField {
            step: step.name.clone(),
            field: "source".into(),
        })?;

    // Get source step's output from template context
    let source_output = template_ctx
        .steps
        .get(source_step)
        .and_then(|r| r.output.as_ref())
        .ok_or_else(|| StepExecutionError::SourceNotFound {
            step: step.name.clone(),
            source_step: source_step.clone(),
        })?;

    // Build apply-verify config from step config
    let config = ApplyVerifyConfig {
        source_step: source_step.clone(),
        verify_command: step.verify.clone(),
        verify_retries: step.verify_retries,
        rollback_strategy: if step.rollback_on_failure {
            RollbackStrategy::Git
        } else {
            RollbackStrategy::None
        },
        timeout: None,
        verify_timeout: Some(Duration::from_secs(300)),
        retry_prompt: step.verify_retry_prompt.clone(),
    };

    // Run apply (with or without verification)
    if config.verify_command.is_some() {
        let result = apply_and_verify(source_output, &config, working_dir).await?;

        Ok(StepResult {
            output: result.output,
            outputs: std::collections::HashMap::new(),
            failed: !result.success,
            error: result.error,
            duration_ms: start.elapsed().as_millis() as u64,
            backend: Some("apply".into()),
            backends: vec!["apply".into()],
        })
    } else {
        let result = apply_only(source_output, working_dir).await?;

        Ok(StepResult {
            output: Some(format!(
                "Applied edits to {} file(s)",
                result.modified_files.len() + result.created_files.len()
            )),
            outputs: std::collections::HashMap::new(),
            failed: false,
            error: None,
            duration_ms: start.elapsed().as_millis() as u64,
            backend: Some("apply".into()),
            backends: vec!["apply".into()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, RoleConfig, RoleExecution, StepConfig, StepType};

    use tempfile::TempDir;

    fn create_test_config() -> LlmuxConfig {
        let mut config = LlmuxConfig::default();

        config.backends.insert(
            "echo".into(),
            BackendConfig {
                command: "echo".into(),
                enabled: true,
                ..Default::default()
            },
        );

        config.roles.insert(
            "test".into(),
            RoleConfig {
                description: "Test role".into(),
                backends: vec!["echo".into()],
                execution: RoleExecution::First,
                min_success: 1,
            },
        );

        config
    }

    #[tokio::test]
    async fn test_execute_shell_step() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("echo 'hello world'".into()),
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        assert!(!result.failed);
        assert!(result.output.is_some());
        assert!(result.output.unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn test_execute_shell_with_template() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let mut template_ctx = TemplateContext::new();
        template_ctx.args.insert("name".into(), "world".into());
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("echo 'hello {{ args.name }}'".into()),
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        assert!(!result.failed);
        assert!(result.output.unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn test_execute_shell_failure() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("exit 1".into()),
            continue_on_error: false,
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path()).await;

        assert!(matches!(
            result,
            Err(StepExecutionError::ShellFailed { .. })
        ));
    }

    #[tokio::test]
    async fn test_execute_shell_continue_on_error() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("exit 1".into()),
            continue_on_error: true,
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        assert!(result.failed);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_execute_skipped_condition() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("echo 'should not run'".into()),
            condition: Some("false".into()),
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        assert!(!result.failed);
        assert!(result.error.unwrap().contains("skipped"));
    }

    #[tokio::test]
    async fn test_execute_query_step() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Query,
            role: Some("test".into()),
            prompt: Some("hello world".into()),
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        // Using echo backend, should get prompt back
        assert!(!result.failed);
        assert!(result.output.is_some());
    }
}
