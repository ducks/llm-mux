//! Rollback strategies for undoing file changes

use super::diff_applier::ModifiedFile;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::process::Command;

/// Errors during rollback
#[derive(Debug, Error)]
pub enum RollbackError {
    #[error("git rollback failed for {path}: {message}")]
    GitFailed { path: PathBuf, message: String },

    #[error("backup restore failed for {path}: {source}")]
    BackupRestoreFailed { path: PathBuf, source: io::Error },

    #[error("backup file not found: {path}")]
    BackupNotFound { path: PathBuf },

    #[error("partial rollback: {succeeded} files restored, {failed} failed")]
    PartialRollback { succeeded: usize, failed: usize },
}

/// Rollback strategy configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RollbackStrategy {
    /// Restore each touched file from the backup taken before the run.
    /// This is the default: it returns files to their exact pre-run state,
    /// preserving any uncommitted work and untracked files.
    #[default]
    Backup,
    /// Restore files with `git checkout -- <file>`.
    ///
    /// WARNING: this restores to HEAD, not to the pre-run working-tree state,
    /// so it discards uncommitted edits the user had in the touched files
    /// before the run. Prefer `Backup`. Opt in only when you specifically want
    /// a checkout-to-HEAD and know the tree was clean.
    Git,
    /// Don't rollback (for debugging)
    None,
}

impl RollbackStrategy {
    /// Parse from string. Unrecognized values fall back to the safe default
    /// (`Backup`) rather than the lossy `Git` checkout.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "git" => Self::Git,
            "backup" => Self::Backup,
            "none" => Self::None,
            _ => Self::default(),
        }
    }
}

/// Result of a rollback operation
#[derive(Debug)]
pub struct RollbackResult {
    /// Files that were successfully restored
    pub restored: Vec<PathBuf>,
    /// Files that failed to restore with error messages
    pub failed: Vec<(PathBuf, String)>,
}

impl RollbackResult {
    /// Check if all files were restored
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Perform rollback using the specified strategy
pub async fn rollback(
    modified_files: &[ModifiedFile],
    created_files: &[PathBuf],
    strategy: RollbackStrategy,
    working_dir: &Path,
) -> Result<RollbackResult, RollbackError> {
    match strategy {
        RollbackStrategy::Git => rollback_git(modified_files, created_files, working_dir).await,
        RollbackStrategy::Backup => rollback_backup(modified_files, created_files).await,
        RollbackStrategy::None => Ok(RollbackResult {
            restored: Vec::new(),
            failed: Vec::new(),
        }),
    }
}

/// Rollback using git checkout
async fn rollback_git(
    modified_files: &[ModifiedFile],
    created_files: &[PathBuf],
    working_dir: &Path,
) -> Result<RollbackResult, RollbackError> {
    let mut result = RollbackResult {
        restored: Vec::new(),
        failed: Vec::new(),
    };

    // Restore modified files
    // A file can be edited more than once in one batch. Restore the newest
    // backup first so the oldest backup, containing the original user state,
    // is applied last.
    for file in modified_files.iter().rev() {
        let relative_path = file.path.strip_prefix(working_dir).unwrap_or(&file.path);

        let output = Command::new("git")
            .args(["checkout", "--"])
            .arg(relative_path)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                result.restored.push(file.path.clone());
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                result.failed.push((file.path.clone(), stderr.to_string()));
            }
            Err(e) => {
                result.failed.push((file.path.clone(), e.to_string()));
            }
        }
    }

    // Remove created files
    for path in created_files.iter().rev() {
        match fs::remove_file(path) {
            Ok(_) => {
                result.restored.push(path.clone());
            }
            Err(e) => {
                result.failed.push((path.clone(), e.to_string()));
            }
        }
    }

    if !result.failed.is_empty() {
        return Err(RollbackError::PartialRollback {
            succeeded: result.restored.len(),
            failed: result.failed.len(),
        });
    }

    Ok(result)
}

/// Rollback using backup files
async fn rollback_backup(
    modified_files: &[ModifiedFile],
    created_files: &[PathBuf],
) -> Result<RollbackResult, RollbackError> {
    let mut result = RollbackResult {
        restored: Vec::new(),
        failed: Vec::new(),
    };

    // Restore from backups
    // Restore repeated edits newest-to-oldest so the original pre-run backup
    // is the final state written to disk.
    for file in modified_files.iter().rev() {
        if !file.backup_path.exists() {
            result.failed.push((
                file.path.clone(),
                format!("backup not found: {:?}", file.backup_path),
            ));
            continue;
        }

        match fs::copy(&file.backup_path, &file.path) {
            Ok(_) => {
                result.restored.push(file.path.clone());
                // Clean up backup file
                let _ = fs::remove_file(&file.backup_path);
            }
            Err(e) => {
                result.failed.push((file.path.clone(), e.to_string()));
            }
        }
    }

    // Remove created files
    for path in created_files {
        match fs::remove_file(path) {
            Ok(_) => {
                result.restored.push(path.clone());
            }
            Err(e) => {
                result.failed.push((path.clone(), e.to_string()));
            }
        }
    }

    // Any failure is worth surfacing - a rollback that couldn't restore some
    // (or all) files left the tree in a mixed state the caller must know about.
    if !result.failed.is_empty() {
        return Err(RollbackError::PartialRollback {
            succeeded: result.restored.len(),
            failed: result.failed.len(),
        });
    }

    Ok(result)
}

