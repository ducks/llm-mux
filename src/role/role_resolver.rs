#![allow(dead_code)]

//! Resolve role names to backend lists using team context

use crate::config::{LlmuxConfig, RoleExecution};
use thiserror::Error;

/// Errors that can occur during role resolution
#[derive(Debug, Error)]
pub enum RoleError {
    #[error("role '{role}' is not defined")]
    RoleNotFound { role: String },

    #[error("team '{team}' is not defined")]
    TeamNotFound { team: String },

    #[error("backend '{backend}' is not configured")]
    BackendNotFound { backend: String },

    #[error("role '{role}' has no backends configured")]
    NoBackends { role: String },
}

/// Resolved role with backends and execution mode
#[derive(Debug, Clone)]
pub struct ResolvedRole {
    /// Role name
    pub name: String,

    /// Backends to use for this role
    pub backends: Vec<String>,

    /// How to execute across backends
    pub execution: RoleExecution,

    /// Minimum successful backends (for parallel mode)
    pub min_success: u32,
}

/// Role resolver that maps role names to backends
#[derive(Debug)]
pub struct RoleResolver<'a> {
    config: &'a LlmuxConfig,
}

impl<'a> RoleResolver<'a> {
    /// Create a new role resolver
    pub fn new(config: &'a LlmuxConfig) -> Self {
        Self { config }
    }

    /// Resolve a role to its backends and execution mode
    ///
    /// Resolution order:
    /// 1. Team-specific role override (team.roles.X)
    /// 2. Global role definition (roles.X)
    pub fn resolve(&self, role: &str, team: Option<&str>) -> Result<ResolvedRole, RoleError> {
        // First try team-specific override
        if let Some(team_name) = team {
            if let Some(team_config) = self.config.teams.get(team_name) {
                if let Some(override_) = team_config.roles.get(role) {
                    // Validate backends exist
                    self.validate_backends(&override_.backends)?;

                    // Get execution mode from override or fall back to global role
                    let (execution, min_success) = if let Some(exec) = override_.execution {
                        (exec, 1) // Override specifies execution mode
                    } else if let Some(global_role) = self.config.roles.get(role) {
                        (global_role.execution, global_role.min_success)
                    } else {
                        (RoleExecution::First, 1)
                    };

                    return Ok(ResolvedRole {
                        name: role.to_string(),
                        backends: override_.backends.clone(),
                        execution,
                        min_success,
                    });
                }
            }
        }

        // Fall back to global role definition
        if let Some(role_config) = self.config.roles.get(role) {
            if role_config.backends.is_empty() {
                return Err(RoleError::NoBackends {
                    role: role.to_string(),
                });
            }

            // Validate backends exist
            self.validate_backends(&role_config.backends)?;

            return Ok(ResolvedRole {
                name: role.to_string(),
                backends: role_config.backends.clone(),
                execution: role_config.execution,
                min_success: role_config.min_success,
            });
        }

        Err(RoleError::RoleNotFound {
            role: role.to_string(),
        })
    }

    /// Validate that all backends exist in config
    fn validate_backends(&self, backends: &[String]) -> Result<(), RoleError> {
        for backend in backends {
            if !self.config.backends.contains_key(backend) {
                return Err(RoleError::BackendNotFound {
                    backend: backend.clone(),
                });
            }
        }
        Ok(())
    }

    /// Get all available roles
    pub fn available_roles(&self) -> Vec<&str> {
        self.config.roles.keys().map(|s| s.as_str()).collect()
    }
}

