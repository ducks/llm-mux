//! Database schema for ecosystem memory

use anyhow::Result;
use rusqlite::Connection;

/// Initialize the database schema
#[allow(dead_code)]
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS facts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ecosystem TEXT NOT NULL,
            fact TEXT NOT NULL,
            source TEXT NOT NULL,
            source_type TEXT,
            category TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(ecosystem, fact, source)
        );

        CREATE INDEX IF NOT EXISTS idx_facts_ecosystem ON facts(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_facts_source ON facts(source);
        CREATE INDEX IF NOT EXISTS idx_facts_category ON facts(category);
        CREATE INDEX IF NOT EXISTS idx_facts_source_type ON facts(source_type);

        CREATE TABLE IF NOT EXISTS project_relationships (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ecosystem TEXT NOT NULL,
            from_project TEXT NOT NULL,
            to_project TEXT NOT NULL,
            relationship_type TEXT NOT NULL,
            metadata TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(ecosystem, from_project, to_project, relationship_type)
        );

        CREATE INDEX IF NOT EXISTS idx_relationships_ecosystem ON project_relationships(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_relationships_from ON project_relationships(from_project);
        CREATE INDEX IF NOT EXISTS idx_relationships_to ON project_relationships(to_project);

        CREATE TABLE IF NOT EXISTS findings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ecosystem TEXT NOT NULL,
            project TEXT,
            category TEXT NOT NULL,
            severity TEXT,
            description TEXT NOT NULL,
            location TEXT,
            workflow_run_id INTEGER,
            status TEXT NOT NULL DEFAULT 'open',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(workflow_run_id) REFERENCES workflow_runs(id)
        );

        CREATE INDEX IF NOT EXISTS idx_findings_ecosystem ON findings(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_findings_project ON findings(project);
        CREATE INDEX IF NOT EXISTS idx_findings_status ON findings(status);
        CREATE INDEX IF NOT EXISTS idx_findings_category ON findings(category);

        CREATE TABLE IF NOT EXISTS workflow_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ecosystem TEXT NOT NULL,
            project TEXT,
            workflow_name TEXT NOT NULL,
            success INTEGER NOT NULL,
            duration_ms INTEGER,
            failed_step TEXT,
            error_message TEXT,
            output_dir TEXT,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_workflow_runs_ecosystem ON workflow_runs(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow ON workflow_runs(workflow_name);
        CREATE INDEX IF NOT EXISTS idx_workflow_runs_created ON workflow_runs(created_at);

        -- Normalized entity storage with history tracking
        CREATE TABLE IF NOT EXISTS entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ecosystem TEXT NOT NULL,
            project TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            entity_name TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(ecosystem, project, entity_type, entity_name)
        );

        CREATE INDEX IF NOT EXISTS idx_entities_ecosystem ON entities(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_entities_project ON entities(project);
        CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
        CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(entity_name);

        CREATE TABLE IF NOT EXISTS entity_properties (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entity_id INTEGER NOT NULL,
            property_name TEXT NOT NULL,
            property_value TEXT NOT NULL,
            source TEXT NOT NULL,
            source_type TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            valid_from TEXT NOT NULL,
            valid_to TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(entity_id, property_name, valid_from),
            FOREIGN KEY(entity_id) REFERENCES entities(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_properties_entity ON entity_properties(entity_id);
        CREATE INDEX IF NOT EXISTS idx_properties_name ON entity_properties(property_name);
        CREATE INDEX IF NOT EXISTS idx_properties_current ON entity_properties(entity_id, property_name, valid_to);
        CREATE INDEX IF NOT EXISTS idx_properties_valid_from ON entity_properties(valid_from);
        "#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_schema() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(tables.contains(&"facts".to_string()));
        assert!(tables.contains(&"project_relationships".to_string()));
        assert!(tables.contains(&"findings".to_string()));
        assert!(tables.contains(&"workflow_runs".to_string()));
        assert!(tables.contains(&"entities".to_string()));
        assert!(tables.contains(&"entity_properties".to_string()));
    }
}
