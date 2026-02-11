#![allow(dead_code)]

//! Role and team configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How a role executes across its backends
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoleExecution {
    /// Use first available backend
    #[default]
    First,
    /// Run all backends in parallel, collect results
    Parallel,
    /// Try each backend until one succeeds
    Fallback,
}

/// Configuration for a role (task type)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Default backends for this role
    pub backends: Vec<String>,

    /// Execution mode
    #[serde(default)]
    pub execution: RoleExecution,

    /// Minimum successful backends required (for parallel mode)
    #[serde(default = "default_min_success")]
    pub min_success: u32,
}

fn default_min_success() -> u32 {
    1
}

impl Default for RoleConfig {
    fn default() -> Self {
        Self {
            description: String::new(),
            backends: Vec::new(),
            execution: RoleExecution::First,
            min_success: 1,
        }
    }
}

/// Override backends for a role within a team
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleOverride {
    /// Backends to use for this role in this team
    pub backends: Vec<String>,

    /// Override execution mode
    pub execution: Option<RoleExecution>,
}

/// Configuration for a team (domain-specific settings)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct TeamConfig {
    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// File patterns to detect this team (e.g., ["Cargo.toml"] for Rust)
    #[serde(default)]
    pub detect: Vec<String>,

    /// Command to verify changes (e.g., "cargo clippy && cargo test")
    pub verify: Option<String>,

    /// Role overrides for this team
    #[serde(default)]
    pub roles: HashMap<String, RoleOverride>,
}

impl TeamConfig {
    /// Get the backends for a role, checking team overrides first
    pub fn get_backends_for_role<'a>(
        &'a self,
        role_name: &str,
        default_role: Option<&'a RoleConfig>,
    ) -> Option<&'a [String]> {
        // Check team override first
        if let Some(override_) = self.roles.get(role_name) {
            return Some(&override_.backends);
        }
        // Fall back to default role config
        default_role.map(|r| r.backends.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_config_minimal() {
        let toml = r#"
            backends = ["claude", "codex"]
        "#;
        let config: RoleConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.backends, vec!["claude", "codex"]);
        assert_eq!(config.execution, RoleExecution::First);
    }

    #[test]
    fn test_role_config_parallel() {
        let toml = r#"
            backends = ["claude", "codex", "gemini"]
            execution = "parallel"
            min_success = 2
        "#;
        let config: RoleConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.execution, RoleExecution::Parallel);
        assert_eq!(config.min_success, 2);
    }

    #[test]
    fn test_team_config() {
        let toml = r#"
            description = "Rust development"
            detect = ["Cargo.toml"]
            verify = "cargo clippy && cargo test"

            [roles.analyzer]
            backends = ["codex", "claude"]

            [roles.security]
            backends = ["gemini"]
            execution = "parallel"
        "#;
        let config: TeamConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.detect, vec!["Cargo.toml"]);
        assert_eq!(config.verify, Some("cargo clippy && cargo test".into()));
        assert!(config.roles.contains_key("analyzer"));
        assert!(config.roles.contains_key("security"));
    }

    #[test]
    fn test_team_get_backends() {
        let team: TeamConfig = toml::from_str(
            r#"
            [roles.analyzer]
            backends = ["codex"]
        "#,
        )
        .unwrap();

        let default_role = RoleConfig {
            backends: vec!["claude".into()],
            ..Default::default()
        };

        // Team override takes precedence
        let backends = team.get_backends_for_role("analyzer", Some(&default_role));
        assert_eq!(backends, Some(vec!["codex".into()].as_slice()));

        // Fall back to default for unknown role
        let backends = team.get_backends_for_role("reviewer", Some(&default_role));
        assert_eq!(backends, Some(vec!["claude".into()].as_slice()));
    }
}
