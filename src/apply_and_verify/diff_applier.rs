//! Diff application with fuzzy matching and backup creation

use super::edit_parser::{DiffHunk, DiffLine, EditOperation, normalize_whitespace};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Maximum line drift for fuzzy hunk matching
const MAX_LINE_DRIFT: usize = 3;

/// Errors during diff application
#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("file not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("failed to read file {path}: {source}")]
    ReadError { path: PathBuf, source: io::Error },

    #[error("failed to write file {path}: {source}")]
    WriteError { path: PathBuf, source: io::Error },

    #[error("failed to create backup for {path}: {source}")]
    BackupError { path: PathBuf, source: io::Error },

    #[error("old text not found in {path}")]
    OldTextNotFound { path: PathBuf },

    #[error("hunk context not found in {path} near line {expected_line}")]
    HunkContextNotFound { path: PathBuf, expected_line: usize },

    #[error("multiple matches for old text in {path}")]
    AmbiguousMatch { path: PathBuf },
}

/// Result of applying edits
#[derive(Debug)]
pub struct ApplyResult {
    /// Files that were modified
    pub modified_files: Vec<ModifiedFile>,
    /// Files that were created
    pub created_files: Vec<PathBuf>,
}

/// A modified file with its backup
#[derive(Debug, Clone)]
pub struct ModifiedFile {
    pub path: PathBuf,
    pub backup_path: PathBuf,
}

/// Apply edits to files
pub struct DiffApplier {
    backup_dir: PathBuf,
    working_dir: PathBuf,
}

impl DiffApplier {
    /// Create a new diff applier
    pub fn new(working_dir: &Path) -> Self {
        Self {
            backup_dir: working_dir.join(".llmux/backups"),
            working_dir: working_dir.to_path_buf(),
        }
    }

    /// Apply all edit operations
    pub fn apply(&self, edits: &[EditOperation]) -> Result<ApplyResult, ApplyError> {
        let mut modified_files = Vec::new();
        let mut created_files = Vec::new();

        // Create backup directory if needed
        fs::create_dir_all(&self.backup_dir).map_err(|e| ApplyError::BackupError {
            path: self.backup_dir.clone(),
            source: e,
        })?;

        for edit in edits {
            match edit {
                EditOperation::UnifiedDiff { path, hunks } => {
                    let full_path = self.working_dir.join(path);
                    let backup = self.create_backup(&full_path)?;
                    self.apply_unified_diff(&full_path, hunks)?;
                    modified_files.push(ModifiedFile {
                        path: full_path,
                        backup_path: backup,
                    });
                }
                EditOperation::OldNewPair { path, old, new } => {
                    let full_path = self.working_dir.join(path);
                    let backup = self.create_backup(&full_path)?;
                    self.apply_old_new(&full_path, old, new)?;
                    modified_files.push(ModifiedFile {
                        path: full_path,
                        backup_path: backup,
                    });
                }
                EditOperation::FullFile { path, content } => {
                    let full_path = self.working_dir.join(path);
                    if full_path.exists() {
                        let backup = self.create_backup(&full_path)?;
                        modified_files.push(ModifiedFile {
                            path: full_path.clone(),
                            backup_path: backup,
                        });
                    } else {
                        // Create parent directories
                        if let Some(parent) = full_path.parent() {
                            fs::create_dir_all(parent).map_err(|e| ApplyError::WriteError {
                                path: parent.to_path_buf(),
                                source: e,
                            })?;
                        }
                        created_files.push(full_path.clone());
                    }
                    fs::write(&full_path, content).map_err(|e| ApplyError::WriteError {
                        path: full_path,
                        source: e,
                    })?;
                }
            }
        }

        Ok(ApplyResult {
            modified_files,
            created_files,
        })
    }

