#![allow(dead_code)]

//! Template context for variable resolution

use crate::config::{RoleConfig, StepResult, TeamConfig};
use minijinja::value::{Object, Value, ValueKind};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Context available to templates during rendering
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    /// Results from completed steps, keyed by step name
    pub steps: HashMap<String, StepResult>,

    /// CLI arguments passed to the workflow
    pub args: HashMap<String, String>,

    /// Current team configuration (if detected)
    pub team: Option<TeamConfig>,

    /// Available roles
    pub roles: HashMap<String, RoleConfig>,

    /// Current item for `for_each` iteration
    pub item: Option<Value>,

    /// Workflow name
    pub workflow: Option<String>,
}

impl TemplateContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a context with workflow arguments
    pub fn with_args(args: HashMap<String, String>) -> Self {
        Self {
            args,
            ..Default::default()
        }
    }

    /// Add a step result to the context
    pub fn add_step(&mut self, name: impl Into<String>, result: StepResult) {
        self.steps.insert(name.into(), result);
    }

    /// Set the current iteration item
    pub fn set_item(&mut self, item: Value) {
        self.item = Some(item);
    }

    /// Clear the iteration item
    pub fn clear_item(&mut self) {
        self.item = None;
    }

    /// Set the team configuration
    pub fn set_team(&mut self, team: TeamConfig) {
        self.team = Some(team);
    }

    /// Set the workflow name
    pub fn set_workflow(&mut self, name: impl Into<String>) {
        self.workflow = Some(name.into());
    }

    /// Convert to a minijinja Value for template rendering
    pub fn to_value(&self) -> Value {
        Value::from_object(ContextObject(self.clone()))
    }

    /// Get list of known top-level variable names for error suggestions
    pub fn known_variables(&self) -> Vec<&str> {
        let mut vars = vec!["steps", "args", "env", "item", "workflow"];
        if self.team.is_some() {
            vars.push("team");
        }
        vars
    }

    /// Get list of known step names for error suggestions
    pub fn known_steps(&self) -> Vec<&str> {
        self.steps.keys().map(|s| s.as_str()).collect()
    }
}

/// Wrapper to implement minijinja::Object for TemplateContext
#[derive(Debug, Clone)]
struct ContextObject(TemplateContext);

impl fmt::Display for ContextObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TemplateContext")
    }
}

impl Object for ContextObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let key_str = key.as_str()?;
        match key_str {
            "steps" => Some(Value::from_object(StepsObject(self.0.steps.clone()))),
            "args" => Some(Value::from_object(ArgsObject(self.0.args.clone()))),
            "team" => self
                .0
                .team
                .as_ref()
                .map(|t| Value::from_object(TeamObject(t.clone()))),
            "item" => self.0.item.clone(),
            "workflow" => self.0.workflow.as_ref().map(|w| Value::from(w.clone())),
            "env" => Some(Value::from_object(EnvObject)),
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        minijinja::value::Enumerator::Str(&["steps", "args", "team", "item", "workflow", "env"])
    }
}

/// Object for accessing step results
#[derive(Debug, Clone)]
struct StepsObject(HashMap<String, StepResult>);

impl fmt::Display for StepsObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "steps")
    }
}

impl Object for StepsObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let step_name = key.as_str()?;
        let result = self.0.get(step_name)?;
        Some(Value::from_object(StepResultObject(result.clone())))
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        minijinja::value::Enumerator::Values(
            self.0.keys().map(|k| Value::from(k.clone())).collect(),
        )
    }
}

/// Object for accessing a single step result
#[derive(Debug, Clone)]
struct StepResultObject(StepResult);

impl fmt::Display for StepResultObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref output) = self.0.output {
            write!(f, "{}", output)
        } else {
            write!(f, "")
        }
    }
}

impl Object for StepResultObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let key_str = key.as_str()?;
        match key_str {
            "output" => self.0.output.as_ref().map(|o| Value::from(o.clone())),
            "outputs" => {
                let map: HashMap<Value, Value> = self
                    .0
                    .outputs
                    .iter()
                    .map(|(k, v)| (Value::from(k.clone()), Value::from(v.clone())))
                    .collect();
                Some(Value::from_iter(map))
            }
            "failed" => Some(Value::from(self.0.failed)),
            "error" => self.0.error.as_ref().map(|e| Value::from(e.clone())),
            "duration_ms" => Some(Value::from(self.0.duration_ms as i64)),
            "backend" => self.0.backend.as_ref().map(|b| Value::from(b.clone())),
            "backends" => Some(Value::from_iter(
                self.0.backends.iter().cloned().map(Value::from),
            )),
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        minijinja::value::Enumerator::Str(&[
            "output",
            "outputs",
            "failed",
            "error",
            "duration_ms",
            "backend",
            "backends",
        ])
    }
}