/// Convenience function to resolve a role
pub fn resolve_role(
    role: &str,
    team: Option<&str>,
    config: &LlmuxConfig,
) -> Result<ResolvedRole, RoleError> {
    let resolver = RoleResolver::new(config);
    resolver.resolve(role, team)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, RoleConfig, RoleOverride, TeamConfig};
    use std::collections::HashMap;

    fn create_test_config() -> LlmuxConfig {
        let mut config = LlmuxConfig::default();

        // Add backends
        config.backends.insert(
            "claude".into(),
            BackendConfig {
                command: "claude".into(),
                ..Default::default()
            },
        );
        config.backends.insert(
            "codex".into(),
            BackendConfig {
                command: "codex".into(),
                ..Default::default()
            },
        );
        config.backends.insert(
            "gemini".into(),
            BackendConfig {
                command: "gemini".into(),
                ..Default::default()
            },
        );

        // Add global roles
        config.roles.insert(
            "analyzer".into(),
            RoleConfig {
                description: "Code analysis".into(),
                backends: vec!["claude".into(), "codex".into()],
                execution: RoleExecution::First,
                min_success: 1,
            },
        );
        config.roles.insert(
            "reviewer".into(),
            RoleConfig {
                description: "Code review".into(),
                backends: vec!["claude".into()],
                execution: RoleExecution::Parallel,
                min_success: 1,
            },
        );

        // Add team with role overrides
        let mut rust_roles = HashMap::new();
        rust_roles.insert(
            "analyzer".into(),
            RoleOverride {
                backends: vec!["codex".into()], // Rust prefers codex
                execution: None,
            },
        );

        config.teams.insert(
            "rust".into(),
            TeamConfig {
                description: "Rust development".into(),
                detect: vec!["Cargo.toml".into()],
                verify: Some("cargo test".into()),
                roles: rust_roles,
            },
        );

        config
    }

    #[test]
    fn test_resolve_global_role() {
        let config = create_test_config();
        let resolver = RoleResolver::new(&config);

        let resolved = resolver.resolve("analyzer", None).unwrap();

        assert_eq!(resolved.name, "analyzer");
        assert_eq!(resolved.backends, vec!["claude", "codex"]);
        assert_eq!(resolved.execution, RoleExecution::First);
    }

    #[test]
    fn test_resolve_team_override() {
        let config = create_test_config();
        let resolver = RoleResolver::new(&config);

        let resolved = resolver.resolve("analyzer", Some("rust")).unwrap();

        assert_eq!(resolved.name, "analyzer");
        assert_eq!(resolved.backends, vec!["codex"]); // Team override
        assert_eq!(resolved.execution, RoleExecution::First); // From global role
    }

    #[test]
    fn test_resolve_role_not_found() {
        let config = create_test_config();
        let resolver = RoleResolver::new(&config);

        let result = resolver.resolve("nonexistent", None);

        assert!(matches!(result, Err(RoleError::RoleNotFound { .. })));
    }

    #[test]
    fn test_resolve_backend_not_found() {
        let mut config = create_test_config();

        // Add role with invalid backend
        config.roles.insert(
            "broken".into(),
            RoleConfig {
                description: "Broken role".into(),
                backends: vec!["nonexistent".into()],
                ..Default::default()
            },
        );

        let resolver = RoleResolver::new(&config);
        let result = resolver.resolve("broken", None);

        assert!(matches!(result, Err(RoleError::BackendNotFound { .. })));
    }

    #[test]
    fn test_resolve_no_backends() {
        let mut config = create_test_config();

        // Add role with no backends
        config.roles.insert(
            "empty".into(),
            RoleConfig {
                description: "Empty role".into(),
                backends: vec![],
                ..Default::default()
            },
        );

        let resolver = RoleResolver::new(&config);
        let result = resolver.resolve("empty", None);

        assert!(matches!(result, Err(RoleError::NoBackends { .. })));
    }

    #[test]
    fn test_resolve_fallback_to_global() {
        let config = create_test_config();
        let resolver = RoleResolver::new(&config);

        // reviewer is not overridden in rust team
        let resolved = resolver.resolve("reviewer", Some("rust")).unwrap();

        assert_eq!(resolved.name, "reviewer");
        assert_eq!(resolved.backends, vec!["claude"]); // From global
        assert_eq!(resolved.execution, RoleExecution::Parallel);
    }

    #[test]
    fn test_available_roles() {
        let config = create_test_config();
        let resolver = RoleResolver::new(&config);

        let roles = resolver.available_roles();

        assert!(roles.contains(&"analyzer"));
        assert!(roles.contains(&"reviewer"));
    }

    #[test]
    fn test_resolve_role_function() {
        let config = create_test_config();

        let resolved = resolve_role("analyzer", None, &config).unwrap();
        assert_eq!(resolved.name, "analyzer");
    }
}
