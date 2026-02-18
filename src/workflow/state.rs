#![allow(dead_code)]

//! Workflow execution state

use crate::config::{EcosystemConfig, StepResult, TeamConfig, WorkflowConfig};
use crate::template::TemplateContext;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Current state of workflow execution
#[derive(Debug)]
pub struct WorkflowState {
    /// Workflow being executed
    pub workflow: WorkflowConfig,

    /// CLI arguments
    pub args: HashMap<String, String>,

    /// Detected or specified team
    pub team: Option<String>,

    /// Team configuration
    pub team_config: Option<TeamConfig>,

    /// Detected ecosystem
    pub ecosystem: Option<String>,

    /// Ecosystem configuration
    pub ecosystem_config: Option<EcosystemConfig>,

    /// Current project within ecosystem
    pub current_project: Option<String>,

    /// Working directory
    pub working_dir: PathBuf,

    /// Results from completed steps
    pub step_results: HashMap<String, StepResult>,

    /// Overall workflow start time
    pub started_at: Instant,

    /// Whether workflow has failed
    pub failed: bool,

    /// Error message if failed
    pub error: Option<String>,
}

impl WorkflowState {
    /// Create new workflow state
    pub fn new(
        workflow: WorkflowConfig,
        args: HashMap<String, String>,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            workflow,
            args,
            team: None,
            team_config: None,
            ecosystem: None,
            ecosystem_config: None,
            current_project: None,
            working_dir,
            step_results: HashMap::new(),
            started_at: Instant::now(),
            failed: false,
            error: None,
        }
    }

    /// Set the team for this workflow
    pub fn with_team(mut self, team: String, config: TeamConfig) -> Self {
        self.team = Some(team);
        self.team_config = Some(config);
        self
    }

    /// Set the ecosystem for this workflow
    pub fn with_ecosystem(
        mut self,
        ecosystem: String,
        config: EcosystemConfig,
        current_project: Option<String>,
    ) -> Self {
        self.ecosystem = Some(ecosystem);
        self.ecosystem_config = Some(config);
        self.current_project = current_project;
        self
    }

    /// Add a step result
    /// If `step_continue_on_error` is true, a failed result won't fail the workflow
    pub fn add_result(
        &mut self,
        step_name: &str,
        result: StepResult,
        step_continue_on_error: bool,
    ) {
        if result.failed && !self.workflow.continue_on_error && !step_continue_on_error {
            self.failed = true;
            self.error = result.error.clone();
        }
        self.step_results.insert(step_name.to_string(), result);
    }

    /// Check if a step has completed
    pub fn has_result(&self, step_name: &str) -> bool {
        self.step_results.contains_key(step_name)
    }

    /// Get a step's result
    pub fn get_result(&self, step_name: &str) -> Option<&StepResult> {
        self.step_results.get(step_name)
    }

    /// Get total elapsed time
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Build template context from current state
    pub fn to_template_context(&self) -> TemplateContext {
        let mut ctx = TemplateContext::with_args(self.args.clone());

        // Add step results
        for (name, result) in &self.step_results {
            ctx.add_step(name.clone(), result.clone());
        }

        // Add team config if present
        if let Some(ref team_config) = self.team_config {
            ctx.set_team(team_config.clone());
        }

        // Add ecosystem config if present
        if let Some(ref ecosystem_name) = self.ecosystem {
            if let Some(ref ecosystem_config) = self.ecosystem_config {
                ctx.set_ecosystem(
                    ecosystem_name.clone(),
                    ecosystem_config.clone(),
                    self.current_project.clone(),
                );
            }
        }

        // Add workflow name
        ctx.set_workflow(self.workflow.name.clone());

        ctx
    }

    /// Check if all dependencies for a step are met
    pub fn dependencies_met(&self, step_name: &str) -> bool {
        if let Some(step) = self.workflow.steps.iter().find(|s| s.name == step_name) {
            step.depends_on.iter().all(|dep| self.has_result(dep))
        } else {
            false
        }
    }
}

/// Result of executing a workflow
#[derive(Debug)]
pub struct WorkflowResult {
    /// Step results map
    pub steps: HashMap<String, StepResult>,

    /// Overall success
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Total execution time
    pub duration: Duration,

    /// Team that was used
    pub team: Option<String>,

    /// Output directory where step outputs are saved
    pub output_dir: Option<String>,
}

impl WorkflowResult {
    /// Create from workflow state
    pub fn from_state(state: &WorkflowState) -> Self {
        Self {
            steps: state.step_results.clone(),
            success: !state.failed,
            error: state.error.clone(),
            duration: state.elapsed(),
            team: state.team.clone(),
            output_dir: None,
        }
    }

    /// Get a specific step's output
    pub fn step_output(&self, step_name: &str) -> Option<&str> {
        self.steps.get(step_name).and_then(|r| r.output.as_deref())
    }

    /// Get list of failed steps
    pub fn failed_steps(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|(_, r)| r.failed)
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;

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
                    step_type: crate::config::StepType::Shell,
                    run: Some("echo hello".into()),
                    ..Default::default()
                },
                StepConfig {
                    name: "step2".into(),
                    step_type: crate::config::StepType::Shell,
                    run: Some("echo world".into()),
                    depends_on: vec!["step1".into()],
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn test_state_creation() {
        let workflow = create_test_workflow();
        let state = WorkflowState::new(workflow, HashMap::new(), PathBuf::from("."));

        assert!(state.step_results.is_empty());
        assert!(!state.failed);
    }

    #[test]
    fn test_add_result() {
        let workflow = create_test_workflow();
        let mut state = WorkflowState::new(workflow, HashMap::new(), PathBuf::from("."));

        let result = StepResult::success("output".into(), "shell".into(), 100);
        state.add_result("step1", result, false);

        assert!(state.has_result("step1"));
        assert!(!state.has_result("step2"));
    }

    #[test]
    fn test_dependencies_met() {
        let workflow = create_test_workflow();
        let mut state = WorkflowState::new(workflow, HashMap::new(), PathBuf::from("."));

        // step2 depends on step1
        assert!(!state.dependencies_met("step2"));

        // Add step1 result
        let result = StepResult::success("output".into(), "shell".into(), 100);
        state.add_result("step1", result, false);

        assert!(state.dependencies_met("step2"));
    }

    #[test]
    fn test_failure_propagation() {
        let workflow = create_test_workflow();
        let mut state = WorkflowState::new(workflow, HashMap::new(), PathBuf::from("."));

        let result = StepResult::failure("error".into(), 100);
        state.add_result("step1", result, false);

        assert!(state.failed);
    }

    #[test]
    fn test_to_template_context() {
        let workflow = create_test_workflow();
        let mut args = HashMap::new();
        args.insert("issue".into(), "123".into());

        let mut state = WorkflowState::new(workflow, args, PathBuf::from("."));

        let result = StepResult::success("step output".into(), "shell".into(), 100);
        state.add_result("step1", result, false);

        let ctx = state.to_template_context();
        assert!(ctx.args.contains_key("issue"));
        assert!(ctx.steps.contains_key("step1"));
    }

    #[test]
    fn test_workflow_result() {
        let workflow = create_test_workflow();
        let mut state = WorkflowState::new(workflow, HashMap::new(), PathBuf::from("."));

        let result = StepResult::success("output".into(), "shell".into(), 100);
        state.add_result("step1", result, false);

        let workflow_result = WorkflowResult::from_state(&state);

        assert!(workflow_result.success);
        assert_eq!(workflow_result.step_output("step1"), Some("output"));
        assert!(workflow_result.failed_steps().is_empty());
    }
}