/// Object for accessing CLI arguments
#[derive(Debug, Clone)]
struct ArgsObject(HashMap<String, String>);

impl fmt::Display for ArgsObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "args")
    }
}

impl Object for ArgsObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let arg_name = key.as_str()?;
        self.0.get(arg_name).map(|v| Value::from(v.clone()))
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        minijinja::value::Enumerator::Values(
            self.0.keys().map(|k| Value::from(k.clone())).collect(),
        )
    }
}

/// Object for accessing team configuration
#[derive(Debug, Clone)]
struct TeamObject(TeamConfig);

impl fmt::Display for TeamObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "team")
    }
}

impl Object for TeamObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let key_str = key.as_str()?;
        match key_str {
            "description" => Some(Value::from(self.0.description.clone())),
            "detect" => Some(Value::from_iter(
                self.0.detect.iter().cloned().map(Value::from),
            )),
            "verify" => self.0.verify.as_ref().map(|v| Value::from(v.clone())),
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        minijinja::value::Enumerator::Str(&["description", "detect", "verify"])
    }
}

/// Object for lazy environment variable access
#[derive(Debug, Clone, Copy)]
struct EnvObject;

impl fmt::Display for EnvObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "env")
    }
}

impl Object for EnvObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let var_name = key.as_str()?;
        std::env::var(var_name).ok().map(Value::from)
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        // Don't enumerate env vars - too many and potentially sensitive
        minijinja::value::Enumerator::Empty
    }
}

/// Helper to convert a Value to a type for conditionals
pub fn value_as_bool(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Bool => value.is_true(),
        ValueKind::Number => {
            if let Some(n) = value.as_i64() {
                n != 0
            } else {
                // For floats, try converting to i64 or check if truthy
                value.is_true()
            }
        }
        ValueKind::String => {
            if let Some(s) = value.as_str() {
                !s.is_empty()
            } else {
                false
            }
        }
        ValueKind::Seq => value.len().unwrap_or(0) > 0,
        ValueKind::Map => value.len().unwrap_or(0) > 0,
        ValueKind::None | ValueKind::Undefined => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let ctx = TemplateContext::new();
        assert!(ctx.steps.is_empty());
        assert!(ctx.args.is_empty());
        assert!(ctx.team.is_none());
    }

    #[test]
    fn test_context_with_args() {
        let mut args = HashMap::new();
        args.insert("issue".into(), "123".into());
        args.insert("dir".into(), ".".into());

        let ctx = TemplateContext::with_args(args);
        assert_eq!(ctx.args.get("issue"), Some(&"123".into()));
    }

    #[test]
    fn test_add_step() {
        let mut ctx = TemplateContext::new();
        let result = StepResult::success("test output".into(), "claude".into(), 1000);
        ctx.add_step("fetch", result);

        assert!(ctx.steps.contains_key("fetch"));
        assert_eq!(ctx.steps["fetch"].output, Some("test output".into()));
    }

    #[test]
    fn test_item_iteration() {
        let mut ctx = TemplateContext::new();
        assert!(ctx.item.is_none());

        ctx.set_item(Value::from("item1"));
        assert!(ctx.item.is_some());

        ctx.clear_item();
        assert!(ctx.item.is_none());
    }

    #[test]
    fn test_known_variables() {
        let mut ctx = TemplateContext::new();
        let vars = ctx.known_variables();
        assert!(vars.contains(&"steps"));
        assert!(vars.contains(&"args"));
        assert!(!vars.contains(&"team")); // No team set

        ctx.set_team(TeamConfig::default());
        let vars = ctx.known_variables();
        assert!(vars.contains(&"team"));
    }

    #[test]
    fn test_value_as_bool() {
        assert!(value_as_bool(&Value::from(true)));
        assert!(!value_as_bool(&Value::from(false)));
        assert!(value_as_bool(&Value::from(1)));
        assert!(!value_as_bool(&Value::from(0)));
        assert!(value_as_bool(&Value::from("hello")));
        assert!(!value_as_bool(&Value::from("")));
        assert!(!value_as_bool(&Value::UNDEFINED));
    }
}
