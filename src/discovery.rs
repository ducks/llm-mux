//! Ecosystem discovery utilities - static file analysis helpers
//!
//! This module provides utility functions for analyzing projects and extracting
//! structured information from manifest files. These functions are meant to be
//! called by discovery workflows, not directly by CLI commands.

#![allow(dead_code)]
//!
//! Discovery workflows should:
//! 1. Use static analysis functions to gather basic facts
//! 2. Call LLM roles to perform deep analysis
//! 3. Store discovered facts in the memory database
//!
//! Example discovery workflow:
//! ```toml
//! [[steps]]
//! name = "analyze"
//! type = "query"
//! role = "ecosystem_analyzer"
//! prompt = "Analyze {{ ecosystem.name }} and discover relationships..."
//! ```

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::config::{EcosystemConfig, ProjectConfig};
use crate::memory::{EcosystemMemory, Fact};

/// Discovered fact about a project
#[derive(Debug, Clone)]
pub struct DiscoveredFact {
    pub fact: String,
    pub source: String,
    pub confidence: f64,
}

/// Analyze a project and discover facts
pub fn analyze_project(
    _ecosystem_name: &str,
    project_name: &str,
    project: &ProjectConfig,
) -> Result<Vec<DiscoveredFact>> {
    let mut facts = Vec::new();
    let project_path_str = project.path.display().to_string();
    let project_path = shellexpand::tilde(&project_path_str);
    let path = Path::new(project_path.as_ref());

    if !path.exists() {
        return Ok(facts);
    }

    // Project type
    if let Some(ref project_type) = project.project_type {
        if !project_type.is_empty() {
            facts.push(DiscoveredFact {
                fact: format!("{} is a {} project", project_name, project_type),
                source: "config".to_string(),
                confidence: 1.0,
            });
        }
    }

    // Project description
    if !project.description.is_empty() {
        facts.push(DiscoveredFact {
            fact: format!("{}: {}", project_name, project.description),
            source: "config".to_string(),
            confidence: 1.0,
        });
    }

    // Dependencies from config
    if !project.depends_on.is_empty() {
        facts.push(DiscoveredFact {
            fact: format!(
                "{} depends on: {}",
                project_name,
                project.depends_on.join(", ")
            ),
            source: "config".to_string(),
            confidence: 1.0,
        });
    }

    // Tags
    if !project.tags.is_empty() {
        facts.push(DiscoveredFact {
            fact: format!("{} tags: {}", project_name, project.tags.join(", ")),
            source: "config".to_string(),
            confidence: 1.0,
        });
    }

    // Analyze manifest files based on project type
    if let Some(ref project_type) = project.project_type {
        match project_type.as_str() {
            "ruby" => analyze_ruby_project(project_name, path, &mut facts)?,
            "rust" => analyze_rust_project(project_name, path, &mut facts)?,
            "javascript" | "typescript" => analyze_node_project(project_name, path, &mut facts)?,
            "go" => analyze_go_project(project_name, path, &mut facts)?,
            "python" => analyze_python_project(project_name, path, &mut facts)?,
            _ => {}
        }
    }

    // Analyze README if present
    analyze_readme(project_name, path, &mut facts)?;

    Ok(facts)
}

