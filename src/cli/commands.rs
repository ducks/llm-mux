//! CLI command implementations

use super::output::{OutputEvent, OutputHandler};
use crate::config::{LlmuxConfig, load_workflow};
use crate::role::detect_team;
use crate::workflow::WorkflowRunner;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Project type detection and configuration
#[derive(Debug, Clone, PartialEq)]
struct ProjectType {
    name: &'static str,
    display_name: &'static str,
    extensions: &'static [&'static str],
    roles: &'static [(&'static str, &'static str)], // (role_name, description)
}

impl ProjectType {
    const RUBY: ProjectType = ProjectType {
        name: "ruby",
        display_name: "Ruby/Rails",
        extensions: &[".rb"],
        roles: &[
            ("ruby_n1", "N+1 query detection"),
            ("ruby_security", "Security vulnerability analysis"),
            ("ruby_perf", "Performance optimization"),
        ],
    };

    const RUST: ProjectType = ProjectType {
        name: "rust",
        display_name: "Rust",
        extensions: &[".rs"],
        roles: &[
            ("rust_safety", "Memory safety analysis"),
            ("rust_perf", "Performance analysis"),
            ("rust_idioms", "Idiomatic patterns"),
        ],
    };

    const JAVASCRIPT: ProjectType = ProjectType {
        name: "javascript",
        display_name: "JavaScript/TypeScript",
        extensions: &[".js", ".ts", ".jsx", ".tsx"],
        roles: &[
            ("js_lint", "Code quality and linting"),
            ("js_security", "Security analysis"),
        ],
    };

    const GO: ProjectType = ProjectType {
        name: "go",
        display_name: "Go",
        extensions: &[".go"],
        roles: &[
            ("go_idioms", "Idiomatic Go patterns"),
            ("go_concurrency", "Concurrency and goroutine analysis"),
            ("go_performance", "Performance optimization"),
        ],
    };

    const PYTHON: ProjectType = ProjectType {
        name: "python",
        display_name: "Python",
        extensions: &[".py"],
        roles: &[
            ("python_types", "Type hints and mypy analysis"),
            ("python_performance", "Performance optimization"),
            ("python_idioms", "Pythonic patterns"),
        ],
    };

    const ALL: &'static [ProjectType] = &[
        Self::RUBY,
        Self::RUST,
        Self::JAVASCRIPT,
        Self::GO,
        Self::PYTHON,
    ];

    /// Count files matching this project type in a directory
    fn count_files(&self, dir: &Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(file_name) = entry.file_name().into_string() {
                    for ext in self.extensions {
                        if file_name.ends_with(ext) {
                            count += 1;
                            break;
                        }
                    }
                }
            }
        }
        count
    }

    /// Detect project type by counting files
    fn detect(dir: &Path) -> Option<&'static ProjectType> {
        let mut best_match: Option<(&ProjectType, usize)> = None;

        for project_type in Self::ALL {
            let count = project_type.count_files(dir);
            if count > 0 {
                best_match = match best_match {
                    None => Some((project_type, count)),
                    Some((_, best_count)) if count > best_count => Some((project_type, count)),
                    Some(existing) => Some(existing),
                };
            }
        }

        best_match.map(|(pt, _)| pt)
    }
}

/// Run a workflow
pub async fn run_workflow(
    workflow_name: &str,
    args: Vec<String>,
    working_dir: &Path,
    team_override: Option<&str>,
    config: Arc<LlmuxConfig>,
    handler: &dyn OutputHandler,
    output_file: Option<&Path>,
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

    // Write to file if specified
    if let Some(path) = output_file {
        if let Some(output) = final_output {
            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory {}: {}", parent.display(), e))?;
            }
            std::fs::write(path, output)
                .map_err(|e| format!("Failed to write output to {}: {}", path.display(), e))?;
        }
    }

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
            message: name.to_string(),
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
            message: name.to_string(),
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

