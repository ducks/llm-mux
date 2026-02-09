//! Role and team resolution for llmux
//!
//! This module handles:
//! - Auto-detecting project teams from marker files
//! - Resolving roles to backend lists with team overrides
//! - Executing roles across backends with different modes
//!
//! # Example
//!
//! ```ignore
//! use llmux::role::{TeamDetector, RoleResolver, RoleExecutor, resolve_role};
//! use llmux::config::LlmuxConfig;
//!
//! // Auto-detect team
//! let teams = config.teams.clone();
//! let team = detect_team(Path::new("."), &teams, None);
//!
//! // Resolve role to backends
//! let resolved = resolve_role("analyzer", team.as_deref(), &config)?;
//!
//! // Execute across backends
//! let executor = RoleExecutor::new(Arc::new(config));
//! let result = executor.execute(&resolved, &request).await?;
//! ```

mod role_executor;
mod role_resolver;
mod team_detector;

pub use role_executor::{ExecutionError, RoleExecutor};
pub use role_resolver::{RoleError, resolve_role};
pub use team_detector::detect_team;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, LlmuxConfig, RoleConfig, RoleExecution, TeamConfig};
    
    use std::sync::Arc;

    fn create_full_config() -> LlmuxConfig {
        let mut config = LlmuxConfig::default();

        // Backends
        config.backends.insert(
            "claude".into(),
            BackendConfig {
                command: "echo".into(), // Use echo for testing
                ..Default::default()
            },
        );
        config.backends.insert(
            "codex".into(),
            BackendConfig {
                command: "echo".into(),
                ..Default::default()
            },
        );

        // Roles
        config.roles.insert(
            "analyzer".into(),
            RoleConfig {
                description: "Analyze code".into(),
                backends: vec!["claude".into(), "codex".into()],
                execution: RoleExecution::First,
                min_success: 1,
            },
        );

        // Teams
        config.teams.insert(
            "rust".into(),
            TeamConfig {
                description: "Rust development".into(),
                detect: vec!["Cargo.toml".into()],
                ..Default::default()
            },
        );

        config
    }

    #[test]
    fn test_full_resolution_flow() {
        let config = create_full_config();

        // Resolve role without team
        let resolved = resolve_role("analyzer", None, &config).unwrap();
        assert_eq!(resolved.backends, vec!["claude", "codex"]);

        // Resolve role with team (no override, falls back to global)
        let resolved = resolve_role("analyzer", Some("rust"), &config).unwrap();
        assert_eq!(resolved.backends, vec!["claude", "codex"]);
    }

    #[tokio::test]
    async fn test_execution_flow() {
        let config = Arc::new(create_full_config());

        // Resolve role
        let resolved = resolve_role("analyzer", None, &config).unwrap();

        // Execute
        let executor = RoleExecutor::new(config);
        let request = crate::backend_executor::BackendRequest::new("test prompt");
        let result = executor.execute(&resolved, &request).await.unwrap();

        assert!(result.output.is_some());
        assert!(!result.succeeded.is_empty());
    }
}