/// Analyze Ruby/Rails project
fn analyze_ruby_project(
    project_name: &str,
    path: &Path,
    facts: &mut Vec<DiscoveredFact>,
) -> Result<()> {
    // Check for Gemfile
    let gemfile = path.join("Gemfile");
    if gemfile.exists() {
        let content = std::fs::read_to_string(&gemfile)?;

        // Check for Rails
        if content.contains("gem 'rails'") || content.contains("gem \"rails\"") {
            facts.push(DiscoveredFact {
                fact: format!("{} is a Rails application", project_name),
                source: "Gemfile".to_string(),
                confidence: 1.0,
            });
        }

        // Check for Sinatra
        if content.contains("gem 'sinatra'") || content.contains("gem \"sinatra\"") {
            facts.push(DiscoveredFact {
                fact: format!("{} is a Sinatra application", project_name),
                source: "Gemfile".to_string(),
                confidence: 1.0,
            });
        }

        // Extract key gems
        let mut gems = Vec::new();
        for line in content.lines() {
            if let Some(gem_name) = extract_gem_name(line) {
                if is_notable_gem(&gem_name) {
                    gems.push(gem_name);
                }
            }
        }

        if !gems.is_empty() {
            facts.push(DiscoveredFact {
                fact: format!("{} uses: {}", project_name, gems.join(", ")),
                source: "Gemfile".to_string(),
                confidence: 0.9,
            });
        }
    }

    Ok(())
}

