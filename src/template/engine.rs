#![allow(dead_code)]

//! Template engine for rendering prompts and commands

use super::context::TemplateContext;
use super::errors::TemplateError;
use super::filters;
use minijinja::Environment;

/// Template rendering engine
///
/// Wraps minijinja with custom filters and strict undefined handling.
pub struct TemplateEngine {
    env: Environment<'static>,
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateEngine {
    /// Create a new template engine with default configuration
    pub fn new() -> Self {
        let mut env = Environment::new();

        // Configure strict undefined handling
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

        // Register custom filters
        filters::register_filters(&mut env);

        Self { env }
    }

    /// Render a template string with the given context
    ///
    /// # Example
    ///
    /// ```ignore
    /// let engine = TemplateEngine::new();
    /// let mut ctx = TemplateContext::new();
    /// ctx.args.insert("issue".into(), "123".into());
    ///
    /// let result = engine.render("Fixing issue {{ args.issue }}", &ctx)?;
    /// assert_eq!(result, "Fixing issue 123");
    /// ```
    pub fn render(&self, template: &str, ctx: &TemplateContext) -> Result<String, TemplateError> {
        // Add the template to the environment
        let mut env = self.env.clone();
        env.add_template("__render__", template)
            .map_err(|e| TemplateError::syntax(e.to_string(), e.line().unwrap_or(0), 0))?;

        let tmpl = env
            .get_template("__render__")
            .map_err(TemplateError::Internal)?;

        tmpl.render(ctx.to_value())
            .map_err(|e| convert_minijinja_error(e, ctx))
    }

    /// Render a template and return the result trimmed
    pub fn render_trimmed(
        &self,
        template: &str,
        ctx: &TemplateContext,
    ) -> Result<String, TemplateError> {
        self.render(template, ctx).map(|s| s.trim().to_string())
    }

    /// Check if a template is syntactically valid
    pub fn validate(&self, template: &str) -> Result<(), TemplateError> {
        let mut env = self.env.clone();
        env.add_template("__validate__", template)
            .map_err(|e| TemplateError::syntax(e.to_string(), e.line().unwrap_or(0), 0))?;
        Ok(())
    }
}

/// Convert a minijinja error to our TemplateError type
fn convert_minijinja_error(err: minijinja::Error, ctx: &TemplateContext) -> TemplateError {
    let msg = err.to_string();
    let line = err.line().unwrap_or(0);

    // Check for undefined variable errors
    if msg.contains("undefined") {
        // Try to extract the variable name
        let var_name = extract_var_from_error(&msg);
        return TemplateError::undefined_variable_at(var_name, line, 0, &ctx.known_variables());
    }

    // Check for type errors
    if msg.contains("not iterable") || msg.contains("cannot be iterated") {
        return TemplateError::type_mismatch("iterable", "non-iterable value");
    }

    // Check for filter errors
    if msg.contains("filter") {
        return TemplateError::filter("unknown", msg);
    }

    // Generic fallback
    TemplateError::syntax(msg, line, 0)
}

/// Extract variable name from minijinja error message
fn extract_var_from_error(msg: &str) -> String {
    // Messages look like: "undefined value (in <string>:1): variable is `steps.foo`"
    if let Some(start) = msg.find('`') {
        if let Some(end) = msg[start + 1..].find('`') {
            return msg[start + 1..start + 1 + end].to_string();
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepResult;
    use std::collections::HashMap;

    #[test]
    fn test_simple_render() {
        let engine = TemplateEngine::new();
        let ctx = TemplateContext::new();
        let result = engine.render("Hello, world!", &ctx).unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_render_with_args() {
        let engine = TemplateEngine::new();
        let mut args = HashMap::new();
        args.insert("name".into(), "Alice".into());
        let ctx = TemplateContext::with_args(args);

        let result = engine.render("Hello, {{ args.name }}!", &ctx).unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn test_render_with_step_output() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        let result = StepResult::success("step output".into(), "claude".into(), 1000);
        ctx.add_step("fetch", result);

        let rendered = engine
            .render("Output: {{ steps.fetch.output }}", &ctx)
            .unwrap();
        assert_eq!(rendered, "Output: step output");
    }

    #[test]
    fn test_render_with_filters() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        ctx.args.insert("files".into(), "a.rs,b.rs,c.rs".into());

        // Use json filter
        let result = engine.render("{{ args.files | json }}", &ctx).unwrap();
        assert!(result.contains("a.rs"));
    }

    #[test]
    fn test_render_trimmed() {
        let engine = TemplateEngine::new();
        let ctx = TemplateContext::new();
        let result = engine.render_trimmed("  hello  ", &ctx).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_undefined_variable_error() {
        let engine = TemplateEngine::new();
        let ctx = TemplateContext::new();

        let result = engine.render("{{ args.nonexistent }}", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TemplateError::UndefinedVariable { .. }));
    }

    #[test]
    fn test_syntax_error() {
        let engine = TemplateEngine::new();
        let ctx = TemplateContext::new();

        let result = engine.render("{{ invalid syntax {{", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_good_template() {
        let engine = TemplateEngine::new();
        assert!(engine.validate("Hello {{ args.name }}").is_ok());
    }

    #[test]
    fn test_validate_bad_template() {
        let engine = TemplateEngine::new();
        assert!(engine.validate("Hello {{ args.name }").is_err());
    }

    #[test]
    fn test_item_in_for_each() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        ctx.set_item(minijinja::value::Value::from("test_item"));

        let result = engine.render("Item: {{ item }}", &ctx).unwrap();
        assert_eq!(result, "Item: test_item");
    }

    #[test]
    fn test_env_access() {
        let engine = TemplateEngine::new();
        let ctx = TemplateContext::new();

        // Use HOME which should always exist
        let result = engine.render("{{ env.HOME }}", &ctx).unwrap();
        assert!(!result.is_empty());
        assert!(result.starts_with('/'));
    }

    #[test]
    fn test_conditional_in_template() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        ctx.args.insert("debug".into(), "true".into());

        let template = "{% if args.debug == 'true' %}Debug mode{% endif %}";
        let result = engine.render(template, &ctx).unwrap();
        assert_eq!(result, "Debug mode");
    }

    #[test]
    fn test_for_loop_in_template() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        let mut result = StepResult::default();
        result.backends = vec!["claude".into(), "codex".into()];
        ctx.add_step("query", result);

        let template = "{% for b in steps.query.backends %}{{ b }},{% endfor %}";
        let rendered = engine.render(template, &ctx).unwrap();
        assert_eq!(rendered, "claude,codex,");
    }

    #[test]
    fn test_shell_escape_filter() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        ctx.args.insert("input".into(), "hello world".into());

        let result = engine
            .render("{{ args.input | shell_escape }}", &ctx)
            .unwrap();
        assert_eq!(result, "'hello world'");
    }

    #[test]
    fn test_nested_step_access() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        let mut result = StepResult::default();
        result
            .outputs
            .insert("claude".into(), "Claude output".into());
        result.outputs.insert("codex".into(), "Codex output".into());
        ctx.add_step("parallel", result);

        let rendered = engine
            .render("{{ steps.parallel.outputs.claude }}", &ctx)
            .unwrap();
        assert_eq!(rendered, "Claude output");
    }
}