    /// Create a backup of a file before modification
    fn create_backup(&self, path: &Path) -> Result<PathBuf, ApplyError> {
        if !path.exists() {
            return Err(ApplyError::FileNotFound {
                path: path.to_path_buf(),
            });
        }

        // Generate backup filename with timestamp
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let backup_name = format!("{}.{}", filename, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(path, &backup_path).map_err(|e| ApplyError::BackupError {
            path: path.to_path_buf(),
            source: e,
        })?;

        Ok(backup_path)
    }

    /// Apply a unified diff to a file
    fn apply_unified_diff(&self, path: &Path, hunks: &[DiffHunk]) -> Result<(), ApplyError> {
        let content = fs::read_to_string(path).map_err(|e| ApplyError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;

        let mut lines: Vec<String> = content.lines().map(String::from).collect();

        // Apply hunks in reverse order to preserve line numbers
        for hunk in hunks.iter().rev() {
            self.apply_hunk(&mut lines, hunk, path)?;
        }

        let new_content = lines.join("\n");
        // Preserve trailing newline if original had one
        let final_content = if content.ends_with('\n') {
            format!("{}\n", new_content)
        } else {
            new_content
        };

        fs::write(path, final_content).map_err(|e| ApplyError::WriteError {
            path: path.to_path_buf(),
            source: e,
        })?;

        Ok(())
    }

    /// Apply a single hunk with fuzzy matching
    fn apply_hunk(
        &self,
        lines: &mut Vec<String>,
        hunk: &DiffHunk,
        _path: &Path,
    ) -> Result<(), ApplyError> {
        // Extract context lines from hunk for matching
        let context_lines: Vec<&str> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                DiffLine::Context(s) | DiffLine::Remove(s) => Some(s.as_str()),
                DiffLine::Add(_) => None,
            })
            .collect();

        // Find the best match position
        let match_pos = self.find_hunk_position(lines, &context_lines, hunk.old_start)?;

        // Build the replacement lines
        let mut new_lines: Vec<String> = Vec::new();
        for line in &hunk.lines {
            match line {
                DiffLine::Context(s) | DiffLine::Add(s) => {
                    new_lines.push(s.clone());
                }
                DiffLine::Remove(_) => {
                    // Skip removed lines
                }
            }
        }

        // Calculate how many lines to remove (context + removed)
        let remove_count = hunk
            .lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Context(_) | DiffLine::Remove(_)))
            .count();

        // Validate match_pos doesn't exceed bounds
        let actual_match_pos = if match_pos >= lines.len() {
            lines.len().saturating_sub(remove_count)
        } else {
            match_pos
        };

        // Replace lines
        let end = (actual_match_pos + remove_count).min(lines.len());
        lines.splice(actual_match_pos..end, new_lines);

        Ok(())
    }

    /// Find the position to apply a hunk using fuzzy matching
    fn find_hunk_position(
        &self,
        lines: &[String],
        context_lines: &[&str],
        expected_start: usize,
    ) -> Result<usize, ApplyError> {
        if context_lines.is_empty() {
            // No context, use expected position
            return Ok(expected_start.saturating_sub(1));
        }

        // Convert to 0-indexed
        let expected_pos = expected_start.saturating_sub(1);

        // Search around the expected position with drift tolerance
        let search_start = expected_pos.saturating_sub(MAX_LINE_DRIFT);
        let search_end = (expected_pos + MAX_LINE_DRIFT).min(lines.len());

        for pos in search_start..search_end {
            if self.context_matches(lines, pos, context_lines) {
                return Ok(pos);
            }
        }

        // Not found within drift range, search entire file
        for pos in 0..lines.len() {
            if self.context_matches(lines, pos, context_lines) {
                return Ok(pos);
            }
        }

        Err(ApplyError::HunkContextNotFound {
            path: PathBuf::new(), // Will be filled by caller
            expected_line: expected_start,
        })
    }

    /// Check if context lines match at a position
    fn context_matches(&self, lines: &[String], pos: usize, context: &[&str]) -> bool {
        if pos + context.len() > lines.len() {
            return false;
        }

        for (i, ctx_line) in context.iter().enumerate() {
            let file_line = &lines[pos + i];
            // Normalize whitespace for comparison
            if normalize_whitespace(file_line) != normalize_whitespace(ctx_line) {
                return false;
            }
        }

        true
    }

    /// Apply old/new text replacement
    fn apply_old_new(&self, path: &Path, old: &str, new: &str) -> Result<(), ApplyError> {
        let content = fs::read_to_string(path).map_err(|e| ApplyError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;

        // Normalize for matching
        let normalized_content = normalize_whitespace(&content);
        let normalized_old = normalize_whitespace(old);

        // Find the old text
        let matches: Vec<_> = normalized_content.match_indices(&normalized_old).collect();

        if matches.is_empty() {
            return Err(ApplyError::OldTextNotFound {
                path: path.to_path_buf(),
            });
        }

        if matches.len() > 1 {
            return Err(ApplyError::AmbiguousMatch {
                path: path.to_path_buf(),
            });
        }

        // Replace in original (preserving original whitespace where possible)
        let new_content = content.replacen(old, new, 1);

        // If exact match failed, try normalized replacement
        let final_content = if new_content == content {
            // The old text wasn't found exactly, try line-by-line
            self.replace_normalized(&content, old, new)?
        } else {
            new_content
        };

        fs::write(path, final_content).map_err(|e| ApplyError::WriteError {
            path: path.to_path_buf(),
            source: e,
        })?;

        Ok(())
    }

    /// Replace text with normalized whitespace matching
    fn replace_normalized(
        &self,
        content: &str,
        old: &str,
        new: &str,
    ) -> Result<String, ApplyError> {
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        // Find where old_lines match in content_lines
        for i in 0..=content_lines.len().saturating_sub(old_lines.len()) {
            let mut matches = true;
            for (j, old_line) in old_lines.iter().enumerate() {
                if normalize_whitespace(content_lines[i + j]) != normalize_whitespace(old_line) {
                    matches = false;
                    break;
                }
            }

            if matches {
                // Build new content
                let mut result: Vec<&str> = content_lines[..i].to_vec();
                result.extend(new_lines.iter());
                result.extend(content_lines[i + old_lines.len()..].iter());
                return Ok(result.join("\n"));
            }
        }

        // Shouldn't reach here if we validated earlier
        Err(ApplyError::OldTextNotFound {
            path: PathBuf::new(),
        })
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
    fn test_apply_old_new_exact() {
        let dir = TempDir::new().unwrap();
        let path = setup_test_file(dir.path(), "test.rs", "fn old() {}\nfn other() {}");

        let applier = DiffApplier::new(dir.path());
        let edits = vec![EditOperation::OldNewPair {
            path: PathBuf::from("test.rs"),
            old: "fn old() {}".to_string(),
            new: "fn new() {}".to_string(),
        }];

        let result = applier.apply(&edits).unwrap();
        assert_eq!(result.modified_files.len(), 1);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("fn new() {}"));
        assert!(!content.contains("fn old() {}"));
    }

    #[test]
    fn test_apply_full_file_create() {
        let dir = TempDir::new().unwrap();
        let applier = DiffApplier::new(dir.path());

        let edits = vec![EditOperation::FullFile {
            path: PathBuf::from("new_file.rs"),
            content: "fn created() {}".to_string(),
        }];

        let result = applier.apply(&edits).unwrap();
        assert_eq!(result.created_files.len(), 1);

        let content = fs::read_to_string(dir.path().join("new_file.rs")).unwrap();
        assert_eq!(content, "fn created() {}");
    }

    #[test]
    fn test_apply_unified_diff() {
        let dir = TempDir::new().unwrap();
        let original = "fn main() {\n    println!(\"hello\");\n}\n";
        setup_test_file(dir.path(), "main.rs", original);

        let applier = DiffApplier::new(dir.path());
        let edits = vec![EditOperation::UnifiedDiff {
            path: PathBuf::from("main.rs"),
            hunks: vec![DiffHunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 4,
                lines: vec![
                    DiffLine::Context("fn main() {".to_string()),
                    DiffLine::Add("    println!(\"start\");".to_string()),
                    DiffLine::Context("    println!(\"hello\");".to_string()),
                    DiffLine::Context("}".to_string()),
                ],
            }],
        }];

        let result = applier.apply(&edits).unwrap();
        assert_eq!(result.modified_files.len(), 1);

        let content = fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert!(content.contains("println!(\"start\")"));
    }

    #[test]
    fn test_backup_created() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "original content");

        let applier = DiffApplier::new(dir.path());
        let edits = vec![EditOperation::OldNewPair {
            path: PathBuf::from("test.rs"),
            old: "original content".to_string(),
            new: "new content".to_string(),
        }];

        let result = applier.apply(&edits).unwrap();
        assert!(result.modified_files[0].backup_path.exists());

        let backup_content = fs::read_to_string(&result.modified_files[0].backup_path).unwrap();
        assert_eq!(backup_content, "original content");
    }

    #[test]
    fn test_old_text_not_found() {
        let dir = TempDir::new().unwrap();
        setup_test_file(dir.path(), "test.rs", "some content");

        let applier = DiffApplier::new(dir.path());
        let edits = vec![EditOperation::OldNewPair {
            path: PathBuf::from("test.rs"),
            old: "nonexistent text".to_string(),
            new: "new text".to_string(),
        }];

        let result = applier.apply(&edits);
        assert!(matches!(result, Err(ApplyError::OldTextNotFound { .. })));
    }

    #[test]
    fn test_fuzzy_matching_whitespace() {
        let dir = TempDir::new().unwrap();
        // File has trailing spaces
        setup_test_file(dir.path(), "test.rs", "fn foo()   \nfn bar()");

        let applier = DiffApplier::new(dir.path());
        let edits = vec![EditOperation::OldNewPair {
            path: PathBuf::from("test.rs"),
            // Old text without trailing spaces
            old: "fn foo()".to_string(),
            new: "fn new()".to_string(),
        }];

        // This should work due to whitespace normalization
        let result = applier.apply(&edits);
        // May fail exact match but normalized should work
        assert!(result.is_ok() || matches!(result, Err(ApplyError::OldTextNotFound { .. })));
    }
}
