#![allow(dead_code)]

//! Configuration loading with multi-layer merge

use super::{BackendConfig, RoleConfig, TeamConfig, WorkflowConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level llmux configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmuxConfig {
    /// Global defaults
    #[serde(default)]
    pub defaults: Defaults,

    /// Backend definitions
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,

    /// Role definitions
    #[serde(default)]
    pub roles: HashMap<String, RoleConfig>,

    /// Team definitions
    #[serde(default)]
    pub teams: HashMap<String, TeamConfig>,
}

/// Global default settings
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    /// Default timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Run backends in parallel by default
    #[serde(default)]
    pub parallel: bool,

    /// Max concurrent backend requests
    pub max_concurrent: Option<u32>,

    /// Shell command wrapper (for nix-shell, docker, etc.)
    pub command_wrapper: Option<String>,
}

fn default_timeout() -> u64 {
    300
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            parallel: false,
            max_concurrent: None,
            command_wrapper: None,
        }
    }
}

/// Result of executing a step
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct StepResult {
    /// Output for single-backend execution
    pub output: Option<String>,

    /// Outputs for parallel execution (by backend name)
    pub outputs: HashMap<String, String>,

    /// Whether the step failed
    pub failed: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Backend that executed (for single execution)
    pub backend: Option<String>,

    /// Backends that executed (for parallel)
    pub backends: Vec<String>,
}


impl StepResult {
    pub fn success(output: String, backend: String, duration_ms: u64) -> Self {
        Self {
            output: Some(output),
            backend: Some(backend.clone()),
            backends: vec![backend],
            duration_ms,
            ..Default::default()
        }
    }

    pub fn parallel_success(outputs: HashMap<String, String>, duration_ms: u64) -> Self {
        let backends: Vec<_> = outputs.keys().cloned().collect();
        Self {
            outputs,
            backends,
            duration_ms,
            ..Default::default()
        }
    }

    pub fn failure(error: String, duration_ms: u64) -> Self {
        Self {
            failed: true,
            error: Some(error),
            duration_ms,
            ..Default::default()
        }
    }
}

impl LlmuxConfig {
    /// Load configuration from the standard hierarchy
    ///
    /// Load order (later overrides earlier):
    /// 1. Built-in defaults
    /// 2. ~/.config/llmux/config.toml
    /// 3. .llmux/config.toml (project)
    pub fn load(project_dir: Option<&Path>) -> Result<Self> {
        let mut config = Self::default();

        // Load user config
        if let Some(user_config_path) = Self::user_config_path() {
            if user_config_path.exists() {
                let user_config = Self::load_file(&user_config_path)
                    .with_context(|| format!("loading {}", user_config_path.display()))?;
                config.merge(user_config);
            }
        }

        // Load project config
        let project_config_path = project_dir
            .map(|p| p.join(".llmux/config.toml"))
            .unwrap_or_else(|| PathBuf::from(".llmux/config.toml"));

        if project_config_path.exists() {
            let project_config = Self::load_file(&project_config_path)
                .with_context(|| format!("loading {}", project_config_path.display()))?;
            config.merge(project_config);
        }

        Ok(config)
    }

    /// Load configuration from a specific file
    pub fn load_file(path: &Path) -> Result<Self> {
        let contents =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self =
            toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
        Ok(config)
    }

    /// Get the user config path (~/.config/llmux/config.toml)
    pub fn user_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("llmux/config.toml"))
    }

    /// Merge another config into this one (other takes precedence)
    pub fn merge(&mut self, other: Self) {
        // Merge defaults (other wins)
        if other.defaults.timeout != default_timeout() {
            self.defaults.timeout = other.defaults.timeout;
        }
        if other.defaults.parallel {
            self.defaults.parallel = other.defaults.parallel;
        }
        if other.defaults.max_concurrent.is_some() {
            self.defaults.max_concurrent = other.defaults.max_concurrent;
        }
        if other.defaults.command_wrapper.is_some() {
            self.defaults.command_wrapper = other.defaults.command_wrapper;
        }

        // Merge backends (other wins for same key)
        for (name, backend) in other.backends {
            self.backends.insert(name, backend);
        }

        // Merge roles (other wins for same key)
        for (name, role) in other.roles {
            self.roles.insert(name, role);
        }

        // Merge teams (other wins for same key)
        for (name, team) in other.teams {
            self.teams.insert(name, team);
        }
    }

    /// Get a backend by name
    pub fn get_backend(&self, name: &str) -> Option<&BackendConfig> {
        self.backends.get(name)
    }

    /// Get a role by name
    pub fn get_role(&self, name: &str) -> Option<&RoleConfig> {
        self.roles.get(name)
    }

    /// Get a team by name
    pub fn get_team(&self, name: &str) -> Option<&TeamConfig> {
        self.teams.get(name)
    }

    /// Get all enabled backends
    pub fn enabled_backends(&self) -> impl Iterator<Item = (&String, &BackendConfig)> {
        self.backends.iter().filter(|(_, b)| b.enabled)
    }
}