/// List configured ecosystems
pub fn list_ecosystems(config: &LlmuxConfig, handler: &dyn OutputHandler) {
    if config.ecosystems.is_empty() {
        handler.emit(OutputEvent::Info {
            message: "(no ecosystems configured)".into(),
        });
        return;
    }

    for (name, ecosystem) in &config.ecosystems {
        handler.emit(OutputEvent::Info {
            message: name.to_string(),
        });
        if !ecosystem.description.is_empty() {
            handler.emit(OutputEvent::Info {
                message: format!("  {}", ecosystem.description),
            });
        }
        if !ecosystem.projects.is_empty() {
            handler.emit(OutputEvent::Info {
                message: format!("  projects: {} configured", ecosystem.projects.len()),
            });
            for (project_name, project) in &ecosystem.projects {
                handler.emit(OutputEvent::Info {
                    message: format!("    {} -> {}", project_name, project.path.display()),
                });
                if !project.depends_on.is_empty() {
                    handler.emit(OutputEvent::Info {
                        message: format!("      depends_on: {:?}", project.depends_on),
                    });
                }
            }
        }
        if !ecosystem.knowledge.is_empty() {
            handler.emit(OutputEvent::Info {
                message: format!("  knowledge: {} facts", ecosystem.knowledge.len()),
            });
        }
    }
}

