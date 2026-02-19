//! Ecosystem memory store

#![allow(dead_code)]

use super::schema::init_schema;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// A fact about the ecosystem
#[derive(Debug, Clone)]
pub struct Fact {
    pub id: Option<i64>,
    pub ecosystem: String,
    pub fact: String,
    pub source: String,
    pub source_type: Option<String>,
    pub category: Option<String>,
    pub confidence: f64,
    pub created_at: String,
    pub updated_at: String,
}

/// A relationship between projects
#[derive(Debug, Clone)]
pub struct ProjectRelationship {
    pub id: Option<i64>,
    pub ecosystem: String,
    pub from_project: String,
    pub to_project: String,
    pub relationship_type: String,
    pub metadata: Option<String>,
    pub created_at: String,
}

/// A finding (bug, issue, tech debt)
#[derive(Debug, Clone)]
pub struct Finding {
    pub id: Option<i64>,
    pub ecosystem: String,
    pub project: Option<String>,
    pub category: String,
    pub severity: Option<String>,
    pub description: String,
    pub location: Option<String>,
    pub workflow_run_id: Option<i64>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A workflow execution record
#[derive(Debug, Clone)]
pub struct WorkflowRun {
    pub id: Option<i64>,
    pub ecosystem: String,
    pub project: Option<String>,
    pub workflow_name: String,
    pub success: bool,
    pub duration_ms: Option<i64>,
    pub failed_step: Option<String>,
    pub error_message: Option<String>,
    pub output_dir: Option<String>,
    pub created_at: String,
}

/// An entity in the ecosystem (dependency, service, config, etc.)
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: Option<i64>,
    pub ecosystem: String,
    pub project: String,
    pub entity_type: String,
    pub entity_name: String,
    pub created_at: String,
}

/// A property of an entity with history tracking
#[derive(Debug, Clone)]
pub struct EntityProperty {
    pub id: Option<i64>,
    pub entity_id: i64,
    pub property_name: String,
    pub property_value: String,
    pub source: String,
    pub source_type: Option<String>,
    pub confidence: f64,
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub created_at: String,
}

/// Ecosystem memory storage
pub struct EcosystemMemory {
    conn: Connection,
}

impl EcosystemMemory {
    /// Open or create ecosystem memory database
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open memory database at {}", path.display()))?;

        init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Get the default memory database path for an ecosystem
    pub fn default_path(ecosystem: &str) -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not determine config directory")?;

        let memory_dir = config_dir.join("llm-mux").join("memory");
        std::fs::create_dir_all(&memory_dir).with_context(|| {
            format!(
                "Failed to create memory directory at {}",
                memory_dir.display()
            )
        })?;

