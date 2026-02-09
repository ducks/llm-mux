//! Auto-detect project team from marker files

use crate::config::TeamConfig;
use std::collections::HashMap;
use std::path::Path;

/// Team detector that finds the appropriate team for a project
#[derive(Debug)]
pub struct TeamDetector {
    /// Team configurations with their detection patterns
    teams: HashMap<String, TeamConfig>,
}

impl TeamDetector {
    /// Create a new team detector from team configurations
    pub fn new(teams: HashMap<String, TeamConfig>) -> Self {
        Self { teams }
    }

    /// Detect the team for a project directory
    ///
    /// Returns the name of the first team whose marker files are found.
    /// Teams are checked in no particular order (HashMap iteration).
    /// For deterministic ordering, teams should be prioritized externally.
    pub fn detect(&self, dir: &Path) -> Option<String> {
        for (name, team) in &self.teams {
            if self.matches_team(dir, team) {
                return Some(name.clone());
            }
        }
        None
    }

    /// Detect with CLI override taking precedence
    pub fn detect_with_override(&self, dir: &Path, override_team: Option<&str>) -> Option<String> {
        // CLI override takes absolute precedence
        if let Some(team) = override_team {
            if self.teams.contains_key(team) {
                return Some(team.to_string());
            }
            // Unknown team specified - return it anyway (will error at resolution)
            return Some(team.to_string());
        }

        self.detect(dir)
    }

    /// Check if a directory matches a team's detection patterns
    fn matches_team(&self, dir: &Path, team: &TeamConfig) -> bool {
        if team.detect.is_empty() {
            return false;
        }

        for pattern in &team.detect {
            if self.pattern_matches(dir, pattern) {
                return true;
            }
        }

        false
    }

    /// Check if a pattern matches in the directory
    fn pattern_matches(&self, dir: &Path, pattern: &str) -> bool {
        // Simple file existence check for now
        // Could be extended to support globs
        let path = dir.join(pattern);
        path.exists()
    }
}

/// Detect team for a directory with optional override
pub fn detect_team(
    dir: &Path,
    teams: &HashMap<String, TeamConfig>,
    override_team: Option<&str>,
) -> Option<String> {
    let detector = TeamDetector::new(teams.clone());
    detector.detect_with_override(dir, override_team)
}

/// Built-in team detection patterns (can be overridden by config)
pub fn default_teams() -> HashMap<String, TeamConfig> {
    let mut teams = HashMap::new();

    teams.insert(
        "rust".into(),
        TeamConfig {
            description: "Rust development".into(),
            detect: vec!["Cargo.toml".into()],
            verify: Some("cargo clippy && cargo test".into()),
            ..Default::default()
        },
    );

    teams.insert(
        "ruby".into(),
        TeamConfig {
            description: "Ruby development".into(),
            detect: vec!["Gemfile".into()],
            verify: Some("bundle exec rake".into()),
            ..Default::default()
        },
    );

    teams.insert(
        "node".into(),
        TeamConfig {
            description: "Node.js development".into(),
            detect: vec!["package.json".into()],
            verify: Some("npm test".into()),
            ..Default::default()
        },
    );

    teams.insert(
        "python".into(),
        TeamConfig {
            description: "Python development".into(),
            detect: vec!["pyproject.toml".into(), "setup.py".into(), "requirements.txt".into()],
            verify: Some("pytest".into()),
            ..Default::default()
        },
    );

    teams.insert(
        "go".into(),
        TeamConfig {
            description: "Go development".into(),
            detect: vec!["go.mod".into()],
            verify: Some("go test ./...".into()),
            ..Default::default()
        },
    );

    teams
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    fn create_test_teams() -> HashMap<String, TeamConfig> {
        let mut teams = HashMap::new();

        teams.insert(
            "rust".into(),
            TeamConfig {
                description: "Rust".into(),
                detect: vec!["Cargo.toml".into()],
                ..Default::default()
            },
        );

        teams.insert(
            "ruby".into(),
            TeamConfig {
                description: "Ruby".into(),
                detect: vec!["Gemfile".into()],
                ..Default::default()
            },
        );

        teams.insert(
            "node".into(),
            TeamConfig {
                description: "Node".into(),
                detect: vec!["package.json".into()],
                ..Default::default()
            },
        );

        teams
    }

    #[test]
    fn test_detect_rust() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("Cargo.toml")).unwrap();

        let detector = TeamDetector::new(create_test_teams());
        let team = detector.detect(dir.path());

        assert_eq!(team, Some("rust".into()));
    }

    #[test]
    fn test_detect_ruby() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("Gemfile")).unwrap();

        let detector = TeamDetector::new(create_test_teams());
        let team = detector.detect(dir.path());

        assert_eq!(team, Some("ruby".into()));
    }

    #[test]
    fn test_detect_node() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("package.json")).unwrap();

        let detector = TeamDetector::new(create_test_teams());
        let team = detector.detect(dir.path());

        assert_eq!(team, Some("node".into()));
    }

    #[test]
    fn test_detect_none() {
        let dir = TempDir::new().unwrap();
        // Empty directory

        let detector = TeamDetector::new(create_test_teams());
        let team = detector.detect(dir.path());

        assert!(team.is_none());
    }

    #[test]
    fn test_cli_override() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("Cargo.toml")).unwrap();

        let detector = TeamDetector::new(create_test_teams());

        // Override should take precedence
        let team = detector.detect_with_override(dir.path(), Some("ruby"));
        assert_eq!(team, Some("ruby".into()));

        // Without override, should detect rust
        let team = detector.detect_with_override(dir.path(), None);
        assert_eq!(team, Some("rust".into()));
    }

    #[test]
    fn test_unknown_override() {
        let dir = TempDir::new().unwrap();

        let detector = TeamDetector::new(create_test_teams());

        // Unknown team is still returned (will error at resolution)
        let team = detector.detect_with_override(dir.path(), Some("unknown"));
        assert_eq!(team, Some("unknown".into()));
    }

    #[test]
    fn test_detect_team_function() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("Gemfile")).unwrap();

        let teams = create_test_teams();
        let team = detect_team(dir.path(), &teams, None);

        assert_eq!(team, Some("ruby".into()));
    }

    #[test]
    fn test_default_teams() {
        let teams = default_teams();

        assert!(teams.contains_key("rust"));
        assert!(teams.contains_key("ruby"));
        assert!(teams.contains_key("node"));
        assert!(teams.contains_key("python"));
        assert!(teams.contains_key("go"));

        // Check rust team has correct detection pattern
        let rust = &teams["rust"];
        assert!(rust.detect.contains(&"Cargo.toml".into()));
    }
}
