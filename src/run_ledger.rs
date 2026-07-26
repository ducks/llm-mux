//! Durable workflow run history and resumable step snapshots.

use crate::config::{BackendRun, StepResult, WorkflowConfig};
use crate::workflow::WorkflowResult;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct LedgerRun {
    pub id: i64,
    pub workflow_name: String,
    pub working_dir: PathBuf,
    pub args: HashMap<String, String>,
    pub team: Option<String>,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub output_dir: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub steps: Vec<LedgerStep>,
}

#[derive(Debug)]
pub struct LedgerStep {
    pub name: String,
    pub position: usize,
    pub status: String,
    pub rendered_input: Option<String>,
    pub output: Option<String>,
    pub outputs: HashMap<String, String>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub backend_runs: Vec<BackendRun>,
}

impl LedgerStep {
    pub fn to_step_result(&self) -> StepResult {
        let successful_backends = self
            .backend_runs
            .iter()
            .filter(|run| run.error.is_none())
            .map(|run| run.backend.clone())
            .collect::<Vec<_>>();
        StepResult {
            output: self.output.clone(),
            outputs: self.outputs.clone(),
            failed: self.status != "success",
            error: self.error.clone(),
            duration_ms: self.duration_ms,
            backend: successful_backends.first().cloned(),
            backends: successful_backends,
            rendered_input: self.rendered_input.clone(),
            backend_runs: self.backend_runs.clone(),
        }
    }
}

pub struct RunLedger {
    conn: Connection,
}

