//! CLI command implementations

use super::output::{OutputEvent, OutputHandler};
use crate::config::{LlmuxConfig, load_workflow};
use crate::role::detect_team;
use crate::workflow::WorkflowRunner;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Run a workflow
pub async fn run_workflow(
    workflow_name: &str,
    args: Vec<String>,
    working_dir: &Path,
    team_override: Option<&str>,
    config: Arc<LlmuxConfig>,
    handler: &dyn OutputHandler,
) -> Result<i32, String> {
    // Load workflow
    let workflow = load_workflow(workflow_name, Some(working_dir))
        .map_err(|e| format!("Failed to load workflow '{}': {}", workflow_name, e))?;

    // Parse workflow args (simple key=value for now)
    let parsed_args = parse_workflow_args(&args);

    handler.emit(OutputEvent::WorkflowStart {
        name: workflow.name.clone(),
        steps: workflow.steps.len(),
    });

    // Create runner and execute
    let runner = WorkflowRunner::new(config.clone());

    let result = runner
        .run(workflow.clone(), parsed_args, working_dir, team_override)
        .await
        .map_err(|e| format!("Workflow execution failed: {}", e))?;

    // Emit completion event
    handler.emit(OutputEvent::WorkflowComplete {
        success: result.success,
        duration_ms: result.duration.as_millis() as u64,
        steps_completed: result.steps.len(),
    });

    // Output final result
    let final_output = result
        .steps
        .values()
        .filter_map(|s| s.output.as_ref())
        .last()
        .map(|s| s.as_str());

    handler.result(result.success, final_output);

    Ok(if result.success { 0 } else { 1 })
}

/// Parse workflow arguments from CLI
fn parse_workflow_args(args: &[String]) -> HashMap<String, String> {
    let mut parsed = HashMap::new();

    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            parsed.insert(key.to_string(), value.to_string());
        } else {
            // Positional arg - store by index
            parsed.insert(format!("arg{}", parsed.len()), arg.clone());
        }
    }

    parsed
}

/// Validate a workflow
pub fn validate_workflow(
    workflow_name: &str,
    working_dir: Option<&Path>,
    handler: &dyn OutputHandler,
) -> Result<i32, String> {
    match load_workflow(workflow_name, working_dir) {
        Ok(wf) => {
            // Run validation
            match wf.validate() {
                Ok(()) => {
                    handler.emit(OutputEvent::Info {
                        message: format!(
                            "✓ Workflow '{}' is valid ({} steps)",
                            wf.name,
                            wf.steps.len()
                        ),
                    });
                    Ok(0)
                }
                Err(errors) => {
                    handler.emit(OutputEvent::Info {
                        message: format!("✗ Workflow '{}' has {} error(s):", wf.name, errors.len()),
                    });
                    for err in &errors {
                        handler.emit(OutputEvent::Info {
                            message: format!("  - {}", err),
                        });
                    }
                    Ok(1)
                }
            }
        }
        Err(e) => {
            handler.emit(OutputEvent::WorkflowError {
                error: format!("Failed to load workflow: {}", e),
            });
            Ok(1)
        }
    }
}

/// Check backend availability
pub async fn doctor(config: &LlmuxConfig, working_dir: &Path, handler: &dyn OutputHandler) -> i32 {
    handler.emit(OutputEvent::Info {
        message: "Checking backends...".into(),
    });

    let mut all_ok = true;

    for (name, backend) in config.enabled_backends() {
        let status = if backend.is_http() {
            // For HTTP backends, we just report the URL
            format!("✓ {} (http: {})", name, backend.command)
        } else {
            // For CLI backends, check if command exists
            let check = tokio::process::Command::new("which")
                .arg(&backend.command)
                .output()
                .await;

            match check {
                Ok(out) if out.status.success() => {
                    format!("✓ {} (cli: {})", name, backend.command)
                }
                _ => {
                    all_ok = false;
                    format!("✗ {} (cli: {} - not found)", name, backend.command)
                }
            }
        };

        handler.emit(OutputEvent::Info { message: status });
    }

    if config.backends.is_empty() {
        handler.emit(OutputEvent::Info {
            message: "  (no backends configured)".into(),
        });
    }

    // Check team detection
    handler.emit(OutputEvent::Info {
        message: "\nChecking team detection...".into(),
    });

    let detected = detect_team(working_dir, &config.teams, None);
    match detected {
        Some(team) => {
            handler.emit(OutputEvent::Info {
                message: format!("✓ Detected team: {}", team),
            });
        }
        None => {
            handler.emit(OutputEvent::Info {
                message: "  (no team detected)".into(),
            });
        }
    }

    if all_ok { 0 } else { 1 }
}

