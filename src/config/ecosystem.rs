#![allow(dead_code)]

//! Ecosystem and project configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration for a project within an ecosystem
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Path to project directory
    pub path: PathBuf,

    /// Project type (ruby, rust, javascript, etc.)
    #[serde(rename = "type")]
    pub project_type: Option<String>,

    /// Other projects this one depends on
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Tags for categorization/filtering
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Configuration for an ecosystem (group of related projects)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EcosystemConfig {
    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Projects in this ecosystem
    #[serde(default)]
    pub projects: HashMap<String, ProjectConfig>,

    /// Facts/knowledge about this ecosystem
    #[serde(default)]
    pub knowledge: Vec<String>,
}

impl Default for EcosystemConfig {
    fn default() -> Self {
        Self {
            description: String::new(),
            projects: HashMap::new(),
            knowledge: Vec::new(),
        }
    }
}

impl EcosystemConfig {
    /// Get a project by name
    pub fn get_project(&self, name: &str) -> Option<&ProjectConfig> {
        self.projects.get(name)
    }

    /// Get all projects that depend on a given project
    pub fn get_dependents(&self, project_name: &str) -> Vec<&String> {
        self.projects
            .iter()
            .filter(|(_, p)| p.depends_on.contains(&project_name.to_string()))
            .map(|(name, _)| name)
            .collect()
    }

    /// Get all projects a given project depends on
    pub fn get_dependencies(&self, project_name: &str) -> Option<&[String]> {
        self.projects
            .get(project_name)
            .map(|p| p.depends_on.as_slice())
    }

    /// Get all projects with a specific tag
    pub fn get_projects_by_tag(&self, tag: &str) -> Vec<&String> {
        self.projects
            .iter()
            .filter(|(_, p)| p.tags.contains(&tag.to_string()))
            .map(|(name, _)| name)
            .collect()
    }

    /// Get all projects of a specific type
    pub fn get_projects_by_type(&self, project_type: &str) -> Vec<&String> {
        self.projects
            .iter()
            .filter(|(_, p)| p.project_type.as_deref() == Some(project_type))
            .map(|(name, _)| name)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_config_minimal() {
        let toml = r#"
            path = "~/dev/myproject"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.path, PathBuf::from("~/dev/myproject"));
        assert_eq!(config.depends_on, Vec::<String>::new());
    }

    #[test]
    fn test_project_config_full() {
        let toml = r#"
            description = "Main application"
            path = "~/dev/app"
            type = "ruby"
            depends_on = ["database", "cache"]
            tags = ["production", "web"]
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.description, "Main application");
        assert_eq!(config.project_type, Some("ruby".into()));
        assert_eq!(config.depends_on, vec!["database", "cache"]);
        assert_eq!(config.tags, vec!["production", "web"]);
    }

    #[test]
    fn test_ecosystem_config() {
        let toml = r#"
            description = "Discourse ecosystem"

            [projects.discourse]
            path = "~/discourse/discourse"
            type = "ruby"
            depends_on = ["mothership", "docker-hosting"]

            [projects.mothership]
            path = "~/discourse/mothership"
            type = "ruby"

            [projects.docker-hosting]
            path = "~/discourse/docker-hosting"
            type = "ruby"
        "#;
        let config: EcosystemConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.description, "Discourse ecosystem");
        assert_eq!(config.projects.len(), 3);
        assert!(config.projects.contains_key("discourse"));
        assert!(config.projects.contains_key("mothership"));
    }

    #[test]
    fn test_ecosystem_get_dependencies() {
        let config: EcosystemConfig = toml::from_str(
            r#"
            [projects.app]
            path = "~/app"
            depends_on = ["db", "cache"]

            [projects.db]
            path = "~/db"

            [projects.cache]
            path = "~/cache"
        "#,
        )
        .unwrap();

        let deps = config.get_dependencies("app").unwrap();
        assert_eq!(deps, &["db", "cache"]);

        let deps = config.get_dependencies("db").unwrap();
        assert_eq!(deps.len(), 0);
    }

    #[test]
    fn test_ecosystem_get_dependents() {
        let config: EcosystemConfig = toml::from_str(
            r#"
            [projects.app]
            path = "~/app"
            depends_on = ["db"]

            [projects.worker]
            path = "~/worker"
            depends_on = ["db"]

            [projects.db]
            path = "~/db"
        "#,
        )
        .unwrap();

        let mut dependents = config.get_dependents("db");
        dependents.sort();
        assert_eq!(dependents, vec!["app", "worker"]);

        let dependents = config.get_dependents("app");
        assert_eq!(dependents.len(), 0);
    }

    #[test]
    fn test_ecosystem_get_by_tag() {
        let config: EcosystemConfig = toml::from_str(
            r#"
            [projects.app]
            path = "~/app"
            tags = ["production", "web"]

            [projects.worker]
            path = "~/worker"
            tags = ["production", "background"]

            [projects.test]
            path = "~/test"
            tags = ["development"]
        "#,
        )
        .unwrap();

        let mut production = config.get_projects_by_tag("production");
        production.sort();
        assert_eq!(production, vec!["app", "worker"]);

        let web = config.get_projects_by_tag("web");
        assert_eq!(web, vec!["app"]);
    }

    #[test]
    fn test_ecosystem_get_by_type() {
        let config: EcosystemConfig = toml::from_str(
            r#"
            [projects.backend]
            path = "~/backend"
            type = "ruby"

            [projects.frontend]
            path = "~/frontend"
            type = "javascript"

            [projects.worker]
            path = "~/worker"
            type = "ruby"
        "#,
        )
        .unwrap();

        let mut ruby_projects = config.get_projects_by_type("ruby");
        ruby_projects.sort();
        assert_eq!(ruby_projects, vec!["backend", "worker"]);

        let js_projects = config.get_projects_by_type("javascript");
        assert_eq!(js_projects, vec!["frontend"]);
    }

    #[test]
    fn test_ecosystem_knowledge() {
        let toml = r#"
            description = "Test ecosystem"
            knowledge = [
                "postgres-manager alerts when unused databases exist",
                "flex clusters are production, yyz is test"
            ]

            [projects.app]
            path = "~/app"
        "#;
        let config: EcosystemConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.knowledge.len(), 2);
        assert!(config.knowledge[0].contains("postgres-manager"));
    }
}
