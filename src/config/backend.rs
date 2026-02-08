//! Backend configuration for LLM providers

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a single LLM backend
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    /// Command to execute (or HTTP URL for API backends)
    pub command: String,

    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,

    /// Whether this backend is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Timeout in seconds for requests
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Model name (for API backends like Ollama)
    pub model: Option<String>,

    /// Maximum retry attempts for transient failures
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Base delay in milliseconds for exponential backoff
    #[serde(default = "default_retry_delay")]
    pub retry_delay_ms: u64,

    /// Whether to auto-retry on rate limits
    #[serde(default = "default_true")]
    pub retry_rate_limit: bool,

    /// Whether to auto-retry on timeouts
    #[serde(default)]
    pub retry_timeout: bool,

    /// Additional environment variables for the command
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_enabled() -> bool {
    true
}

fn default_timeout() -> u64 {
    300 // 5 minutes
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_delay() -> u64 {
    1000 // 1 second
}

fn default_true() -> bool {
    true
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            enabled: true,
            timeout: default_timeout(),
            model: None,
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay(),
            retry_rate_limit: true,
            retry_timeout: false,
            env: HashMap::new(),
        }
    }
}

impl BackendConfig {
    /// Returns true if this is an HTTP API backend (URL starts with http)
    pub fn is_http(&self) -> bool {
        self.command.starts_with("http://") || self.command.starts_with("https://")
    }

    /// Returns true if this is a CLI backend
    pub fn is_cli(&self) -> bool {
        !self.is_http()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_minimal() {
        let toml = r#"
            command = "claude"
        "#;
        let config: BackendConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.command, "claude");
        assert!(config.enabled);
        assert_eq!(config.timeout, 300);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_deserialize_full() {
        let toml = r#"
            command = "codex"
            args = ["exec", "--json"]
            enabled = true
            timeout = 60
            max_retries = 5
            retry_delay_ms = 2000
        "#;
        let config: BackendConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.command, "codex");
        assert_eq!(config.args, vec!["exec", "--json"]);
        assert_eq!(config.timeout, 60);
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn test_deserialize_http_backend() {
        let toml = r#"
            command = "http://localhost:11434"
            model = "qwen3-coder"
        "#;
        let config: BackendConfig = toml::from_str(toml).unwrap();
        assert!(config.is_http());
        assert!(!config.is_cli());
        assert_eq!(config.model, Some("qwen3-coder".into()));
    }

    #[test]
    fn test_reject_unknown_fields() {
        let toml = r#"
            command = "claude"
            unknown_field = "value"
        "#;
        let result: Result<BackendConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }
}