/// Load a workflow from the standard hierarchy
///
/// Search order (first match wins):
/// 1. .llmux/workflows/{name}.toml (project)
/// 2. ~/.config/llmux/workflows/{name}.toml (user)
/// 3. Built-in workflows (embedded)
pub fn load_workflow(name: &str, project_dir: Option<&Path>) -> Result<WorkflowConfig> {
    let filename = format!("{}.toml", name);

    // Check project workflows
    let project_path = project_dir
        .map(|p| p.join(".llmux/workflows").join(&filename))
        .unwrap_or_else(|| PathBuf::from(".llmux/workflows").join(&filename));

    if project_path.exists() {
        return load_workflow_file(&project_path);
    }

    // Check user workflows
    if let Some(user_dir) = dirs::config_dir() {
        let user_path = user_dir.join("llmux/workflows").join(&filename);
        if user_path.exists() {
            return load_workflow_file(&user_path);
        }
    }

    // TODO: Check built-in workflows

    anyhow::bail!("workflow '{}' not found", name)
}

fn load_workflow_file(path: &Path) -> Result<WorkflowConfig> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let workflow: WorkflowConfig =
        toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;

    // Validate the workflow
    workflow.validate().map_err(|errors| {
        anyhow::anyhow!("workflow validation failed:\n  {}", errors.join("\n  "))
    })?;

    Ok(workflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_load_empty_config() {
        let config = LlmuxConfig::default();
        assert!(config.backends.is_empty());
        assert!(config.roles.is_empty());
        assert!(config.teams.is_empty());
    }

    #[test]
    fn test_load_config_file() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
            [defaults]
            timeout = 60

            [backends.claude]
            command = "claude"

            [backends.codex]
            command = "codex"
            args = ["exec", "--json"]
        "#
        )
        .unwrap();

        let config = LlmuxConfig::load_file(&config_path).unwrap();
        assert_eq!(config.defaults.timeout, 60);
        assert!(config.backends.contains_key("claude"));
        assert!(config.backends.contains_key("codex"));
    }

    #[test]
    fn test_config_merge() {
        let mut base = LlmuxConfig::default();
        base.backends.insert(
            "claude".into(),
            BackendConfig {
                command: "claude".into(),
                timeout: 30,
                ..Default::default()
            },
        );

        let mut override_config = LlmuxConfig::default();
        override_config.backends.insert(
            "claude".into(),
            BackendConfig {
                command: "claude-new".into(),
                timeout: 60,
                ..Default::default()
            },
        );
        override_config.backends.insert(
            "codex".into(),
            BackendConfig {
                command: "codex".into(),
                ..Default::default()
            },
        );

        base.merge(override_config);

        // Override wins for existing key
        assert_eq!(base.backends["claude"].command, "claude-new");
        assert_eq!(base.backends["claude"].timeout, 60);

        // New key added
        assert!(base.backends.contains_key("codex"));
    }

    #[test]
    fn test_step_result() {
        let success = StepResult::success("output".into(), "claude".into(), 1000);
        assert!(!success.failed);
        assert_eq!(success.output, Some("output".into()));
        assert_eq!(success.backend, Some("claude".into()));

        let failure = StepResult::failure("timeout".into(), 30000);
        assert!(failure.failed);
        assert_eq!(failure.error, Some("timeout".into()));
    }
}
