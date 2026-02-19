#![allow(dead_code, unused_variables)]

//! Step execution logic

use crate::apply_and_verify::RollbackStrategy;
use crate::apply_and_verify::{ApplyVerifyConfig, ApplyVerifyError, apply_and_verify, apply_only};
use crate::backend_executor::BackendRequest;
use crate::config::{LlmuxConfig, StepConfig, StepResult, StepType};
use crate::process::{OutputStream, OutputWaitError, exit_status_code, wait_for_child_output};
use crate::role::{RoleExecutor, resolve_role};
use crate::template::{TemplateContext, TemplateEngine, evaluate_condition};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

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

    #[error("shell command timed out after {0:?}")]
    ShellTimeout(Duration),
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

/// Execute a single step
pub async fn execute_step(
    step: &StepConfig,
    ctx: &ExecutionContext,
    template_ctx: &TemplateContext,
    team: Option<&str>,
    working_dir: &std::path::Path,
) -> Result<StepResult, StepExecutionError> {
    let start = Instant::now();

    tracing::info!(
        step = %step.name,
        step_type = ?step.step_type,
        "Executing step"
    );

    // Check condition
    if let Some(ref condition) = step.condition {
        let should_run = evaluate_condition(condition, template_ctx)?;
        if !should_run {
            tracing::info!(step = %step.name, "Step skipped: condition evaluated to false");
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

    let result = match step.step_type {
        StepType::Shell => execute_shell_step(step, ctx, template_ctx, working_dir).await,
        StepType::Query => execute_query_step(step, ctx, template_ctx, team).await,
        StepType::Apply => execute_apply_step(step, ctx, template_ctx, working_dir).await,
        StepType::Store => execute_store_step(step, ctx, template_ctx).await,
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
    };

    match &result {
        Ok(step_result) => {
            if step_result.failed {
                tracing::error!(
                    step = %step.name,
                    error = ?step_result.error,
                    duration_ms = step_result.duration_ms,
                    "Step failed"
                );
            } else {
                tracing::info!(
                    step = %step.name,
                    duration_ms = step_result.duration_ms,
                    backend = ?step_result.backend,
                    "Step completed"
                );
            }
        }
        Err(e) => {
            tracing::error!(
                step = %step.name,
                error = %e,
                "Step execution error"
            );
        }
    }

    result
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

    let timeout_duration = step.timeout.map(Duration::from_millis);

    let map_wait_error = |err: OutputWaitError| match err {
        OutputWaitError::Read {
            stream,
            source,
            exit_code,
        } => {
            let stream_label = match stream {
                OutputStream::Stdout => "stdout",
                OutputStream::Stderr => "stderr",
            };
            StepExecutionError::ShellFailed {
                message: format!("failed to read {}: {}", stream_label, source),
                exit_code,
            }
        }
        OutputWaitError::Wait { source } => StepExecutionError::ShellFailed {
            message: format!("failed to wait: {}", source),
            exit_code: None,
        },
    };

    let output_result = if let Some(dur) = timeout_duration {
        match timeout(dur, wait_for_child_output(&mut child)).await {
            Ok(result) => result.map_err(map_wait_error),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await; // Reap the process
                let duration_ms = start.elapsed().as_millis() as u64;
                if step.continue_on_error {
                    return Ok(StepResult {
                        output: None,
                        outputs: std::collections::HashMap::new(),
                        failed: true,
                        error: Some(format!("command timed out after {:?}", dur)),
                        duration_ms,
                        backend: Some("shell".into()),
                        backends: vec!["shell".into()],
                    });
                }
                return Err(StepExecutionError::ShellTimeout(dur));
            }
        }
    } else {
        wait_for_child_output(&mut child)
            .await
            .map_err(map_wait_error)
    };

    let (stdout, stderr, status) = output_result?;

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
            format!("command exited with code {:?}", exit_status_code(&status))
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
                exit_code: exit_status_code(&status),
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
    let mut rendered_prompt = ctx.template_engine.render(prompt, template_ctx)?;

    // If output_schema is present, append JSON formatting instructions
    if let Some(ref schema) = step.output_schema {
        let schema_json = serde_json::to_string_pretty(schema).unwrap_or_else(|_| "{}".to_string());

        rendered_prompt.push_str(&format!(
            "\n\nIMPORTANT: You MUST respond with valid JSON matching this schema:\n```json\n{}\n```\n\nDo not include any text before or after the JSON object.",
            schema_json
        ));
    }

    // Resolve role to backends
    let resolved_role = resolve_role(role_name, team, &ctx.config)?;

    // Create backend request
    let request = BackendRequest::new(rendered_prompt);

    // Execute
    let result = ctx.role_executor.execute(&resolved_role, &request).await?;
    let mut step_result = result.to_step_result();

    // Validate against schema if present
    if let Some(ref schema) = step.output_schema {
        if let Some(ref output) = step_result.output {
            if let Err(e) = validate_json_schema(output, schema) {
                step_result.failed = true;
                step_result.error = Some(format!("Output validation failed: {}", e));
            }
        }
    }

    Ok(step_result)
}

/// Strip markdown code fences from output if present
fn strip_markdown_fences(output: &str) -> &str {
    let trimmed = output.trim();

    // Strip backend headers (=== backend ===) if present
    let without_header = if let Some(header_end) = trimmed.find("\n") {
        if trimmed.starts_with("===") {
            trimmed[header_end + 1..].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    // Look for ```json fence anywhere in the output
    if let Some(start_pos) = without_header.find("```json") {
        // Find the closing ``` after the opening fence
        if let Some(end_pos) = without_header[start_pos + 7..].find("```") {
            return without_header[start_pos + 7..start_pos + 7 + end_pos].trim();
        }
    }

    // Look for generic ``` fence anywhere in the output
    if let Some(start_pos) = without_header.find("```") {
        // Find the closing ``` after the opening fence
        if let Some(end_pos) = without_header[start_pos + 3..].find("```") {
            return without_header[start_pos + 3..start_pos + 3 + end_pos].trim();
        }
    }

    // No fences found, return as-is
    without_header
}

/// Validate JSON output against a schema
fn validate_json_schema(output: &str, schema: &crate::config::OutputSchema) -> Result<(), String> {
    // Strip markdown code fences if present
    let clean_output = strip_markdown_fences(output);

    // Parse the output as JSON
    let json: serde_json::Value =
        serde_json::from_str(clean_output).map_err(|e| format!("Invalid JSON: {}", e))?;

    // Check that it's an object if schema_type is "object"
    if schema.schema_type == "object" {
        let obj = json
            .as_object()
            .ok_or_else(|| "Expected object, got something else".to_string())?;

        // Check required fields
        for required_field in &schema.required {
            if !obj.contains_key(required_field) {
                return Err(format!("Missing required field: {}", required_field));
            }
        }

        // Validate property types
        for (prop_name, prop_schema) in &schema.properties {
            if let Some(value) = obj.get(prop_name) {
                validate_property_type(value, prop_schema)?;
            }
        }
    }

    Ok(())
}

/// Validate a property value against its schema
fn validate_property_type(
    value: &serde_json::Value,
    schema: &crate::config::PropertySchema,
) -> Result<(), String> {
    match schema.prop_type.as_str() {
        "string" => {
            if !value.is_string() {
                return Err(format!("Expected string, got {:?}", value));
            }
        }
        "number" => {
            if !value.is_number() {
                return Err(format!("Expected number, got {:?}", value));
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(format!("Expected boolean, got {:?}", value));
            }
        }
        "array" => {
            let arr = value
                .as_array()
                .ok_or_else(|| format!("Expected array, got {:?}", value))?;

            // If items schema is present, validate each item
            if let Some(ref items_schema) = schema.items {
                for item in arr {
                    validate_property_type(item, items_schema)?;
                }
            }
        }
        "object" => {
            if !value.is_object() {
                return Err(format!("Expected object, got {:?}", value));
            }
        }
        _ => {
            return Err(format!("Unknown type: {}", schema.prop_type));
        }
    }
    Ok(())
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

/// Execute a store step - saves discovered data to memory database
async fn execute_store_step(
    step: &StepConfig,
    ctx: &ExecutionContext,
    template_ctx: &TemplateContext,
) -> Result<StepResult, StepExecutionError> {
    let start = Instant::now();

    // Get the input data (from previous step's output)
    let input = step
        .prompt
        .as_ref()
        .ok_or_else(|| StepExecutionError::MissingField {
            step: step.name.clone(),
            field: "prompt".into(),
        })?;

    // Render template to get the actual JSON data
    let rendered_data = ctx.template_engine.render(input, template_ctx)?;

    // Strip markdown fences if present
    let json_data = strip_markdown_fences(&rendered_data).to_string();

    // Get ecosystem name from context
    let ecosystem_name = template_ctx
        .ecosystem
        .as_ref()
        .map(|e| e.name.clone())
        .ok_or_else(|| StepExecutionError::MissingField {
            step: step.name.clone(),
            field: "ecosystem".into(),
        })?;

    // Parse and store the data
    let result = store_json_data(&ecosystem_name, &json_data);

    let (summary, failed, error) = match result {
        Ok(msg) => (msg, false, None),
        Err(e) => (
            format!("Failed to store data: {}", e),
            true,
            Some(format!("{}", e)),
        ),
    };

    Ok(StepResult {
        output: Some(summary),
        outputs: std::collections::HashMap::new(),
        failed,
        error,
        duration_ms: start.elapsed().as_millis() as u64,
        backend: Some("store".into()),
        backends: vec!["store".into()],
    })
}

/// Parse JSON output from LLM and store in SQLite memory database
fn store_json_data(ecosystem: &str, json_data: &str) -> Result<String, anyhow::Error> {
    use crate::memory::{EcosystemMemory, Entity, EntityProperty, Fact, ProjectRelationship};

    // Parse JSON
    let parsed: serde_json::Value = serde_json::from_str(json_data)?;

    // Open memory database
    let db_path = EcosystemMemory::default_path(ecosystem)?;
    let mut memory = EcosystemMemory::open(&db_path)?;

    let mut facts_stored = 0;
    let mut relationships_stored = 0;
    let mut entities_stored = 0;

    // Store facts if present
    if let Some(facts_array) = parsed.get("facts").and_then(|v| v.as_array()) {
        for fact_json in facts_array {
            if let (Some(project), Some(fact_text), Some(source), Some(confidence)) = (
                fact_json.get("project").and_then(|v| v.as_str()),
                fact_json.get("fact").and_then(|v| v.as_str()),
                fact_json.get("source").and_then(|v| v.as_str()),
                fact_json.get("confidence").and_then(|v| v.as_f64()),
            ) {
                // Extract optional category and source_type
                let category = fact_json
                    .get("category")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let source_type = fact_json
                    .get("source_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let fact = Fact {
                    id: None,
                    ecosystem: ecosystem.to_string(),
                    fact: format!("{}: {}", project, fact_text),
                    source: source.to_string(),
                    source_type,
                    category,
                    confidence,
                    created_at: String::new(),
                    updated_at: String::new(),
                };

                memory.add_fact(&fact)?;
                facts_stored += 1;
            }
        }
    }

    // Store relationships if present
    if let Some(relationships_array) = parsed.get("relationships").and_then(|v| v.as_array()) {
        for rel_json in relationships_array {
            if let (Some(from), Some(to), Some(rel_type)) = (
                rel_json.get("from").and_then(|v| v.as_str()),
                rel_json.get("to").and_then(|v| v.as_str()),
                rel_json.get("type").and_then(|v| v.as_str()),
            ) {
                let evidence = rel_json
                    .get("evidence")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let rel = ProjectRelationship {
                    id: None,
                    ecosystem: ecosystem.to_string(),
                    from_project: from.to_string(),
                    to_project: to.to_string(),
                    relationship_type: rel_type.to_string(),
                    metadata: evidence.map(|e| format!(r#"{{"evidence":"{}"}}"#, e)),
                    created_at: String::new(),
                };

                memory.add_relationship(&rel)?;
                relationships_stored += 1;
            }
        }
    }

    // Store entities if present
    if let Some(entities_array) = parsed.get("entities").and_then(|v| v.as_array()) {
        for entity_json in entities_array {
            if let (Some(entity_type), Some(entity_name), Some(source)) = (
                entity_json.get("entity_type").and_then(|v| v.as_str()),
                entity_json.get("entity_name").and_then(|v| v.as_str()),
                entity_json.get("source").and_then(|v| v.as_str()),
            ) {
                let project = entity_json
                    .get("project")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let source_type = entity_json
                    .get("source_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let confidence = entity_json
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);

                // Get or create the entity
                let entity = Entity {
                    id: None,
                    ecosystem: ecosystem.to_string(),
                    project: project.to_string(),
                    entity_type: entity_type.to_string(),
                    entity_name: entity_name.to_string(),
                    created_at: String::new(),
                };
                let entity_id = memory.get_or_create_entity(&entity)?;

                // Store each property
                if let Some(properties) = entity_json.get("properties").and_then(|v| v.as_object())
                {
                    for (prop_name, prop_value) in properties {
                        let prop_value_str = match prop_value {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            other => other.to_string(),
                        };

                        let property = EntityProperty {
                            id: None,
                            entity_id,
                            property_name: prop_name.clone(),
                            property_value: prop_value_str,
                            source: source.to_string(),
                            source_type: source_type.clone(),
                            confidence,
                            valid_from: String::new(),
                            valid_to: None,
                            created_at: String::new(),
                        };
                        memory.set_entity_property(&property)?;
                    }
                }

                entities_stored += 1;
            }
        }
    }

    Ok(format!(
        "Stored {} facts, {} relationships, and {} entities in {}",
        facts_stored,
        relationships_stored,
        entities_stored,
        db_path.display()
    ))
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_execute_shell_timeout() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("sleep 1".into()),
            timeout: Some(50),
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path()).await;

        assert!(matches!(result, Err(StepExecutionError::ShellTimeout(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_execute_shell_timeout_continue_on_error() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("sleep 1".into()),
            timeout: Some(50),
            continue_on_error: true,
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        assert!(result.failed);
        assert!(result.error.unwrap().contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_execute_shell_timeout_success() {
        let config = Arc::new(create_test_config());
        let ctx = ExecutionContext::new(config);
        let template_ctx = TemplateContext::new();
        let dir = TempDir::new().unwrap();

        let step = StepConfig {
            name: "test".into(),
            step_type: StepType::Shell,
            run: Some("sleep 1; echo done".into()),
            timeout: Some(2000),
            ..Default::default()
        };

        let result = execute_step(&step, &ctx, &template_ctx, None, dir.path())
            .await
            .unwrap();

        assert!(!result.failed);
        assert!(result.output.unwrap().contains("done"));
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