/// List configured backends
pub fn list_backends(config: &LlmuxConfig, handler: &dyn OutputHandler) {
    if config.backends.is_empty() {
        handler.emit(OutputEvent::Info {
            message: "(no backends configured)".into(),
        });
        return;
    }

    for (name, backend) in &config.backends {
        let enabled = if backend.enabled { "✓" } else { "✗" };
        let kind = if backend.is_http() { "http" } else { "cli" };
        handler.emit(OutputEvent::Info {
            message: format!("{} {} ({}: {})", enabled, name, kind, backend.command),
        });
    }
}

/// List configured teams
pub fn list_teams(config: &LlmuxConfig, handler: &dyn OutputHandler) {
    if config.teams.is_empty() {
        handler.emit(OutputEvent::Info {
            message: "(no teams configured)".into(),
        });
        return;
    }

    for (name, team) in &config.teams {
        handler.emit(OutputEvent::Info {
            message: format!("{}", name),
        });
        if !team.description.is_empty() {
            handler.emit(OutputEvent::Info {
                message: format!("  {}", team.description),
            });
        }
        if !team.detect.is_empty() {
            handler.emit(OutputEvent::Info {
                message: format!("  detect: {:?}", team.detect),
            });
        }
    }
}

/// List configured roles
pub fn list_roles(config: &LlmuxConfig, handler: &dyn OutputHandler) {
    if config.roles.is_empty() {
        handler.emit(OutputEvent::Info {
            message: "(no roles configured)".into(),
        });
        return;
    }

    for (name, role) in &config.roles {
        handler.emit(OutputEvent::Info {
            message: format!("{}", name),
        });
        if !role.description.is_empty() {
            handler.emit(OutputEvent::Info {
                message: format!("  {}", role.description),
            });
        }
        handler.emit(OutputEvent::Info {
            message: format!("  backends: {:?}", role.backends),
        });
        handler.emit(OutputEvent::Info {
            message: format!("  execution: {:?}", role.execution),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct MockHandler {
        events: Arc<Mutex<Vec<OutputEvent>>>,
    }

    impl MockHandler {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn events(&self) -> Vec<OutputEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl OutputHandler for MockHandler {
        fn emit(&self, event: OutputEvent) {
            self.events.lock().unwrap().push(event);
        }
        fn result(&self, _success: bool, _output: Option<&str>) {}
    }

    #[test]
    fn test_parse_workflow_args_key_value() {
        let args = vec!["name=test".to_string(), "count=5".to_string()];
        let parsed = parse_workflow_args(&args);

        assert_eq!(parsed.get("name"), Some(&"test".to_string()));
        assert_eq!(parsed.get("count"), Some(&"5".to_string()));
    }

    #[test]
    fn test_parse_workflow_args_positional() {
        let args = vec!["first".to_string(), "second".to_string()];
        let parsed = parse_workflow_args(&args);

        assert_eq!(parsed.get("arg0"), Some(&"first".to_string()));
        assert_eq!(parsed.get("arg1"), Some(&"second".to_string()));
    }

    #[test]
    fn test_parse_workflow_args_mixed() {
        let args = vec!["positional".to_string(), "key=value".to_string()];
        let parsed = parse_workflow_args(&args);

        assert_eq!(parsed.get("arg0"), Some(&"positional".to_string()));
        assert_eq!(parsed.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_list_backends_empty() {
        let config = LlmuxConfig::default();
        let handler = MockHandler::new();

        list_backends(&config, &handler);

        let events = handler.events();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_list_teams_empty() {
        let config = LlmuxConfig::default();
        let handler = MockHandler::new();

        list_teams(&config, &handler);

        let events = handler.events();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_list_roles_empty() {
        let config = LlmuxConfig::default();
        let handler = MockHandler::new();

        list_roles(&config, &handler);

        let events = handler.events();
        assert_eq!(events.len(), 1);
    }
}
