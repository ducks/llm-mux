//! Database schema for ecosystem memory

use rusqlite::Connection;
use anyhow::Result;

/// Initialize the database schema
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS facts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ecosystem TEXT NOT NULL,
            fact TEXT NOT NULL,
            source TEXT NOT NULL,
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(ecosystem, fact, source)
        );

        CREATE INDEX IF NOT EXISTS idx_facts_ecosystem ON facts(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_facts_source ON facts(source);

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
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_workflow_runs_ecosystem ON workflow_runs(ecosystem);
        CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow ON workflow_runs(workflow_name);
        CREATE INDEX IF NOT EXISTS idx_workflow_runs_created ON workflow_runs(created_at);
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
    }
}
