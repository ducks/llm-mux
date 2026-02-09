#![allow(dead_code)]

//! Expression evaluation for step conditionals
//!
//! Evaluates expressions like `steps.fix.action == 'fix'` against a template context.

use super::context::TemplateContext;
use super::errors::TemplateError;
use minijinja::value::Value;

/// Evaluate a conditional expression against a context
///
/// Supports:
/// - Variable access: `steps.name.output`, `args.issue`
/// - Equality: `==`, `!=`
/// - Boolean: `and`, `or`, `not`
/// - Literals: `'string'`, `"string"`, `true`, `false`
/// - Parentheses: `(expr)`
pub fn evaluate_condition(expr: &str, ctx: &TemplateContext) -> Result<bool, TemplateError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok(true); // Empty condition is always true
    }

    // Use minijinja's expression evaluation by wrapping in an if statement
    let mut env = minijinja::Environment::new();
    super::filters::register_filters(&mut env);

    // Wrap the expression in a template that outputs true/false
    let template_str = format!("{{% if {expr} %}}true{{% else %}}false{{% endif %}}");

    env.add_template("expr", &template_str)
        .map_err(|e| TemplateError::expression(format!("invalid expression syntax: {}", e)))?;

    let template = env
        .get_template("expr")
        .map_err(|e| TemplateError::expression(format!("failed to get template: {}", e)))?;

    let result = template.render(ctx.to_value()).map_err(|e| {
        // Try to provide helpful error messages
        let msg = e.to_string();
        if msg.contains("undefined") {
            let var_name = extract_undefined_var(&msg);
            TemplateError::undefined_variable(var_name, &ctx.known_variables())
        } else {
            TemplateError::expression(msg)
        }
    })?;

    Ok(result == "true")
}

/// Evaluate an expression and return the resulting Value
///
/// Useful for accessing nested fields like `steps.analyze.outputs.claude`
pub fn evaluate_expression(expr: &str, ctx: &TemplateContext) -> Result<Value, TemplateError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok(Value::UNDEFINED);
    }

    let mut env = minijinja::Environment::new();
    super::filters::register_filters(&mut env);

    // Render the expression directly
    let template_str = format!("{{{{ {expr} }}}}");

    env.add_template("expr", &template_str)
        .map_err(|e| TemplateError::expression(format!("invalid expression: {}", e)))?;

    let template = env
        .get_template("expr")
        .map_err(|e| TemplateError::expression(format!("failed to get template: {}", e)))?;

    let result = template
        .render(ctx.to_value())
        .map_err(|e| TemplateError::expression(e.to_string()))?;

    // Parse the result back to a Value
    // For simple cases this works, but complex objects become strings
    if result == "true" {
        Ok(Value::from(true))
    } else if result == "false" {
        Ok(Value::from(false))
    } else if let Ok(n) = result.parse::<i64>() {
        Ok(Value::from(n))
    } else if let Ok(n) = result.parse::<f64>() {
        Ok(Value::from(n))
    } else if result.is_empty() || result == "none" {
        Ok(Value::UNDEFINED)
    } else {
        Ok(Value::from(result))
    }
}

/// Extract variable name from an undefined error message
fn extract_undefined_var(msg: &str) -> String {
    // Minijinja error messages look like "undefined value: steps.foo"
    if let Some(pos) = msg.find("undefined") {
        let after = &msg[pos..];
        // Extract the variable path
        if let Some(colon) = after.find(':') {
            let var_part = after[colon + 1..].trim();
            // Take until whitespace or end
            return var_part
                .split_whitespace()
                .next()
                .unwrap_or("unknown")
                .to_string();
        }
    }
    "unknown".to_string()
}