/// Initialize llmux configuration interactively
pub async fn init_config(
    working_dir: &Path,
    global: bool,
    project: bool,
    no_detect: bool,
    force: bool,
    handler: &dyn OutputHandler,
) -> Result<i32, String> {
    use std::fs;

    // Determine scope (default to asking user, but flags override)
    let is_global = if global {
        true
    } else if project {
        false
    } else {
        // Interactive prompt
        handler.emit(OutputEvent::Info {
            message: "Initialize configuration:".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  1. Global (~/.config/llm-mux/config.toml) - backends for all projects".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  2. Project (.llm-mux/config.toml) - roles/teams for this project".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "\nFor first-time setup, choose Global. Run with --global or --project to skip this prompt.\n".into(),
        });
        return Ok(0);
    };

    let config_path = if is_global {
        dirs::config_dir()
            .ok_or_else(|| "Could not determine config directory".to_string())?
            .join("llm-mux")
            .join("config.toml")
    } else {
        working_dir.join(".llm-mux").join("config.toml")
    };

    // Check if config already exists
    if config_path.exists() && !force {
        handler.emit(OutputEvent::Info {
            message: format!("Config already exists at {}", config_path.display()),
        });
        handler.emit(OutputEvent::Info {
            message: "Use --force to overwrite".into(),
        });
        return Ok(1);
    }

    let scope_name = if is_global { "global" } else { "project" };
    handler.emit(OutputEvent::Info {
        message: format!("=== llm-mux {} configuration setup ===\n", scope_name),
    });

    // Detect available backends
    handler.emit(OutputEvent::Info {
        message: "Detecting available LLM backends...".into(),
    });

    let mut detected_backends = Vec::new();

    // Check for claude
    if let Ok(output) = tokio::process::Command::new("which")
        .arg("claude")
        .output()
        .await
    {
        if output.status.success() {
            detected_backends.push("claude");
            handler.emit(OutputEvent::Info {
                message: "  ✓ claude".into(),
            });
        }
    }

    // Check for codex
    if let Ok(output) = tokio::process::Command::new("which")
        .arg("codex")
        .output()
        .await
    {
        if output.status.success() {
            detected_backends.push("codex");
            handler.emit(OutputEvent::Info {
                message: "  ✓ codex".into(),
            });
        }
    }

    // Check for gemini-cli (npx)
    if let Ok(output) = tokio::process::Command::new("npx")
        .arg("--yes")
        .arg("@google/gemini-cli")
        .arg("--version")
        .output()
        .await
    {
        if output.status.success() {
            detected_backends.push("gemini");
            handler.emit(OutputEvent::Info {
                message: "  ✓ gemini".into(),
            });
        }
    }

    // Check for ollama
    if let Ok(output) = tokio::process::Command::new("curl")
        .arg("-s")
        .arg("http://localhost:11434/api/tags")
        .output()
        .await
    {
        if output.status.success() {
            detected_backends.push("ollama");
            handler.emit(OutputEvent::Info {
                message: "  ✓ ollama".into(),
            });
        }
    }

    if detected_backends.is_empty() {
        handler.emit(OutputEvent::Info {
            message: "\n  No LLM backends detected. Install at least one:".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "    - claude: https://claude.ai/download".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "    - codex: https://github.com/openai/codex".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "    - ollama: https://ollama.ai".into(),
        });
        return Ok(1);
    }

    // Detect project type (only for project init)
    let project_type = if !is_global && !no_detect {
        handler.emit(OutputEvent::Info {
            message: "\nDetecting project type...".into(),
        });

        if let Some(detected) = ProjectType::detect(working_dir) {
            handler.emit(OutputEvent::Info {
                message: format!("  Detected: {} project", detected.display_name),
            });
            Some(detected)
        } else {
            None
        }
    } else {
        None
    };

    // Generate config
    handler.emit(OutputEvent::Info {
        message: "\nGenerating configuration...".into(),
    });

    let mut config_content = String::new();

    // Add backends
    config_content.push_str("# Backends\n");
    for backend in &detected_backends {
        match *backend {
            "claude" => {
                config_content.push_str("[backends.claude]\n");
                config_content.push_str("enabled = true\n");
                config_content.push_str("command = \"claude\"\n");
                config_content.push_str("args = [\"-p\", \"--\"]\n");
                config_content.push_str("timeout = 600\n\n");
            }
            "codex" => {
                config_content.push_str("[backends.codex]\n");
                config_content.push_str("enabled = true\n");
                config_content.push_str("command = \"codex\"\n");
                config_content.push_str("args = [\"exec\", \"--json\", \"-s\", \"read-only\"]\n\n");
            }
            "gemini" => {
                config_content.push_str("[backends.gemini]\n");
                config_content.push_str("enabled = true\n");
                config_content.push_str("command = \"npx\"\n");
                config_content.push_str("args = [\"@google/gemini-cli\", \"-m\", \"gemini-2.0-flash\"]\n");
                config_content.push_str("timeout = 300\n\n");
            }
            "ollama" => {
                config_content.push_str("[backends.ollama]\n");
                config_content.push_str("enabled = true\n");
                config_content.push_str("command = \"http://localhost:11434\"\n");
                config_content.push_str("model = \"qwen3-coder-next\"\n\n");
            }
            _ => {}
        }
    }

    // Add roles (global gets basic default, project gets detected roles)
    if is_global {
        config_content.push_str("# Basic roles\n");
        config_content.push_str("[roles.default]\n");
        config_content.push_str("description = \"Default role for general queries\"\n");
        config_content.push_str(&format!("backends = [\"{}\"]\n", detected_backends[0]));
        config_content.push_str("execution = \"first\"\n\n");
    } else if let Some(ptype) = project_type {
        // Project-specific roles
        config_content.push_str(&format!("# {} team roles\n", ptype.display_name));
        for (role_name, description) in ptype.roles {
            config_content.push_str(&format!("[roles.{}]\n", role_name));
            config_content.push_str(&format!("description = \"{}\"\n", description));
            config_content.push_str(&format!("backends = [\"{}\"]\n", detected_backends[0]));
            config_content.push_str("execution = \"first\"\n\n");
        }
    }

    // Create config directory if needed
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    // Write config
    fs::write(&config_path, config_content)
        .map_err(|e| format!("Failed to write config: {}", e))?;

    handler.emit(OutputEvent::Info {
        message: format!("\n✓ Configuration written to {}", config_path.display()),
    });

    if is_global {
        handler.emit(OutputEvent::Info {
            message: "\nNext steps:".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  1. Run 'llm-mux doctor' to test backends".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  2. Run 'llm-mux init --project' in your project directories".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  3. Create workflows in ~/.config/llm-mux/workflows/".into(),
        });
    } else {
        handler.emit(OutputEvent::Info {
            message: "\nNext steps:".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  1. Review and customize project-specific roles".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  2. Create workflows in .llm-mux/workflows/".into(),
        });
        handler.emit(OutputEvent::Info {
            message: "  3. Add .llm-mux/ to your .gitignore if needed".into(),
        });
    }

    Ok(0)
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