        Ok(memory_dir.join(format!("{}.db", ecosystem)))
    }

    /// Add a fact to the ecosystem
    pub fn add_fact(&mut self, fact: &Fact) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO facts (ecosystem, fact, source, source_type, category, confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(ecosystem, fact, source) DO UPDATE SET
                source_type = excluded.source_type,
                category = excluded.category,
                confidence = excluded.confidence,
                updated_at = excluded.updated_at",
            (
                &fact.ecosystem,
                &fact.fact,
                &fact.source,
                &fact.source_type,
                &fact.category,
                fact.confidence,
                &now,
                &now,
            ),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get all facts for an ecosystem
    pub fn get_facts(&self, ecosystem: &str) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ecosystem, fact, source, source_type, category, confidence, created_at, updated_at
             FROM facts
             WHERE ecosystem = ?1
             ORDER BY confidence DESC, created_at DESC",
        )?;

        let facts = stmt
            .query_map([ecosystem], |row| {
                Ok(Fact {
                    id: Some(row.get(0)?),
                    ecosystem: row.get(1)?,
                    fact: row.get(2)?,
                    source: row.get(3)?,
                    source_type: row.get(4)?,
                    category: row.get(5)?,
                    confidence: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(facts)
    }

    /// Add a project relationship
    pub fn add_relationship(&mut self, rel: &ProjectRelationship) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO project_relationships (ecosystem, from_project, to_project, relationship_type, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(ecosystem, from_project, to_project, relationship_type) DO NOTHING",
            (
                &rel.ecosystem,
                &rel.from_project,
                &rel.to_project,
                &rel.relationship_type,
                &rel.metadata,
                &now,
            ),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get relationships for a project
    pub fn get_relationships(
        &self,
        ecosystem: &str,
        project: Option<&str>,
    ) -> Result<Vec<ProjectRelationship>> {
        let query = if project.is_some() {
            "SELECT id, ecosystem, from_project, to_project, relationship_type, metadata, created_at
             FROM project_relationships
             WHERE ecosystem = ?1 AND (from_project = ?2 OR to_project = ?2)
             ORDER BY created_at DESC"
        } else {
            "SELECT id, ecosystem, from_project, to_project, relationship_type, metadata, created_at
             FROM project_relationships
             WHERE ecosystem = ?1
             ORDER BY created_at DESC"
        };

        let mut stmt = self.conn.prepare(query)?;

        let rows: Vec<ProjectRelationship> = if let Some(proj) = project {
            stmt.query_map([ecosystem, proj], |row| {
                Ok(ProjectRelationship {
                    id: Some(row.get(0)?),
                    ecosystem: row.get(1)?,
                    from_project: row.get(2)?,
                    to_project: row.get(3)?,
                    relationship_type: row.get(4)?,
                    metadata: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([ecosystem], |row| {
                Ok(ProjectRelationship {
                    id: Some(row.get(0)?),
                    ecosystem: row.get(1)?,
                    from_project: row.get(2)?,
                    to_project: row.get(3)?,
                    relationship_type: row.get(4)?,
                    metadata: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    }

    /// Add a finding
    pub fn add_finding(&mut self, finding: &Finding) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO findings (ecosystem, project, category, severity, description, location, workflow_run_id, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                &finding.ecosystem,
                &finding.project,
                &finding.category,
                &finding.severity,
                &finding.description,
                &finding.location,
                &finding.workflow_run_id,
                &finding.status,
                &now,
                &now,
            ),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get findings for an ecosystem or project
    pub fn get_findings(
        &self,
        ecosystem: &str,
        project: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<Finding>> {
        let query = match (project, status) {
            (Some(_), Some(_)) => {
                "SELECT id, ecosystem, project, category, severity, description, location, workflow_run_id, status, created_at, updated_at
                 FROM findings
                 WHERE ecosystem = ?1 AND project = ?2 AND status = ?3
                 ORDER BY created_at DESC"
            }
            (Some(_), None) => {
                "SELECT id, ecosystem, project, category, severity, description, location, workflow_run_id, status, created_at, updated_at
                 FROM findings
                 WHERE ecosystem = ?1 AND project = ?2
                 ORDER BY created_at DESC"
            }
            (None, Some(_)) => {
                "SELECT id, ecosystem, project, category, severity, description, location, workflow_run_id, status, created_at, updated_at
                 FROM findings
                 WHERE ecosystem = ?1 AND status = ?2
                 ORDER BY created_at DESC"
            }
            (None, None) => {
                "SELECT id, ecosystem, project, category, severity, description, location, workflow_run_id, status, created_at, updated_at
                 FROM findings
                 WHERE ecosystem = ?1
                 ORDER BY created_at DESC"
            }
        };

        let mut stmt = self.conn.prepare(query)?;

        let row_mapper = |row: &rusqlite::Row| -> rusqlite::Result<Finding> {
            Ok(Finding {
                id: Some(row.get(0)?),
                ecosystem: row.get(1)?,
                project: row.get(2)?,
                category: row.get(3)?,
                severity: row.get(4)?,
                description: row.get(5)?,
                location: row.get(6)?,
                workflow_run_id: row.get(7)?,
                status: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        };

        let findings: Vec<Finding> = match (project, status) {
            (Some(p), Some(s)) => stmt
                .query_map([ecosystem, p, s], row_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            (Some(p), None) => stmt
                .query_map([ecosystem, p], row_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            (None, Some(s)) => stmt
                .query_map([ecosystem, s], row_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            (None, None) => stmt
                .query_map([ecosystem], row_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };

        Ok(findings)
    }

    /// Record a workflow run
    pub fn record_run(&mut self, run: &WorkflowRun) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO workflow_runs (ecosystem, project, workflow_name, success, duration_ms, failed_step, error_message, output_dir, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                &run.ecosystem,
                &run.project,
                &run.workflow_name,
                run.success,
                run.duration_ms,
                &run.failed_step,
                &run.error_message,
                &run.output_dir,
                &now,
            ),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get recent workflow runs
    pub fn get_recent_runs(&self, ecosystem: &str, limit: usize) -> Result<Vec<WorkflowRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ecosystem, project, workflow_name, success, duration_ms, failed_step, error_message, output_dir, created_at
             FROM workflow_runs
             WHERE ecosystem = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;

        let runs = stmt
            .query_map([ecosystem, &limit.to_string()], |row| {
                Ok(WorkflowRun {
                    id: Some(row.get(0)?),
                    ecosystem: row.get(1)?,
                    project: row.get(2)?,
                    workflow_name: row.get(3)?,
                    success: row.get(4)?,
                    duration_ms: row.get(5)?,
                    failed_step: row.get(6)?,
                    error_message: row.get(7)?,
                    output_dir: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(runs)
    }

    /// Get or create an entity
    pub fn get_or_create_entity(&mut self, entity: &Entity) -> Result<i64> {
        // Try to find existing entity
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM entities
                 WHERE ecosystem = ?1 AND project = ?2 AND entity_type = ?3 AND entity_name = ?4",
                (
                    &entity.ecosystem,
                    &entity.project,
                    &entity.entity_type,
                    &entity.entity_name,
                ),
                |row| row.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            return Ok(id);
        }

        // Create new entity
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO entities (ecosystem, project, entity_type, entity_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                &entity.ecosystem,
                &entity.project,
                &entity.entity_type,
                &entity.entity_name,
                &now,
            ),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Add or update an entity property (with history tracking)
    pub fn set_entity_property(&mut self, property: &EntityProperty) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();

        // Check if there's a current property with the same value
        let existing: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, property_value FROM entity_properties
                 WHERE entity_id = ?1 AND property_name = ?2 AND valid_to IS NULL",
                (property.entity_id, &property.property_name),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((existing_id, existing_value)) = existing {
            // If value hasn't changed, just return existing ID
            if existing_value == property.property_value {
                return Ok(existing_id);
            }

            // Value changed - close out the old property
            self.conn.execute(
                "UPDATE entity_properties SET valid_to = ?1 WHERE id = ?2",
                (&now, existing_id),
            )?;
        }

        // Insert new property value
        self.conn.execute(
            "INSERT INTO entity_properties
             (entity_id, property_name, property_value, source, source_type, confidence, valid_from, valid_to, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
            (
                property.entity_id,
                &property.property_name,
                &property.property_value,
                &property.source,
                &property.source_type,
                property.confidence,
                &now,
                &now,
            ),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get current properties for an entity
    pub fn get_entity_properties(&self, entity_id: i64) -> Result<Vec<EntityProperty>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entity_id, property_name, property_value, source, source_type, confidence, valid_from, valid_to, created_at
             FROM entity_properties
             WHERE entity_id = ?1 AND valid_to IS NULL
             ORDER BY property_name",
        )?;

        let properties = stmt
            .query_map([entity_id], |row| {
                Ok(EntityProperty {
                    id: Some(row.get(0)?),
                    entity_id: row.get(1)?,
                    property_name: row.get(2)?,
                    property_value: row.get(3)?,
                    source: row.get(4)?,
                    source_type: row.get(5)?,
                    confidence: row.get(6)?,
                    valid_from: row.get(7)?,
                    valid_to: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(properties)
    }

    /// Get property history for an entity
    pub fn get_entity_property_history(
        &self,
        entity_id: i64,
        property_name: &str,
    ) -> Result<Vec<EntityProperty>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entity_id, property_name, property_value, source, source_type, confidence, valid_from, valid_to, created_at
             FROM entity_properties
             WHERE entity_id = ?1 AND property_name = ?2
             ORDER BY valid_from DESC",
        )?;

        let properties = stmt
            .query_map([&entity_id.to_string(), property_name], |row| {
                Ok(EntityProperty {
                    id: Some(row.get(0)?),
                    entity_id: row.get(1)?,
                    property_name: row.get(2)?,
                    property_value: row.get(3)?,
                    source: row.get(4)?,
                    source_type: row.get(5)?,
                    confidence: row.get(6)?,
                    valid_from: row.get(7)?,
                    valid_to: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(properties)
    }

    /// Get entities by type
    pub fn get_entities_by_type(
        &self,
        ecosystem: &str,
        project: &str,
        entity_type: &str,
    ) -> Result<Vec<Entity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ecosystem, project, entity_type, entity_name, created_at
             FROM entities
             WHERE ecosystem = ?1 AND project = ?2 AND entity_type = ?3
             ORDER BY entity_name",
        )?;

        let entities = stmt
            .query_map([ecosystem, project, entity_type], |row| {
                Ok(Entity {
                    id: Some(row.get(0)?),
                    ecosystem: row.get(1)?,
                    project: row.get(2)?,
                    entity_type: row.get(3)?,
                    entity_name: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_facts() {
        let mut memory = EcosystemMemory::open(Path::new(":memory:")).unwrap();

        let fact = Fact {
            id: None,
            ecosystem: "test".into(),
            fact: "Uses PostgreSQL".into(),
            source: "config".into(),
            source_type: Some("file".into()),
            category: Some("dependency".into()),
            confidence: 1.0,
            created_at: String::new(),
            updated_at: String::new(),
        };

        memory.add_fact(&fact).unwrap();

        let facts = memory.get_facts("test").unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact, "Uses PostgreSQL");
    }

    #[test]
    fn test_add_and_get_findings() {
        let mut memory = EcosystemMemory::open(Path::new(":memory:")).unwrap();

        let finding = Finding {
            id: None,
            ecosystem: "test".into(),
            project: Some("api".into()),
            category: "bug".into(),
            severity: Some("high".into()),
            description: "N+1 query in user endpoint".into(),
            location: Some("api/users.rs:42".into()),
            workflow_run_id: None,
            status: "open".into(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        memory.add_finding(&finding).unwrap();

        let findings = memory.get_findings("test", None, Some("open")).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].description, "N+1 query in user endpoint");
    }

    #[test]
    fn test_record_workflow_run() {
        let mut memory = EcosystemMemory::open(Path::new(":memory:")).unwrap();

        let run = WorkflowRun {
            id: None,
            ecosystem: "test".into(),
            project: Some("api".into()),
            workflow_name: "bug-hunt".into(),
            success: true,
            duration_ms: Some(5000),
            failed_step: None,
            error_message: None,
            output_dir: Some("/tmp/workflows/bug-hunt-123".into()),
            created_at: String::new(),
        };

        memory.record_run(&run).unwrap();

        let runs = memory.get_recent_runs("test", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].workflow_name, "bug-hunt");
        assert!(runs[0].success);
        assert_eq!(
            runs[0].output_dir,
            Some("/tmp/workflows/bug-hunt-123".into())
        );
    }

    #[test]
    fn test_entity_property_tracking() {
        let mut memory = EcosystemMemory::open(Path::new(":memory:")).unwrap();

        // Create entity
        let entity = Entity {
            id: None,
            ecosystem: "test".into(),
            project: "discourse".into(),
            entity_type: "dependency".into(),
            entity_name: "rails".into(),
            created_at: String::new(),
        };

        let entity_id = memory.get_or_create_entity(&entity).unwrap();

        // Add version property
        let prop1 = EntityProperty {
            id: None,
            entity_id,
            property_name: "version".into(),
            property_value: "8.0".into(),
            source: "Gemfile".into(),
            source_type: Some("file".into()),
            confidence: 1.0,
            valid_from: String::new(),
            valid_to: None,
            created_at: String::new(),
        };

        memory.set_entity_property(&prop1).unwrap();

        // Check current properties
        let properties = memory.get_entity_properties(entity_id).unwrap();
        assert_eq!(properties.len(), 1);
        assert_eq!(properties[0].property_name, "version");
        assert_eq!(properties[0].property_value, "8.0");
        assert!(properties[0].valid_to.is_none());

        // Update version
        let prop2 = EntityProperty {
            id: None,
            entity_id,
            property_name: "version".into(),
            property_value: "8.1".into(),
            source: "Gemfile".into(),
            source_type: Some("file".into()),
            confidence: 1.0,
            valid_from: String::new(),
            valid_to: None,
            created_at: String::new(),
        };

        memory.set_entity_property(&prop2).unwrap();

        // Check current properties show new version
        let properties = memory.get_entity_properties(entity_id).unwrap();
        assert_eq!(properties.len(), 1);
        assert_eq!(properties[0].property_value, "8.1");
        assert!(properties[0].valid_to.is_none());

        // Check history shows both versions
        let history = memory
            .get_entity_property_history(entity_id, "version")
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].property_value, "8.1"); // Most recent first
        assert!(history[0].valid_to.is_none());
        assert_eq!(history[1].property_value, "8.0");
        assert!(history[1].valid_to.is_some()); // Old version is closed out
    }

    #[test]
    fn test_get_entities_by_type() {
        let mut memory = EcosystemMemory::open(Path::new(":memory:")).unwrap();

        // Add multiple dependencies
        let rails = Entity {
            id: None,
            ecosystem: "test".into(),
            project: "discourse".into(),
            entity_type: "dependency".into(),
            entity_name: "rails".into(),
            created_at: String::new(),
        };

        let postgres = Entity {
            id: None,
            ecosystem: "test".into(),
            project: "discourse".into(),
            entity_type: "dependency".into(),
            entity_name: "postgresql".into(),
            created_at: String::new(),
        };

        memory.get_or_create_entity(&rails).unwrap();
        memory.get_or_create_entity(&postgres).unwrap();

        // Query dependencies
        let deps = memory
            .get_entities_by_type("test", "discourse", "dependency")
            .unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|e| e.entity_name == "rails"));
        assert!(deps.iter().any(|e| e.entity_name == "postgresql"));
    }
}
