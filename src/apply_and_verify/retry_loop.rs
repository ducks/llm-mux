//! Apply-verify-rollback-retry orchestration

use super::diff_applier::{ApplyError, ApplyResult, DiffApplier, ModifiedFile};
use super::edit_parser::{EditParseError, parse_edits};
use super::rollback::{RollbackError, RollbackStrategy, cleanup_backups, rollback};
use super::verification::{VerifyError, VerifyResult, run_verify_cancellable};
use std::path::Path;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Errors during apply-verify cycle
#[derive(Debug, Error)]
pub enum ApplyVerifyError {
    #[error("edit parsing failed: {0}")]
    ParseError(#[from] EditParseError),

    #[error("apply failed: {0}")]
    ApplyError(#[from] ApplyError),

    #[error("verification failed: {0}")]
    VerifyError(#[from] VerifyError),

    #[error("rollback failed: {0}")]
    RollbackError(#[from] RollbackError),

    #[error("verification failed after {attempts} attempts")]
    MaxRetriesExceeded { attempts: u32 },

    #[error("apply-verify cycle timed out after {0:?}")]
    Timeout(Duration),

    #[error("source step output not found: {step}")]
    SourceNotFound { step: String },

    #[error(
        "verification retries require another LLM query and are not supported by apply_and_verify"
    )]
    UnsupportedRetries,
}

/// Configuration for apply-verify cycle
#[derive(Debug, Clone)]
pub struct ApplyVerifyConfig {
    /// Source step name to get edits from
    pub source_step: String,
    /// Verification command to run
    pub verify_command: Option<String>,
    /// Number of retry attempts on verification failure
    pub verify_retries: u32,
    /// Rollback strategy
    pub rollback_strategy: RollbackStrategy,
    /// Timeout for entire cycle
    pub timeout: Option<Duration>,
    /// Timeout for verification command
    pub verify_timeout: Option<Duration>,
    /// Prompt template for retry queries
    pub retry_prompt: Option<String>,
    /// Cancellation shared with the owning workflow.
    pub cancellation: Option<tokio_util::sync::CancellationToken>,
}

impl Default for ApplyVerifyConfig {
    fn default() -> Self {
        Self {
            source_step: String::new(),
            verify_command: None,
            verify_retries: 0,
            rollback_strategy: RollbackStrategy::default(),
            timeout: None,
            verify_timeout: Some(Duration::from_secs(300)), // 5 minute default
            retry_prompt: None,
            cancellation: None,
        }
    }
}

/// Result of a single apply-verify attempt
#[derive(Debug)]
pub struct AttemptResult {
    /// Attempt number (1-indexed)
    pub attempt: u32,
    /// Files that were modified
    pub modified_files: Vec<ModifiedFile>,
    /// Files that were created
    pub created_files: Vec<std::path::PathBuf>,
    /// Verification result if run
    pub verify_result: Option<VerifyResult>,
    /// Whether this attempt succeeded
    pub success: bool,
    /// Duration of this attempt
    pub duration: Duration,
}

/// Final result of apply-verify cycle
#[derive(Debug)]
pub struct ApplyVerifyResult {
    /// Whether the cycle succeeded
    pub success: bool,
    /// All attempt results
    pub attempts: Vec<AttemptResult>,
    /// Final output (from successful verification or last attempt)
    pub output: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
    /// Total duration
    pub total_duration: Duration,
}

impl ApplyVerifyResult {
    /// Get the number of attempts made
    pub fn attempt_count(&self) -> u32 {
        self.attempts.len() as u32
    }
}

/// Run the apply-verify-retry cycle
pub async fn apply_and_verify(
    source_output: &str,
    config: &ApplyVerifyConfig,
    working_dir: &Path,
) -> Result<ApplyVerifyResult, ApplyVerifyError> {
    if config.verify_retries > 0 {
        return Err(ApplyVerifyError::UnsupportedRetries);
    }

    let start = Instant::now();
    let mut attempts = Vec::new();
    let mut current_output = source_output.to_string();
    let max_attempts = config.verify_retries + 1;

    for attempt_num in 1..=max_attempts {
        let attempt_start = Instant::now();

        // Parse edits from output
        let edits = parse_edits(&current_output)?;

        // Apply edits
        let applier = DiffApplier::new(working_dir);
        let apply_result = applier.apply(&edits)?;

        // Run verification if configured
        let verify_result = if let Some(ref verify_cmd) = config.verify_command {
            match run_verify_cancellable(
                verify_cmd,
                working_dir,
                config.verify_timeout,
                config.cancellation.as_ref(),
            )
            .await
            {
                Ok(result) => Some(result),
                Err(error) => {
                    rollback(
                        &apply_result.modified_files,
                        &apply_result.created_files,
                        config.rollback_strategy,
                        working_dir,
                    )
                    .await?;
                    return Err(error.into());
                }
            }
        } else {
            None
        };

        let success = verify_result.as_ref().is_none_or(|r| r.success);
        let attempt_duration = attempt_start.elapsed();

        let attempt = AttemptResult {
            attempt: attempt_num,
            modified_files: apply_result.modified_files.clone(),
            created_files: apply_result.created_files.clone(),
            verify_result: verify_result.clone(),
            success,
            duration: attempt_duration,
        };

        attempts.push(attempt);

        if success {
            // Success! Clean up backups and return
            cleanup_backups(&apply_result.modified_files);

            return Ok(ApplyVerifyResult {
                success: true,
                attempts,
                output: verify_result.map(|r| r.stdout),
                error: None,
                total_duration: start.elapsed(),
            });
        }

        // Verification failed - rollback and maybe retry
        rollback(
            &apply_result.modified_files,
            &apply_result.created_files,
            config.rollback_strategy,
            working_dir,
        )
        .await?;

        if attempt_num < max_attempts {
            // Prepare retry prompt with error context
            let error_context = verify_result
                .as_ref()
                .map(|r| r.combined_output())
                .unwrap_or_default();

            current_output = build_retry_prompt(
                &current_output,
                &error_context,
                config.retry_prompt.as_deref(),
            );
        }
    }

    // All attempts failed
    let _last_error = attempts
        .last()
        .and_then(|a| a.verify_result.as_ref())
        .map(|r| r.combined_output())
        .unwrap_or_else(|| "verification failed".to_string());

    Err(ApplyVerifyError::MaxRetriesExceeded {
        attempts: max_attempts,
    })
}

/// Build retry prompt with error context
fn build_retry_prompt(original: &str, error_context: &str, template: Option<&str>) -> String {
    if let Some(tmpl) = template {
        tmpl.replace("{{ original }}", original)
            .replace("{{ error }}", error_context)
    } else {
        format!(
            "The previous edit attempt failed verification.\n\n\
             Original edits:\n{}\n\n\
             Verification error:\n{}\n\n\
             Please provide corrected edits.",
            original, error_context
        )
    }
}

/// Convenience function for simple apply without verification
pub async fn apply_only(
    source_output: &str,
    working_dir: &Path,
) -> Result<ApplyResult, ApplyVerifyError> {
    let edits = parse_edits(source_output)?;
    let applier = DiffApplier::new(working_dir);
    let result = applier.apply(&edits)?;
    cleanup_backups(&result.modified_files);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_file(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn test_apply_verify_success() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "fn old() {}");

        let source_output = r#"{"path": "test.rs", "old": "fn old() {}", "new": "fn new() {}"}"#;

        let config = ApplyVerifyConfig {
            source_step: "test".into(),
            verify_command: Some("true".into()), // Always succeeds
            ..Default::default()
        };

        let result = apply_and_verify(source_output, &config, dir.path())
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.attempt_count(), 1);

        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("fn new()"));
    }

    #[tokio::test]
    async fn test_apply_only() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "fn old() {}");

        let source_output = r#"{"path": "test.rs", "old": "fn old() {}", "new": "fn new() {}"}"#;

        let result = apply_only(source_output, dir.path()).await.unwrap();

        assert_eq!(result.modified_files.len(), 1);

        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("fn new()"));
    }

    #[tokio::test]
    async fn test_apply_verify_failure_no_retry() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "fn old() {}");

        let source_output = r#"{"path": "test.rs", "old": "fn old() {}", "new": "fn new() {}"}"#;

        let config = ApplyVerifyConfig {
            source_step: "test".into(),
            verify_command: Some("false".into()), // Always fails
            verify_retries: 0,
            rollback_strategy: RollbackStrategy::Backup,
            ..Default::default()
        };

        let result = apply_and_verify(source_output, &config, dir.path()).await;

        assert!(matches!(
            result,
            Err(ApplyVerifyError::MaxRetriesExceeded { .. })
        ));

        // File should be rolled back
        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("fn old()"));
    }

    /// Initialize a git repo in `dir`, commit `path` with `committed`, then
    /// overwrite it with `uncommitted` (leaving dirty, unstaged work). Returns
    /// once the tree is in that state. Skips (returns false) if git is absent.
    fn git_repo_with_uncommitted(
        dir: &Path,
        path: &str,
        committed: &str,
        uncommitted: &str,
    ) -> bool {
        use std::process::Command;
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return false;
        }
        let _ = git(&["config", "user.email", "t@t"]);
        let _ = git(&["config", "user.name", "t"]);
        fs::write(dir.join(path), committed).unwrap();
        if !git(&["add", path]) || !git(&["commit", "-qm", "init"]) {
            return false;
        }
        // The user's in-flight, uncommitted edit.
        fs::write(dir.join(path), uncommitted).unwrap();
        true
    }

    #[tokio::test]
    async fn test_backup_rollback_preserves_uncommitted_user_work() {
        let dir = TempDir::new().unwrap();
        // Committed "A", user edited to "fn user_wip() {}" without committing.
        if !git_repo_with_uncommitted(
            dir.path(),
            "test.rs",
            "fn committed() {}",
            "fn user_wip() {}",
        ) {
            eprintln!("git unavailable; skipping");
            return;
        }

        // llm-mux edits the user's in-flight version, then verify fails.
        let source_output =
            r#"{"path": "test.rs", "old": "fn user_wip() {}", "new": "fn llm_edit() {}"}"#;
        let config = ApplyVerifyConfig {
            source_step: "test".into(),
            verify_command: Some("false".into()),
            verify_retries: 0,
            rollback_strategy: RollbackStrategy::Backup,
            ..Default::default()
        };

        let result = apply_and_verify(source_output, &config, dir.path()).await;
        assert!(matches!(
            result,
            Err(ApplyVerifyError::MaxRetriesExceeded { .. })
        ));

        // Must restore the user's uncommitted work, NOT git's committed HEAD.
        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert_eq!(
            content, "fn user_wip() {}",
            "backup rollback must restore the user's uncommitted state, not HEAD"
        );
    }

    #[tokio::test]
    async fn test_git_rollback_loses_uncommitted_work_regression_guard() {
        // Documents exactly why Backup is the default: the Git strategy
        // restores to HEAD, discarding the user's uncommitted edit. If this
        // ever stops being true (git rollback learns to preserve the tree),
        // revisit the default.
        let dir = TempDir::new().unwrap();
        if !git_repo_with_uncommitted(
            dir.path(),
            "test.rs",
            "fn committed() {}",
            "fn user_wip() {}",
        ) {
            eprintln!("git unavailable; skipping");
            return;
        }

        let source_output =
            r#"{"path": "test.rs", "old": "fn user_wip() {}", "new": "fn llm_edit() {}"}"#;
        let config = ApplyVerifyConfig {
            source_step: "test".into(),
            verify_command: Some("false".into()),
            verify_retries: 0,
            rollback_strategy: RollbackStrategy::Git,
            ..Default::default()
        };

        let _ = apply_and_verify(source_output, &config, dir.path()).await;

        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert_eq!(
            content, "fn committed() {}",
            "git rollback restores to HEAD (this is the data-loss the Backup default avoids)"
        );
    }

    #[tokio::test]
    async fn test_apply_no_verify() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "fn old() {}");

        let source_output = r#"{"path": "test.rs", "old": "fn old() {}", "new": "fn new() {}"}"#;

        let config = ApplyVerifyConfig {
            source_step: "test".into(),
            verify_command: None, // No verification
            ..Default::default()
        };

        let result = apply_and_verify(source_output, &config, dir.path())
            .await
            .unwrap();

        assert!(result.success);

        let content = fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("fn new()"));
    }

    #[test]
    fn test_build_retry_prompt_default() {
        let prompt = build_retry_prompt("original edits", "error message", None);
        assert!(prompt.contains("original edits"));
        assert!(prompt.contains("error message"));
    }

    #[test]
    fn test_build_retry_prompt_custom() {
        let template = "Fix this: {{ error }}\nBased on: {{ original }}";
        let prompt = build_retry_prompt("edits", "error", Some(template));
        assert_eq!(prompt, "Fix this: error\nBased on: edits");
    }

    #[test]
    fn test_config_default() {
        let config = ApplyVerifyConfig::default();
        assert_eq!(config.verify_retries, 0);
        // Default rollback preserves the user's pre-run state (see
        // RollbackStrategy docs); Git-checkout rollback is opt-in only.
        assert_eq!(config.rollback_strategy, RollbackStrategy::Backup);
        assert!(config.verify_timeout.is_some());
    }

    #[tokio::test]
    async fn test_apply_verify_result_helpers() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "content");

        let source_output = r#"{"path": "test.rs", "content": "new content"}"#;

        let config = ApplyVerifyConfig {
            verify_command: Some("true".into()),
            ..Default::default()
        };

        let result = apply_and_verify(source_output, &config, dir.path())
            .await
            .unwrap();

        assert_eq!(result.attempt_count(), 1);
    }
}
