//! Template engine for llmux
//!
//! Provides Jinja2-compatible templating for prompts, commands, and expressions.
//!
//! # Features
//!
//! - Variable substitution: `{{ steps.fetch.output }}`, `{{ args.issue }}`
//! - Filters: `shell_escape`, `json`, `join`, `first`, `last`, `trim`, `default`
//! - Conditionals: `{% if condition %}...{% endif %}`
//! - Loops: `{% for item in items %}...{% endfor %}`
//! - Expression evaluation for step conditions
//!
//! # Example
//!
//! ```ignore
//! use llmux::template::{TemplateEngine, TemplateContext};
//!
//! let engine = TemplateEngine::new();
//! let mut ctx = TemplateContext::new();
//! ctx.args.insert("issue".into(), "123".into());
//!
//! let prompt = engine.render(
//!     "Fix issue #{{ args.issue }} by analyzing the code.",
//!     &ctx
//! )?;
//! ```

mod conditionals;
mod context;
mod engine;
mod errors;
mod filters;

#[allow(unused_imports)]
pub use conditionals::{evaluate_condition, evaluate_expression, should_execute_step};
pub use context::TemplateContext;
pub use engine::TemplateEngine;
pub use errors::TemplateError;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepResult;

    #[test]
    fn test_full_workflow() {
        // Simulate a workflow with multiple steps
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();

        // Add workflow args
        ctx.args.insert("issue".into(), "123".into());
        ctx.args.insert("dir".into(), ".".into());

        // Simulate first step completing
        let fetch_result = StepResult::success(
            r#"{"title": "Bug fix", "body": "Fix the thing"}"#.into(),
            "gh".into(),
            500,
        );
        ctx.add_step("fetch", fetch_result);

        // Render prompt for second step
        let prompt = engine
            .render(
                "Based on issue #{{ args.issue }}:\n{{ steps.fetch.output }}\n\nPropose a fix.",
                &ctx,
            )
            .unwrap();

        assert!(prompt.contains("issue #123"));
        assert!(prompt.contains("Bug fix"));

        // Simulate second step completing
        let analyze_result =
            StepResult::success("Apply fix to src/main.rs".into(), "claude".into(), 5000);
        ctx.add_step("analyze", analyze_result);

        // Check condition for third step
        let should_run = should_execute_step(Some("steps.analyze.failed == false"), &ctx).unwrap();
        assert!(should_run);

        // Check condition that should skip
        let should_skip = should_execute_step(Some("steps.analyze.failed == true"), &ctx).unwrap();
        assert!(!should_skip);
    }

    #[test]
    fn test_for_each_iteration() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();

        // Simulate step that outputs files to process
        let list_result = StepResult::success(
            "src/main.rs\nsrc/lib.rs\nsrc/config.rs".into(),
            "find".into(),
            100,
        );
        ctx.add_step("list_files", list_result);

        // For each iteration, set the item
        let files = vec!["src/main.rs", "src/lib.rs", "src/config.rs"];
        for file in files {
            ctx.set_item(minijinja::value::Value::from(file));
            let cmd = engine.render("cat {{ item }}", &ctx).unwrap();
            assert!(cmd.starts_with("cat src/"));
        }
        ctx.clear_item();
    }

    #[test]
    fn test_error_suggestions() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        ctx.add_step("analyze", StepResult::default());

        // Typo in step name
        let result = engine.render("{{ steps.anaylze.output }}", &ctx);
        assert!(result.is_err());
        // The error should suggest "analyze"
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("anaylze") || err_msg.contains("undefined"));
    }

    #[test]
    fn test_shell_safety() {
        let engine = TemplateEngine::new();
        let mut ctx = TemplateContext::new();
        ctx.args.insert("user_input".into(), "$(rm -rf /)".into());

        // Without shell_escape - dangerous!
        let dangerous = engine.render("echo {{ args.user_input }}", &ctx).unwrap();
        assert!(dangerous.contains("$(rm"));

        // With shell_escape - safe
        let safe = engine
            .render("echo {{ args.user_input | shell_escape }}", &ctx)
            .unwrap();
        assert!(safe.contains("'$(rm"));
        assert!(safe.contains(")'"));
    }
}