/// Analyze Rust project
fn analyze_rust_project(
    project_name: &str,
    path: &Path,
    facts: &mut Vec<DiscoveredFact>,
) -> Result<()> {
    let cargo_toml = path.join("Cargo.toml");
    if cargo_toml.exists() {
        let content = std::fs::read_to_string(&cargo_toml)?;

        // Parse as TOML and extract dependencies
        if let Ok(parsed) = content.parse::<toml::Value>() {
            if let Some(deps) = parsed.get("dependencies").and_then(|v| v.as_table()) {
                let dep_names: Vec<_> = deps.keys().map(|k| k.as_str()).collect();
                if !dep_names.is_empty() {
                    facts.push(DiscoveredFact {
                        fact: format!("{} uses: {}", project_name, dep_names.join(", ")),
                        source: "Cargo.toml".to_string(),
                        confidence: 0.9,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Analyze Node.js project
fn analyze_node_project(
    project_name: &str,
    path: &Path,
    facts: &mut Vec<DiscoveredFact>,
) -> Result<()> {
    let package_json = path.join("package.json");
    if package_json.exists() {
        let content = std::fs::read_to_string(&package_json)?;

        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
            // Check for framework
            if let Some(deps) = parsed.get("dependencies").and_then(|v| v.as_object()) {
                if deps.contains_key("react") {
                    facts.push(DiscoveredFact {
                        fact: format!("{} is a React application", project_name),
                        source: "package.json".to_string(),
                        confidence: 1.0,
                    });
                }
                if deps.contains_key("vue") {
                    facts.push(DiscoveredFact {
                        fact: format!("{} is a Vue application", project_name),
                        source: "package.json".to_string(),
                        confidence: 1.0,
                    });
                }
                if deps.contains_key("next") {
                    facts.push(DiscoveredFact {
                        fact: format!("{} is a Next.js application", project_name),
                        source: "package.json".to_string(),
                        confidence: 1.0,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Analyze Go project
fn analyze_go_project(
    project_name: &str,
    path: &Path,
    facts: &mut Vec<DiscoveredFact>,
) -> Result<()> {
    let go_mod = path.join("go.mod");
    if go_mod.exists() {
        let content = std::fs::read_to_string(&go_mod)?;

        // Extract module name
        for line in content.lines() {
            if line.starts_with("module ") {
                let module = line.strip_prefix("module ").unwrap_or("").trim();
                facts.push(DiscoveredFact {
                    fact: format!("{} is Go module: {}", project_name, module),
                    source: "go.mod".to_string(),
                    confidence: 1.0,
                });
                break;
            }
        }
    }

    Ok(())
}

/// Analyze Python project
fn analyze_python_project(
    project_name: &str,
    path: &Path,
    facts: &mut Vec<DiscoveredFact>,
) -> Result<()> {
    // Check for requirements.txt
    let requirements = path.join("requirements.txt");
    if requirements.exists() {
        let content = std::fs::read_to_string(&requirements)?;

        // Check for Django
        if content.contains("Django") {
            facts.push(DiscoveredFact {
                fact: format!("{} is a Django application", project_name),
                source: "requirements.txt".to_string(),
                confidence: 1.0,
            });
        }

        // Check for Flask
        if content.contains("Flask") {
            facts.push(DiscoveredFact {
                fact: format!("{} is a Flask application", project_name),
                source: "requirements.txt".to_string(),
                confidence: 1.0,
            });
        }
    }

    Ok(())
}

/// Analyze README file
fn analyze_readme(project_name: &str, path: &Path, facts: &mut Vec<DiscoveredFact>) -> Result<()> {
    // Try common README names
    for readme_name in &["README.md", "README", "readme.md", "Readme.md"] {
        let readme = path.join(readme_name);
        if readme.exists() {
            if let Ok(content) = std::fs::read_to_string(&readme) {
                // Extract first paragraph as description if not too long
                let first_para = content
                    .lines()
                    .skip_while(|line| line.trim().starts_with('#') || line.trim().is_empty())
                    .take_while(|line| !line.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");

                if !first_para.is_empty() && first_para.len() < 300 {
                    facts.push(DiscoveredFact {
                        fact: format!("{}: {}", project_name, first_para.trim()),
                        source: "README".to_string(),
                        confidence: 0.8,
                    });
                }

                break;
            }
        }
    }

    Ok(())
}

/// Extract gem name from Gemfile line
fn extract_gem_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("gem ") {
        // Extract gem name between quotes
        if let Some(start) = trimmed.find(['\'', '"']) {
            let quote = trimmed.chars().nth(start)?;
            let after_quote = &trimmed[start + 1..];
            if let Some(end) = after_quote.find(quote) {
                return Some(after_quote[..end].to_string());
            }
        }
    }
    None
}

/// Check if a gem is notable enough to mention
fn is_notable_gem(gem: &str) -> bool {
    matches!(
        gem,
        "pg" | "mysql2"
            | "redis"
            | "sidekiq"
            | "resque"
            | "elasticsearch"
            | "aws-sdk"
            | "stripe"
            | "devise"
            | "cancancan"
            | "pundit"
    )
}

/// Discover and seed ecosystem knowledge
pub async fn discover_ecosystem(
    ecosystem_name: &str,
    config: &EcosystemConfig,
    force: bool,
) -> Result<HashMap<String, Vec<DiscoveredFact>>> {
    let mut all_facts = HashMap::new();

    // Open memory database
    let memory_path = EcosystemMemory::default_path(ecosystem_name)?;
    let mut memory = EcosystemMemory::open(&memory_path)?;

    // Check if facts already exist
    if !force {
        let existing_facts = memory.get_facts(ecosystem_name)?;
        if !existing_facts.is_empty() {
            return Err(anyhow::anyhow!(
                "Knowledge base already exists for ecosystem '{}'. Use --force to re-discover.",
                ecosystem_name
            ));
        }
    }

    // Analyze each project
    for (project_name, project_config) in &config.projects {
        let facts = analyze_project(ecosystem_name, project_name, project_config)
            .with_context(|| format!("Failed to analyze project '{}'", project_name))?;

        all_facts.insert(project_name.to_string(), facts.clone());

        // Store facts in database
        for discovered_fact in facts {
            let fact = Fact {
                id: None,
                ecosystem: ecosystem_name.to_string(),
                fact: discovered_fact.fact,
                source: discovered_fact.source,
                source_type: Some("file".to_string()),
                category: None,
                confidence: discovered_fact.confidence,
                created_at: String::new(),
                updated_at: String::new(),
            };
            memory.add_fact(&fact)?;
        }
    }

    // Add ecosystem-level knowledge from config
    for knowledge in &config.knowledge {
        let fact = Fact {
            id: None,
            ecosystem: ecosystem_name.to_string(),
            fact: knowledge.clone(),
            source: "config".to_string(),
            source_type: Some("config".to_string()),
            category: Some("knowledge".to_string()),
            confidence: 1.0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        memory.add_fact(&fact)?;
    }

    Ok(all_facts)
}