/// Clean up backup files after successful verification
pub fn cleanup_backups(modified_files: &[ModifiedFile]) {
    for file in modified_files {
        let _ = fs::remove_file(&file.backup_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_rollback_strategy_from_str() {
        assert_eq!(RollbackStrategy::from_str("git"), RollbackStrategy::Git);
        assert_eq!(
            RollbackStrategy::from_str("backup"),
            RollbackStrategy::Backup
        );
        assert_eq!(RollbackStrategy::from_str("none"), RollbackStrategy::None);
        // Unknown falls back to the safe default, not the lossy Git checkout.
        assert_eq!(
            RollbackStrategy::from_str("unknown"),
            RollbackStrategy::Backup
        );
    }

    #[tokio::test]
    async fn test_rollback_backup() {
        let dir = TempDir::new().unwrap();
        let backup_dir = dir.path().join("backups");
        fs::create_dir(&backup_dir).unwrap();

        // Create original and backup
        let original_path = setup_test_file(dir.path(), "test.rs", "modified content");
        let backup_path = setup_test_file(&backup_dir, "test.rs.backup", "original content");

        let modified = ModifiedFile {
            path: original_path.clone(),
            backup_path,
        };

        let result = rollback_backup(&[modified], &[]).await.unwrap();

        assert_eq!(result.restored.len(), 1);
        assert!(result.failed.is_empty());

        let content = fs::read_to_string(&original_path).unwrap();
        assert_eq!(content, "original content");
    }

    #[tokio::test]
    async fn test_rollback_backup_restores_repeated_edits_in_reverse_order() {
        let dir = TempDir::new().unwrap();
        let path = setup_test_file(dir.path(), "test.rs", "second edit");
        let original_backup = setup_test_file(dir.path(), "original.bak", "original");
        let first_edit_backup = setup_test_file(dir.path(), "first-edit.bak", "first edit");
        let modified = vec![
            ModifiedFile {
                path: path.clone(),
                backup_path: original_backup,
            },
            ModifiedFile {
                path: path.clone(),
                backup_path: first_edit_backup,
            },
        ];

        rollback_backup(&modified, &[]).await.unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "original");
    }

    #[tokio::test]
    async fn test_rollback_none() {
        let dir = TempDir::new().unwrap();
        let path = setup_test_file(dir.path(), "test.rs", "modified");

        let modified = ModifiedFile {
            path: path.clone(),
            backup_path: dir.path().join("backup"),
        };

        let result = rollback(&[modified], &[], RollbackStrategy::None, dir.path())
            .await
            .unwrap();

        // None strategy should not restore anything
        assert!(result.restored.is_empty());
        assert!(result.failed.is_empty());

        // File should still have modified content
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "modified");
    }

    #[tokio::test]
    async fn test_rollback_removes_created_files() {
        let dir = TempDir::new().unwrap();
        let created_path = setup_test_file(dir.path(), "new.rs", "new content");

        let result = rollback_backup(&[], std::slice::from_ref(&created_path))
            .await
            .unwrap();

        assert_eq!(result.restored.len(), 1);
        assert!(!created_path.exists());
    }

    #[test]
    fn test_cleanup_backups() {
        let dir = TempDir::new().unwrap();
        let backup_path = setup_test_file(dir.path(), "backup", "content");

        let modified = ModifiedFile {
            path: dir.path().join("original"),
            backup_path: backup_path.clone(),
        };

        assert!(backup_path.exists());
        cleanup_backups(&[modified]);
        assert!(!backup_path.exists());
    }

    #[test]
    fn test_rollback_result_is_complete() {
        let complete = RollbackResult {
            restored: vec![PathBuf::from("a")],
            failed: vec![],
        };
        assert!(complete.is_complete());

        let partial = RollbackResult {
            restored: vec![PathBuf::from("a")],
            failed: vec![(PathBuf::from("b"), "error".to_string())],
        };
        assert!(!partial.is_complete());
    }
}
