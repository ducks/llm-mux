//! Workflow and step configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Step type - explicit, not inferred
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepType {
    /// Run a shell command
    Shell,
    /// Query LLM backend(s)
    Query,
    /// Apply file edits
    Apply,
    /// Wait for human input
    Input,
}

/// Argument definition for a workflow
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArgDef {
    /// Whether this argument is required
    #[serde(default)]
    pub required: bool,

    /// Default value if not provided
    pub default: Option<String>,

    /// Description for help text
    #[serde(default)]
    pub description: String,
}

/// Configuration for a workflow step
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StepConfig {
    /// Step name (unique within workflow)
    pub name: String,

    /// Step type
    #[serde(rename = "type")]
    pub step_type: StepType,

    /// Role to use (for query steps)
    pub role: Option<String>,

    /// Run all backends in role in parallel
    #[serde(default)]
    pub parallel: bool,

    /// Minimum successful backends (for parallel)
    pub min_success: Option<u32>,

    /// Prompt template (for query steps)
    pub prompt: Option<String>,

    /// Command to run (for shell steps)
    pub run: Option<String>,

    /// Source step for edits (for apply steps)
    pub source: Option<String>,

    /// Verification command (for apply steps)
    pub verify: Option<String>,

    /// Number of verification retries
    #[serde(default)]
    pub verify_retries: u32,

    /// Prompt for re-query on verification failure
    pub verify_retry_prompt: Option<String>,

    /// Rollback on failure (for apply steps)
    #[serde(default)]
    pub rollback_on_failure: bool,

    /// Steps this step depends on
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Conditional execution expression
    #[serde(rename = "if")]
    pub condition: Option<String>,

    /// Iterate over an array
    pub for_each: Option<String>,

    /// Continue workflow if this step fails
    #[serde(default)]
    pub continue_on_error: bool,

    /// Timeout in milliseconds
    pub timeout: Option<u64>,

    /// Number of retries on transient failure
    #[serde(default)]
    pub retries: u32,

    /// Retry delay in milliseconds
    #[serde(default = "default_retry_delay")]
    pub retry_delay: u64,

    /// Expected output schema (for validation)
    pub output_schema: Option<OutputSchema>,

    /// Human-readable options (for input steps)
    pub options: Option<Vec<String>>,
}

fn default_retry_delay() -> u64 {
    1000
}

impl Default for StepConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            step_type: StepType::Shell,
            role: None,
            parallel: false,
            min_success: None,
            prompt: None,
            run: None,
            source: None,
            verify: None,
            verify_retries: 0,
            verify_retry_prompt: None,
            rollback_on_failure: false,
            depends_on: Vec::new(),
            condition: None,
            for_each: None,
            continue_on_error: false,
            timeout: None,
            retries: 0,
            retry_delay: default_retry_delay(),
            output_schema: None,
            options: None,
        }
    }
}


/// JSON Schema subset for output validation
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,

    #[serde(default)]
    pub required: Vec<String>,

    #[serde(default)]
    pub properties: HashMap<String, PropertySchema>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub prop_type: String,

    pub items: Option<Box<PropertySchema>>,
}

/// Full workflow configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct WorkflowConfig {
    /// Workflow name
    pub name: String,

    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Workflow version
    pub version: Option<u32>,

    /// Workflow arguments
    #[serde(default)]
    pub args: HashMap<String, ArgDef>,

    /// Workflow-level timeout in milliseconds
    pub timeout: Option<u64>,

    /// Workflow-level continue_on_error default
    #[serde(default)]
    pub continue_on_error: bool,

    /// Steps in this workflow
    #[serde(default)]
    pub steps: Vec<StepConfig>,
}

impl WorkflowConfig {
    /// Validate the workflow configuration
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Check for duplicate step names
        let mut seen_names = std::collections::HashSet::new();
        for step in &self.steps {
            if !seen_names.insert(&step.name) {
                errors.push(format!("duplicate step name: {}", step.name));
            }
        }