impl RunLedger {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create run ledger directory {}", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open run ledger {}", path.display()))?;
        #[cfg(unix)]
        if path != Path::new(":memory:") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to secure run ledger {}", path.display()))?;
        }
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workflow_name TEXT NOT NULL,
                working_dir TEXT NOT NULL,
                args_json TEXT NOT NULL,
                team TEXT,
                status TEXT NOT NULL,
                duration_ms INTEGER,
                error TEXT,
                output_dir TEXT,
                started_at TEXT NOT NULL,
                finished_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);

            CREATE TABLE IF NOT EXISTS run_steps (
                run_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                position INTEGER NOT NULL,
                status TEXT NOT NULL,
                rendered_input TEXT,
                output TEXT,
                outputs_json TEXT NOT NULL,
                error TEXT,
                duration_ms INTEGER NOT NULL,
                PRIMARY KEY (run_id, name),
                FOREIGN KEY(run_id) REFERENCES runs(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS run_backends (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id INTEGER NOT NULL,
                step_name TEXT NOT NULL,
                backend TEXT NOT NULL,
                model TEXT,
                duration_ms INTEGER NOT NULL,
                prompt_tokens INTEGER,
                completion_tokens INTEGER,
                total_tokens INTEGER,
                estimated_cost_usd REAL,
                error TEXT,
                FOREIGN KEY(run_id, step_name)
                    REFERENCES run_steps(run_id, name) ON DELETE CASCADE
            );
            "#,
        )?;
        Ok(Self { conn })
    }

    pub fn default_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("could not determine config directory")?;
        Ok(config_dir.join("llm-mux").join("runs.db"))
    }

    pub fn start_run(
        &mut self,
        workflow_name: &str,
        working_dir: &Path,
        args: &HashMap<String, String>,
        team: Option<&str>,
    ) -> Result<i64> {
        let args_json = serde_json::to_string(args)?;
        self.conn.execute(
            "INSERT INTO runs
             (workflow_name, working_dir, args_json, team, status, started_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
            params![
                workflow_name,
                working_dir.to_string_lossy(),
                args_json,
                team,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn complete_run(
        &mut self,
        run_id: i64,
        workflow: &WorkflowConfig,
        result: &WorkflowResult,
    ) -> Result<()> {
        let transaction = self.conn.transaction()?;
        for (position, step) in workflow.steps.iter().enumerate() {
            let Some(step_result) = result.steps.get(&step.name) else {
                continue;
            };
            let status = if step_result.failed {
                "failed"
            } else if step_result
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("skipped:"))
            {
                "skipped"
            } else {
                "success"
            };
            transaction.execute(
                "INSERT OR REPLACE INTO run_steps
                 (run_id, name, position, status, rendered_input, output, outputs_json,
                  error, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    run_id,
                    step.name,
                    position as i64,
                    status,
                    step_result.rendered_input,
                    step_result.output,
                    serde_json::to_string(&step_result.outputs)?,
                    step_result.error,
                    step_result.duration_ms as i64,
                ],
            )?;
            transaction.execute(
                "DELETE FROM run_backends WHERE run_id = ?1 AND step_name = ?2",
                params![run_id, step.name],
            )?;
            for backend in &step_result.backend_runs {
                transaction.execute(
                    "INSERT INTO run_backends
                     (run_id, step_name, backend, model, duration_ms, prompt_tokens,
                      completion_tokens, total_tokens, estimated_cost_usd, error)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        run_id,
                        step.name,
                        backend.backend,
                        backend.model,
                        backend.duration_ms as i64,
                        backend.prompt_tokens,
                        backend.completion_tokens,
                        backend.total_tokens,
                        backend.estimated_cost_usd,
                        backend.error,
                    ],
                )?;
            }
        }
        transaction.execute(
            "UPDATE runs SET status = ?1, duration_ms = ?2, error = ?3,
             output_dir = ?4, finished_at = ?5 WHERE id = ?6",
            params![
                if result.success { "success" } else { "failed" },
                result.duration.as_millis() as i64,
                result.error,
                result.output_dir,
                chrono::Utc::now().to_rfc3339(),
                run_id,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn fail_run(&mut self, run_id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = 'failed', error = ?1, finished_at = ?2 WHERE id = ?3",
            params![error, chrono::Utc::now().to_rfc3339(), run_id],
        )?;
        Ok(())
    }

    pub fn get_run(&self, run_id: i64) -> Result<Option<LedgerRun>> {
        let run = self
            .conn
            .query_row(
                "SELECT id, workflow_name, working_dir, args_json, team, status,
                 duration_ms, error, output_dir, started_at, finished_at
                 FROM runs WHERE id = ?1",
                [run_id],
                |row| {
                    let args_json: String = row.get(3)?;
                    Ok(LedgerRun {
                        id: row.get(0)?,
                        workflow_name: row.get(1)?,
                        working_dir: PathBuf::from(row.get::<_, String>(2)?),
                        args: serde_json::from_str(&args_json).unwrap_or_default(),
                        team: row.get(4)?,
                        status: row.get(5)?,
                        duration_ms: row.get::<_, Option<i64>>(6)?.map(|value| value as u64),
                        error: row.get(7)?,
                        output_dir: row.get(8)?,
                        started_at: row.get(9)?,
                        finished_at: row.get(10)?,
                        steps: Vec::new(),
                    })
                },
            )
            .optional()?;

        let Some(mut run) = run else {
            return Ok(None);
        };
        let mut statement = self.conn.prepare(
            "SELECT name, position, status, rendered_input, output, outputs_json,
             error, duration_ms FROM run_steps WHERE run_id = ?1 ORDER BY position",
        )?;
        let steps = statement
            .query_map([run_id], |row| {
                let outputs_json: String = row.get(5)?;
                Ok(LedgerStep {
                    name: row.get(0)?,
                    position: row.get::<_, i64>(1)? as usize,
                    status: row.get(2)?,
                    rendered_input: row.get(3)?,
                    output: row.get(4)?,
                    outputs: serde_json::from_str(&outputs_json).unwrap_or_default(),
                    error: row.get(6)?,
                    duration_ms: row.get::<_, i64>(7)? as u64,
                    backend_runs: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        for mut step in steps {
            let mut backend_statement = self.conn.prepare(
                "SELECT backend, model, duration_ms, prompt_tokens, completion_tokens,
                 total_tokens, estimated_cost_usd, error
                 FROM run_backends WHERE run_id = ?1 AND step_name = ?2 ORDER BY id",
            )?;
            step.backend_runs = backend_statement
                .query_map(params![run_id, step.name], |row| {
                    Ok(BackendRun {
                        backend: row.get(0)?,
                        model: row.get(1)?,
                        duration_ms: row.get::<_, i64>(2)? as u64,
                        prompt_tokens: row.get(3)?,
                        completion_tokens: row.get(4)?,
                        total_tokens: row.get(5)?,
                        estimated_cost_usd: row.get(6)?,
                        error: row.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            run.steps.push(step);
        }
        Ok(Some(run))
    }

    pub fn resumable_steps(run: &LedgerRun) -> HashMap<String, StepResult> {
        run.steps
            .iter()
            .filter(|step| step.status == "success")
            .map(|step| (step.name.clone(), step.to_step_result()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;
    use std::time::Duration;

    #[test]
    fn records_and_reloads_complete_run() {
        let mut ledger = RunLedger::open(Path::new(":memory:")).unwrap();
        let workflow = WorkflowConfig {
            name: "review".into(),
            steps: vec![StepConfig {
                name: "analyze".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let args = HashMap::from([("scope".into(), "src".into())]);
        let run_id = ledger
            .start_run("review", Path::new("/tmp/project"), &args, Some("rust"))
            .unwrap();
        let result = WorkflowResult {
            steps: HashMap::from([(
                "analyze".into(),
                StepResult {
                    output: Some("finding".into()),
                    duration_ms: 42,
                    backend_runs: vec![BackendRun {
                        backend: "codex".into(),
                        model: Some("gpt-test".into()),
                        total_tokens: Some(12),
                        estimated_cost_usd: Some(0.01),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )]),
            success: true,
            error: None,
            duration: Duration::from_millis(50),
            team: Some("rust".into()),
            output_dir: Some("/tmp/artifacts".into()),
        };
        ledger.complete_run(run_id, &workflow, &result).unwrap();

        let stored = ledger.get_run(run_id).unwrap().unwrap();
        assert_eq!(stored.status, "success");
        assert_eq!(stored.args["scope"], "src");
        assert_eq!(stored.steps[0].output.as_deref(), Some("finding"));
        assert_eq!(stored.steps[0].backend_runs[0].total_tokens, Some(12));
        assert_eq!(RunLedger::resumable_steps(&stored).len(), 1);
    }

    #[test]
    fn failed_steps_are_not_resumed() {
        let run = LedgerRun {
            id: 1,
            workflow_name: "review".into(),
            working_dir: PathBuf::from("."),
            args: HashMap::new(),
            team: None,
            status: "failed".into(),
            duration_ms: None,
            error: None,
            output_dir: None,
            started_at: String::new(),
            finished_at: None,
            steps: vec![LedgerStep {
                name: "bad".into(),
                position: 0,
                status: "failed".into(),
                rendered_input: None,
                output: None,
                outputs: HashMap::new(),
                error: Some("nope".into()),
                duration_ms: 1,
                backend_runs: Vec::new(),
            }],
        };
        assert!(RunLedger::resumable_steps(&run).is_empty());
    }
}