/// Check if a step should be executed based on its condition
///
/// Returns true if:
/// - No condition specified
/// - Condition evaluates to true
pub fn should_execute_step(
    condition: Option<&str>,
    ctx: &TemplateContext,
) -> Result<bool, TemplateError> {
    match condition {
        None => Ok(true),
        Some(cond) => evaluate_condition(cond, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepResult;

    fn ctx_with_step() -> TemplateContext {
        let mut ctx = TemplateContext::new();
        let mut result = StepResult::success("fix applied".into(), "claude".into(), 1000);
        // Add some extra fields for testing
        result.outputs.insert("action".into(), "fix".into());
        ctx.add_step("analyze", result);
        ctx.args.insert("issue".into(), "123".into());
        ctx
    }

    #[test]
    fn test_empty_condition() {
        let ctx = TemplateContext::new();
        assert!(evaluate_condition("", &ctx).unwrap());
        assert!(evaluate_condition("  ", &ctx).unwrap());
    }

    #[test]
    fn test_true_literal() {
        let ctx = TemplateContext::new();
        assert!(evaluate_condition("true", &ctx).unwrap());
    }

    #[test]
    fn test_false_literal() {
        let ctx = TemplateContext::new();
        assert!(!evaluate_condition("false", &ctx).unwrap());
    }

    #[test]
    fn test_string_equality() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("args.issue == '123'", &ctx).unwrap());
        assert!(!evaluate_condition("args.issue == '456'", &ctx).unwrap());
    }

    #[test]
    fn test_string_inequality() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("args.issue != '456'", &ctx).unwrap());
        assert!(!evaluate_condition("args.issue != '123'", &ctx).unwrap());
    }

    #[test]
    fn test_step_output_access() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("steps.analyze.output", &ctx).unwrap());
        assert!(evaluate_condition("steps.analyze.failed == false", &ctx).unwrap());
    }

    #[test]
    fn test_boolean_and() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("true and true", &ctx).unwrap());
        assert!(!evaluate_condition("true and false", &ctx).unwrap());
        assert!(evaluate_condition("args.issue == '123' and steps.analyze.output", &ctx).unwrap());
    }

    #[test]
    fn test_boolean_or() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("true or false", &ctx).unwrap());
        assert!(!evaluate_condition("false or false", &ctx).unwrap());
    }

    #[test]
    fn test_boolean_not() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("not false", &ctx).unwrap());
        assert!(!evaluate_condition("not true", &ctx).unwrap());
        assert!(!evaluate_condition("not steps.analyze.output", &ctx).unwrap());
    }

    #[test]
    fn test_parentheses() {
        let ctx = ctx_with_step();
        assert!(evaluate_condition("(true and true) or false", &ctx).unwrap());
        assert!(!evaluate_condition("true and (true and false)", &ctx).unwrap());
    }

    #[test]
    fn test_undefined_variable_error() {
        let ctx = TemplateContext::new();
        let result = evaluate_condition("steps.nonexistent.output", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_should_execute_step_no_condition() {
        let ctx = TemplateContext::new();
        assert!(should_execute_step(None, &ctx).unwrap());
    }

    #[test]
    fn test_should_execute_step_true_condition() {
        let ctx = ctx_with_step();
        assert!(should_execute_step(Some("true"), &ctx).unwrap());
        assert!(should_execute_step(Some("args.issue == '123'"), &ctx).unwrap());
    }

    #[test]
    fn test_should_execute_step_false_condition() {
        let ctx = ctx_with_step();
        assert!(!should_execute_step(Some("false"), &ctx).unwrap());
        assert!(!should_execute_step(Some("args.issue == '999'"), &ctx).unwrap());
    }

    #[test]
    fn test_evaluate_expression_string() {
        let ctx = ctx_with_step();
        let result = evaluate_expression("args.issue", &ctx).unwrap();
        assert_eq!(result.to_string(), "123");
    }

    #[test]
    fn test_evaluate_expression_bool() {
        let ctx = ctx_with_step();
        let result = evaluate_expression("steps.analyze.failed", &ctx).unwrap();
        assert!(!result.is_true());
    }
}