        // Check depends_on references
        let step_names: std::collections::HashSet<_> =
            self.steps.iter().map(|s| s.name.as_str()).collect();
        for step in &self.steps {
            for dep in &step.depends_on {
                if !step_names.contains(dep.as_str()) {
                    errors.push(format!(
                        "step '{}' depends on unknown step '{}'",
                        step.name, dep
                    ));
                }
            }
        }

        // Check step type requirements
        for step in &self.steps {
            match step.step_type {
                StepType::Shell => {
                    if step.run.is_none() {
                        errors.push(format!("shell step '{}' missing 'run' field", step.name));
                    }
                }
                StepType::Query => {
                    if step.prompt.is_none() {
                        errors.push(format!("query step '{}' missing 'prompt' field", step.name));
                    }
                    if step.role.is_none() {
                        errors.push(format!("query step '{}' missing 'role' field", step.name));
                    }
                }
                StepType::Apply => {
                    if step.source.is_none() {
                        errors.push(format!("apply step '{}' missing 'source' field", step.name));
                    }
                }
                StepType::Input => {
                    if step.prompt.is_none() {
                        errors.push(format!("input step '{}' missing 'prompt' field", step.name));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_config_shell() {
        let toml = r#"
            name = "fetch"
            type = "shell"
            run = "gh issue view 123"
        "#;
        let step: StepConfig = toml::from_str(toml).unwrap();
        assert_eq!(step.name, "fetch");
        assert_eq!(step.step_type, StepType::Shell);
        assert_eq!(step.run, Some("gh issue view 123".into()));
    }

    #[test]
    fn test_step_config_query() {
        let toml = r#"
            name = "analyze"
            type = "query"
            role = "analyzer"
            parallel = true
            prompt = "Find bugs..."
            depends_on = ["fetch"]
        "#;
        let step: StepConfig = toml::from_str(toml).unwrap();
        assert_eq!(step.step_type, StepType::Query);
        assert_eq!(step.role, Some("analyzer".into()));
        assert!(step.parallel);
        assert_eq!(step.depends_on, vec!["fetch"]);
    }

    #[test]
    fn test_workflow_config() {
        let toml = r#"
            name = "hunt"
            description = "Find bugs"
            version = 1

            [args.dir]
            required = false
            default = "."

            [[steps]]
            name = "analyze"
            type = "query"
            role = "analyzer"
            prompt = "Find bugs"
        "#;
        let workflow: WorkflowConfig = toml::from_str(toml).unwrap();
        assert_eq!(workflow.name, "hunt");
        assert!(workflow.args.contains_key("dir"));
        assert_eq!(workflow.steps.len(), 1);
    }

    #[test]
    fn test_workflow_validation() {
        let workflow = WorkflowConfig {
            name: "test".into(),
            description: String::new(),
            version: None,
            args: HashMap::new(),
            timeout: None,
            continue_on_error: false,
            steps: vec![
                StepConfig {
                    name: "good".into(),
                    step_type: StepType::Shell,
                    run: Some("echo test".into()),
                    role: None,
                    parallel: false,
                    min_success: None,
                    prompt: None,
                    source: None,
                    verify: None,
                    verify_retries: 0,
                    verify_retry_prompt: None,
                    rollback_on_failure: false,
                    depends_on: vec![],
                    condition: None,
                    for_each: None,
                    continue_on_error: false,
                    timeout: None,
                    retries: 0,
                    retry_delay: 1000,
                    output_schema: None,
                    options: None,
                },
                StepConfig {
                    name: "bad".into(),
                    step_type: StepType::Query,
                    run: None,
                    role: None, // Missing!
                    parallel: false,
                    min_success: None,
                    prompt: None, // Missing!
                    source: None,
                    verify: None,
                    verify_retries: 0,
                    verify_retry_prompt: None,
                    rollback_on_failure: false,
                    depends_on: vec!["nonexistent".into()], // Invalid!
                    condition: None,
                    for_each: None,
                    continue_on_error: false,
                    timeout: None,
                    retries: 0,
                    retry_delay: 1000,
                    output_schema: None,
                    options: None,
                },
            ],
        };

        let result = workflow.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("nonexistent")));
        assert!(errors.iter().any(|e| e.contains("prompt")));
        assert!(errors.iter().any(|e| e.contains("role")));
    }
}
