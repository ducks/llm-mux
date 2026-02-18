//! Workflow runner - orchestrates step execution

use super::detect_ecosystem;
use super::executor::{ExecutionContext, StepExecutionError, execute_step};
use super::state::{WorkflowResult, WorkflowState};
use crate::backend_executor::output_parser::extract_json;
use crate::config::{LlmuxConfig, StepResult, WorkflowConfig};
use crate::role::detect_team;
use crate::template::evaluate_expression;
use minijinja::value::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Errors during workflow execution
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("step '{step}' failed: {message}")]
    StepFailed { step: String, message: String },

    #[error("step execution error: {0}")]
    Execution(#[from] StepExecutionError),

    #[error("circular dependency detected involving step '{step}'")]
    CircularDependency { step: String },

    #[error("step '{step}' depends on unknown step '{dependency}'")]
    UnknownDependency { step: String, dependency: String },

    #[error("template error: {0}")]
    Template(#[from] crate::template::TemplateError),
}

/// Workflow runner
pub struct WorkflowRunner {
    config: Arc<LlmuxConfig>,
}

impl WorkflowRunner {
    /// Create a new workflow runner
    pub fn new(config: Arc<LlmuxConfig>) -> Self {
        Self { config }
    }

    /// Create output directory for workflow run
    fn create_output_dir(workflow_name: &str) -> Result<PathBuf, WorkflowError> {
        let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let dir_name = format!("{}-{}", workflow_name, timestamp);

        let output_dir = std::env::temp_dir().join("llm-mux").join("workflows").join(dir_name);

        std::fs::create_dir_all(&output_dir).map_err(|e| {
            WorkflowError::StepFailed {
                step: "create_output_dir".into(),
                message: format!("Failed to create output directory: {}", e),
            }
        })?;

        tracing::info!(path = %output_dir.display(), "Created workflow output directory");
        Ok(output_dir)
    }

    /// Save step output to file
    fn save_step_output(
        output_dir: &Path,
        step_name: &str,
        output: &str,
        failed: bool,
    ) -> Result<(), std::io::Error> {
        let filename = if failed {
            format!("{}.failed.txt", step_name)
        } else {
            format!("{}.txt", step_name)
        };

        let output_path = output_dir.join(filename);
        std::fs::write(&output_path, output)?;

        tracing::debug!(
            step = step_name,
            path = %output_path.display(),
            "Saved step output"
        );

        Ok(())
    }

    /// Run a workflow
    pub async fn run(
        &self,
        workflow: WorkflowConfig,
        args: HashMap<String, String>,
        working_dir: &Path,
        team_override: Option<&str>,
    ) -> Result<WorkflowResult, WorkflowError> {
        // Validate workflow first
        self.validate_workflow(&workflow)?;

        // Create output directory for this workflow run
        let output_dir = Self::create_output_dir(&workflow.name)?;

        // Detect team
        let team = detect_team(working_dir, &self.config.teams, team_override);

        // Detect ecosystem
        let ecosystem = detect_ecosystem(working_dir, &self.config.ecosystems);

        // Create state
        let mut state = WorkflowState::new(workflow.clone(), args, working_dir.to_path_buf());

        if let Some(ref team_name) = team {
            if let Some(team_config) = self.config.teams.get(team_name) {
                state = state.with_team(team_name.clone(), team_config.clone());
            }
        }

        if let Some((ecosystem_name, project_name)) = ecosystem {
            if let Some(ecosystem_config) = self.config.ecosystems.get(&ecosystem_name) {
                state = state.with_ecosystem(
                    ecosystem_name.clone(),
                    ecosystem_config.clone(),
                    Some(project_name),
                );
            }
        }

        // Create execution context
        let ctx = ExecutionContext::new(self.config.clone());

        // Get execution order
        let order = self.topological_sort(&workflow)?;

        // Execute steps in order
        for step_name in order {
            if state.failed && !workflow.continue_on_error {
                break;
            }

            if let Some(step) = workflow.steps.iter().find(|s| s.name == step_name) {
                let mut template_ctx = state.to_template_context();

                // Handle for_each
                if let Some(ref for_each_expr) = step.for_each {
                    let items = self.evaluate_for_each(for_each_expr, &template_ctx)?;
                    let mut results = Vec::new();

                    for (idx, item) in items.into_iter().enumerate() {
                        // Reuse context, just update item (avoids expensive clone)
                        template_ctx.set_item(item);

                        match execute_step(step, &ctx, &template_ctx, team.as_deref(), working_dir)
                            .await
                        {
                            Ok(result) => {
                                // Save output for each iteration
                                if let Some(ref output) = result.output {
                                    let iter_step_name = format!("{}.{}", step_name, idx);
                                    if let Err(e) = Self::save_step_output(
                                        &output_dir,
                                        &iter_step_name,
                                        output,
                                        result.failed,
                                    ) {
                                        tracing::warn!(
                                            step = &iter_step_name,
                                            error = %e,
                                            "Failed to save iteration output"
                                        );
                                    }
                                }
                                results.push(result);
                            }
                            Err(e) if step.continue_on_error => {
                                let error_msg = e.to_string();
                                let iter_step_name = format!("{}.{}", step_name, idx);

                                // Save error for this iteration
                                if let Err(err) = Self::save_step_output(
                                    &output_dir,
                                    &iter_step_name,
                                    &error_msg,
                                    true,
                                ) {
                                    tracing::warn!(
                                        step = &iter_step_name,
                                        error = %err,
                                        "Failed to save iteration error"
                                    );
                                }

                                results.push(StepResult::failure(error_msg, 0));
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    // Clear item after loop
                    template_ctx.clear_item();

                    // Aggregate results
                    let aggregated = self.aggregate_for_each_results(results);
                    state.add_result(&step_name, aggregated, step.continue_on_error);
                } else {
                    // Regular step execution
                    match execute_step(step, &ctx, &template_ctx, team.as_deref(), working_dir)
                        .await
                    {
                        Ok(result) => {
                            // Save step output to file
                            if let Some(ref output) = result.output {
                                if let Err(e) = Self::save_step_output(
                                    &output_dir,
                                    &step_name,
                                    output,
                                    result.failed,
                                ) {
                                    tracing::warn!(
                                        step = &step_name,
                                        error = %e,
                                        "Failed to save step output"
                                    );
                                }
                            }

                            state.add_result(&step_name, result, step.continue_on_error);
                        }
                        Err(e) if step.continue_on_error => {
                            let error_msg = e.to_string();

                            // Save error output
                            if let Err(err) =
                                Self::save_step_output(&output_dir, &step_name, &error_msg, true)
                            {
                                tracing::warn!(
                                    step = &step_name,
                                    error = %err,
                                    "Failed to save error output"
                                );
                            }

                            state.add_result(
                                &step_name,
                                StepResult::failure(error_msg, 0),
                                true,
                            );
                        }
                        Err(e) => {
                            let error_msg = e.to_string();

                            // Save error output before returning
                            if let Err(err) =
                                Self::save_step_output(&output_dir, &step_name, &error_msg, true)
                            {
                                tracing::warn!(
                                    step = &step_name,
                                    error = %err,
                                    "Failed to save error output"
                                );
                            }

                            return Err(WorkflowError::StepFailed {
                                step: step_name.clone(),
                                message: error_msg,
                            });
                        }
                    }
                }
            }
        }

        tracing::info!(
            output_dir = %output_dir.display(),
            "Workflow outputs saved"
        );

        let mut result = WorkflowResult::from_state(&state);
        result.output_dir = Some(output_dir.to_string_lossy().to_string());
        Ok(result)
    }

    /// Validate workflow before execution
    fn validate_workflow(&self, workflow: &WorkflowConfig) -> Result<(), WorkflowError> {
        // Check for unknown dependencies
        let step_names: std::collections::HashSet<_> =
            workflow.steps.iter().map(|s| s.name.as_str()).collect();

        for step in &workflow.steps {
            for dep in &step.depends_on {
                if !step_names.contains(dep.as_str()) {
                    return Err(WorkflowError::UnknownDependency {
                        step: step.name.clone(),
                        dependency: dep.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Topological sort of steps based on dependencies
    fn topological_sort(&self, workflow: &WorkflowConfig) -> Result<Vec<String>, WorkflowError> {
        let mut result = Vec::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut in_progress: std::collections::HashSet<String> = std::collections::HashSet::new();

        let step_map: HashMap<String, Vec<String>> = workflow
            .steps
            .iter()
            .map(|s| (s.name.clone(), s.depends_on.clone()))
            .collect();

        fn visit(
            step_name: &str,
            step_map: &HashMap<String, Vec<String>>,
            visited: &mut std::collections::HashSet<String>,
            in_progress: &mut std::collections::HashSet<String>,
            result: &mut Vec<String>,
        ) -> Result<(), WorkflowError> {
            if visited.contains(step_name) {
                return Ok(());
            }
            if in_progress.contains(step_name) {
                return Err(WorkflowError::CircularDependency {
                    step: step_name.to_string(),
                });
            }

            in_progress.insert(step_name.to_string());

            if let Some(deps) = step_map.get(step_name) {
                for dep in deps {
                    visit(dep, step_map, visited, in_progress, result)?;
                }
            }

            in_progress.remove(step_name);
            visited.insert(step_name.to_string());
            result.push(step_name.to_string());

            Ok(())
        }

        for step in &workflow.steps {
            visit(
                &step.name,
                &step_map,
                &mut visited,
                &mut in_progress,
                &mut result,
            )?;
        }

        Ok(result)
    }

    /// Evaluate for_each expression to get items
    fn evaluate_for_each(
        &self,
        expr: &str,
        ctx: &crate::template::TemplateContext,
    ) -> Result<Vec<Value>, WorkflowError> {
        // Try to evaluate as an expression
        let value = evaluate_expression(expr, ctx)?;

        // If it's a string, try to extract JSON first
        // (minijinja's try_iter on strings iterates characters, which is not what we want)
        if value.kind() == minijinja::value::ValueKind::String {
            let s = value.to_string();

            if let Some(json) = extract_json(&s) {
                // If we found JSON, try to iterate over it
                if let Some(arr) = json.as_array() {
                    return Ok(arr.iter().map(json_to_minijinja_value).collect());
                }
                // If it's an object, return as single item
                if json.is_object() {
                    return Ok(vec![json_to_minijinja_value(&json)]);
                }
            }

            // Fall back to comma-separated parsing for plain strings
            return Ok(s
                .split(',')
                .map(|s| Value::from(s.trim().to_string()))
                .collect());
        }

        // For non-strings (arrays, maps, etc.), try to iterate directly
        match value.try_iter() {
            Ok(iter) => Ok(iter.collect()),
            Err(_) => {
                // Shouldn't happen for non-strings, but fallback just in case
                Ok(vec![value])
            }
        }
    }

    /// Aggregate for_each results
    fn aggregate_for_each_results(&self, results: Vec<StepResult>) -> StepResult {
        let mut outputs = Vec::new();
        let mut all_failed = true;
        let mut any_failed = false;
        let mut total_duration = 0u64;
        let mut backends = Vec::new();

        for result in results {
            if let Some(output) = result.output {
                outputs.push(output);
            }
            if !result.failed {
                all_failed = false;
            }
            if result.failed {
                any_failed = true;
            }
            total_duration += result.duration_ms;
            backends.extend(result.backends);
        }

        StepResult {
            output: Some(outputs.join("\n")),
            outputs: HashMap::new(),
            failed: all_failed,
            error: if any_failed {
                Some("some iterations failed".into())
            } else {
                None
            },
            duration_ms: total_duration,
            backend: backends.first().cloned(),
            backends,
        }
    }
}

/// Convert serde_json::Value to minijinja::value::Value
fn json_to_minijinja_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::from(()),
        serde_json::Value::Bool(b) => Value::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(f) = n.as_f64() {
                Value::from(f)
            } else {
                Value::from(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::from(s.clone()),
        serde_json::Value::Array(arr) => {
            Value::from(arr.iter().map(json_to_minijinja_value).collect::<Vec<_>>())
        }
        serde_json::Value::Object(obj) => {
            let map: std::collections::BTreeMap<String, Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_minijinja_value(v)))
                .collect();
            Value::from_iter(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, StepConfig, StepType};
    use tempfile::TempDir;

    fn create_test_config() -> LlmuxConfig {
        let mut config = LlmuxConfig::default();
        config.backends.insert(
            "echo".into(),
            BackendConfig {
                command: "echo".into(),
                ..Default::default()
            },
        );
        config
    }

    fn create_test_workflow() -> WorkflowConfig {
        WorkflowConfig {
            name: "test".into(),
            description: "Test workflow".into(),
            version: Some(1),
            args: HashMap::new(),
            timeout: None,
            continue_on_error: false,
            steps: vec![
                StepConfig {
                    name: "step1".into(),
                    step_type: StepType::Shell,
                    run: Some("echo 'step1'".into()),
                    ..Default::default()
                },
                StepConfig {
                    name: "step2".into(),
                    step_type: StepType::Shell,
                    run: Some("echo 'step2'".into()),
                    depends_on: vec!["step1".into()],
                    ..Default::default()
                },
            ],
        }
    }

    #[tokio::test]
    async fn test_run_simple_workflow() {
        let config = Arc::new(create_test_config());
        let runner = WorkflowRunner::new(config);
        let workflow = create_test_workflow();
        let dir = TempDir::new().unwrap();

        let result = runner
            .run(workflow, HashMap::new(), dir.path(), None)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.steps.len(), 2);
        assert!(result.step_output("step1").is_some());
        assert!(result.step_output("step2").is_some());
    }

    #[test]
    fn test_topological_sort() {
        let config = Arc::new(create_test_config());
        let runner = WorkflowRunner::new(config);
        let workflow = create_test_workflow();

        let order = runner.topological_sort(&workflow).unwrap();

        // step1 should come before step2
        let step1_pos = order.iter().position(|s| s == "step1").unwrap();
        let step2_pos = order.iter().position(|s| s == "step2").unwrap();
        assert!(step1_pos < step2_pos);
    }

    #[test]
    fn test_circular_dependency_detection() {
        let config = Arc::new(create_test_config());
        let runner = WorkflowRunner::new(config);

        let workflow = WorkflowConfig {
            name: "circular".into(),
            steps: vec![
                StepConfig {
                    name: "a".into(),
                    step_type: StepType::Shell,
                    run: Some("echo a".into()),
                    depends_on: vec!["b".into()],
                    ..Default::default()
                },
                StepConfig {
                    name: "b".into(),
                    step_type: StepType::Shell,
                    run: Some("echo b".into()),
                    depends_on: vec!["a".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let result = runner.topological_sort(&workflow);
        assert!(matches!(
            result,
            Err(WorkflowError::CircularDependency { .. })
        ));
    }

    #[test]
    fn test_unknown_dependency_detection() {
        let config = Arc::new(create_test_config());
        let runner = WorkflowRunner::new(config);

        let workflow = WorkflowConfig {
            name: "unknown".into(),
            steps: vec![StepConfig {
                name: "a".into(),
                step_type: StepType::Shell,
                run: Some("echo a".into()),
                depends_on: vec!["nonexistent".into()],
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = runner.validate_workflow(&workflow);
        assert!(matches!(
            result,
            Err(WorkflowError::UnknownDependency { .. })
        ));
    }

    #[tokio::test]
    async fn test_workflow_with_args() {
        let config = Arc::new(create_test_config());
        let runner = WorkflowRunner::new(config);

        let workflow = WorkflowConfig {
            name: "args_test".into(),
            steps: vec![StepConfig {
                name: "echo_arg".into(),
                step_type: StepType::Shell,
                run: Some("echo {{ args.message }}".into()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut args = HashMap::new();
        args.insert("message".into(), "hello from args".into());

        let dir = TempDir::new().unwrap();
        let result = runner.run(workflow, args, dir.path(), None).await.unwrap();

        assert!(result.success);
        assert!(
            result
                .step_output("echo_arg")
                .unwrap()
                .contains("hello from args")
        );
    }

    #[tokio::test]
    async fn test_step_output_in_template() {
        let config = Arc::new(create_test_config());
        let runner = WorkflowRunner::new(config);

        let workflow = WorkflowConfig {
            name: "chain_test".into(),
            steps: vec![
                StepConfig {
                    name: "first".into(),
                    step_type: StepType::Shell,
                    run: Some("echo 'first_output'".into()),
                    ..Default::default()
                },
                StepConfig {
                    name: "second".into(),
                    step_type: StepType::Shell,
                    run: Some("echo 'got: {{ steps.first.output }}'".into()),
                    depends_on: vec!["first".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let dir = TempDir::new().unwrap();
        let result = runner
            .run(workflow, HashMap::new(), dir.path(), None)
            .await
            .unwrap();

        assert!(result.success);
        assert!(
            result
                .step_output("second")
                .unwrap()
                .contains("first_output")
        );
    }
}
