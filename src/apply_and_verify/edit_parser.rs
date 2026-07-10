//! Edit format parser for LLM output
//!
//! Supports three edit formats:
//! - Unified diff (preferred)
//! - Old/new text pairs
//! - Full file replacement

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Errors during edit parsing
#[derive(Debug, Error)]
pub enum EditParseError {
    #[error("no edits found in output")]
    NoEditsFound,

    #[error("invalid unified diff format: {message}")]
    InvalidDiff { message: String },

    #[error("missing required field '{field}' in edit")]
    MissingField { field: String },

    #[error("failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("ambiguous edit format")]
    AmbiguousFormat,
}

/// A single edit operation
#[derive(Debug, Clone, PartialEq)]
pub enum EditOperation {
    /// Unified diff with hunks
    UnifiedDiff { path: PathBuf, hunks: Vec<DiffHunk> },

    /// Old text to be replaced with new text
    OldNewPair {
        path: PathBuf,
        old: String,
        new: String,
    },

    /// Replace entire file content
    FullFile { path: PathBuf, content: String },
}

/// A single hunk from a unified diff
#[derive(Debug, Clone, PartialEq)]
pub struct DiffHunk {
    /// Starting line in original file (1-indexed)
    pub old_start: usize,
    /// Number of lines in original
    pub old_count: usize,
    /// Starting line in new file (1-indexed)
    pub new_start: usize,
    /// Number of lines in new
    pub new_count: usize,
    /// Lines in the hunk (with +/- prefix)
    pub lines: Vec<DiffLine>,
}

/// A line in a diff hunk
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    Context(String),
    Add(String),
    Remove(String),
}

/// JSON format for old/new pairs
#[derive(Debug, Deserialize, Serialize)]
struct OldNewJson {
    path: String,
    old: String,
    new: String,
}

/// JSON format for full file replacement
#[derive(Debug, Deserialize, Serialize)]
struct FullFileJson {
    path: String,
    content: String,
}

/// JSON format for edits array
#[derive(Debug, Deserialize, Serialize)]
struct EditsArray {
    edits: Vec<EditJson>,
}

/// Union of edit formats in JSON
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum EditJson {
    OldNew(OldNewJson),
    FullFile(FullFileJson),
}

/// Parse edits from LLM output, auto-detecting format
pub fn parse_edits(output: &str) -> Result<Vec<EditOperation>, EditParseError> {
    // Try unified diff first
    if let Ok(edits) = parse_unified_diff(output)
        && !edits.is_empty()
    {
        return Ok(edits);
    }

    // Try JSON formats
    if let Ok(edits) = parse_json_edits(output)
        && !edits.is_empty()
    {
        return Ok(edits);
    }

    // Try extracting JSON from markdown code blocks
    if let Some(json_str) = extract_json_block(output)
        && let Ok(edits) = parse_json_edits(&json_str)
        && !edits.is_empty()
    {
        return Ok(edits);
    }

    Err(EditParseError::NoEditsFound)
}

/// Parse unified diff format
pub fn parse_unified_diff(input: &str) -> Result<Vec<EditOperation>, EditParseError> {
    let mut edits = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_hunks: Vec<DiffHunk> = Vec::new();

    // Regex for diff header: --- a/path or +++ b/path
    let header_re = Regex::new(r"^(?:---|\+\+\+)\s+[ab]/(.+)$").unwrap();
    // Regex for hunk header: @@ -old_start,old_count +new_start,new_count @@
    let hunk_re = Regex::new(r"^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@").unwrap();

    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Check for diff header
        if let Some(caps) = header_re.captures(line) {
            let path = PathBuf::from(&caps[1]);

            // If we have a previous file, save it
            if let Some(prev_path) = current_path.take()
                && !current_hunks.is_empty()
            {
                edits.push(EditOperation::UnifiedDiff {
                    path: prev_path,
                    hunks: std::mem::take(&mut current_hunks),
                });
            }

            // Start tracking new file (use +++ path as the canonical one)
            if line.starts_with("+++") {
                current_path = Some(path);
            }
            i += 1;
            continue;
        }

        // Check for hunk header
        if let Some(caps) = hunk_re.captures(line) {
            let old_start: usize = caps[1].parse().unwrap_or(1);
            let old_count: usize = caps.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
            let new_start: usize = caps[3].parse().unwrap_or(1);
            let new_count: usize = caps.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));

            let mut hunk_lines = Vec::new();
            i += 1;

            // Parse hunk lines until we hit another header or end
            while i < lines.len() {
                let hunk_line = lines[i];

                if hunk_line.starts_with("@@")
                    || hunk_line.starts_with("---")
                    || hunk_line.starts_with("+++")
                    || hunk_line.starts_with("diff ")
                {
                    break;
                }

                if let Some(content) = hunk_line.strip_prefix('+') {
                    hunk_lines.push(DiffLine::Add(content.to_string()));
                } else if let Some(content) = hunk_line.strip_prefix('-') {
                    hunk_lines.push(DiffLine::Remove(content.to_string()));
                } else if let Some(content) = hunk_line.strip_prefix(' ') {
                    hunk_lines.push(DiffLine::Context(content.to_string()));
                } else if hunk_line.is_empty() || hunk_line == "\\ No newline at end of file" {
                    // Skip empty lines or no-newline markers
                } else {
                    // Treat as context line (some diffs omit the space prefix)
                    hunk_lines.push(DiffLine::Context(hunk_line.to_string()));
                }

                i += 1;
            }

            current_hunks.push(DiffHunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines: hunk_lines,
            });
            continue;
        }

        i += 1;
    }

    // Save final file if any
    if let Some(path) = current_path
        && !current_hunks.is_empty()
    {
        edits.push(EditOperation::UnifiedDiff {
            path,
            hunks: current_hunks,
        });
    }

    Ok(edits)
}

/// Parse JSON edit formats
fn parse_json_edits(input: &str) -> Result<Vec<EditOperation>, EditParseError> {
    let mut edits = Vec::new();

    // Try parsing as an array of edits
    if let Ok(arr) = serde_json::from_str::<EditsArray>(input) {
        for edit in arr.edits {
            edits.push(convert_json_edit(edit));
        }
        return Ok(edits);
    }

    // Try parsing as a direct array
    if let Ok(arr) = serde_json::from_str::<Vec<EditJson>>(input) {
        for edit in arr {
            edits.push(convert_json_edit(edit));
        }
        return Ok(edits);
    }

    // Try parsing as a single edit
    if let Ok(edit) = serde_json::from_str::<EditJson>(input) {
        edits.push(convert_json_edit(edit));
        return Ok(edits);
    }

    Ok(edits)
}

/// Convert JSON edit to EditOperation
fn convert_json_edit(edit: EditJson) -> EditOperation {
    match edit {
        EditJson::OldNew(on) => EditOperation::OldNewPair {
            path: PathBuf::from(on.path),
            old: on.old,
            new: on.new,
        },
        EditJson::FullFile(ff) => EditOperation::FullFile {
            path: PathBuf::from(ff.path),
            content: ff.content,
        },
    }
}

/// Extract JSON from markdown code blocks
fn extract_json_block(input: &str) -> Option<String> {
    // Look for ```json ... ``` or ``` ... ```
    let json_block_re = Regex::new(r"```(?:json)?\s*\n([\s\S]*?)\n```").unwrap();

    for caps in json_block_re.captures_iter(input) {
        let content = &caps[1];
        // Verify it looks like JSON
        let trimmed = content.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return Some(trimmed.to_string());
        }
    }

    None
}

/// Normalize whitespace for fuzzy matching
pub fn normalize_whitespace(s: &str) -> String {
    s.lines()
        .map(|line| {
            // Normalize trailing whitespace but preserve leading
            line.trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_unified_diff_single_hunk() {
        let diff = r#"
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;

        let edits = parse_edits(diff).unwrap();
        assert_eq!(edits.len(), 1);

        match &edits[0] {
            EditOperation::UnifiedDiff { path, hunks } => {
                assert_eq!(path, &PathBuf::from("src/main.rs"));
                assert_eq!(hunks.len(), 1);
                assert_eq!(hunks[0].old_start, 1);
                assert_eq!(hunks[0].new_count, 4);
            }
            _ => panic!("expected unified diff"),
        }
    }

    #[test]
    fn test_parse_unified_diff_multiple_hunks() {
        let diff = r#"
--- a/lib.rs
+++ b/lib.rs
@@ -1,2 +1,3 @@
+// Header
 pub fn foo() {}
 pub fn bar() {}
@@ -10,3 +11,4 @@
 fn test_foo() {
     foo();
+    bar();
 }
"#;

        let edits = parse_edits(diff).unwrap();
        assert_eq!(edits.len(), 1);

        match &edits[0] {
            EditOperation::UnifiedDiff { hunks, .. } => {
                assert_eq!(hunks.len(), 2);
                assert_eq!(hunks[0].old_start, 1);
                assert_eq!(hunks[1].old_start, 10);
            }
            _ => panic!("expected unified diff"),
        }
    }

    #[test]
    fn test_parse_old_new_json() {
        let json = r#"
{
    "path": "src/lib.rs",
    "old": "fn old() {}",
    "new": "fn new() {}"
}
"#;

        let edits = parse_edits(json).unwrap();
        assert_eq!(edits.len(), 1);

        match &edits[0] {
            EditOperation::OldNewPair { path, old, new } => {
                assert_eq!(path, &PathBuf::from("src/lib.rs"));
                assert_eq!(old, "fn old() {}");
                assert_eq!(new, "fn new() {}");
            }
            _ => panic!("expected old/new pair"),
        }
    }

    #[test]
    fn test_parse_full_file_json() {
        let json = r#"
{
    "path": "new_file.rs",
    "content": "fn main() {}"
}
"#;

        let edits = parse_edits(json).unwrap();
        assert_eq!(edits.len(), 1);

        match &edits[0] {
            EditOperation::FullFile { path, content } => {
                assert_eq!(path, &PathBuf::from("new_file.rs"));
                assert_eq!(content, "fn main() {}");
            }
            _ => panic!("expected full file"),
        }
    }

    #[test]
    fn test_parse_edits_array() {
        let json = r#"
{
    "edits": [
        {"path": "a.rs", "old": "old1", "new": "new1"},
        {"path": "b.rs", "old": "old2", "new": "new2"}
    ]
}
"#;

        let edits = parse_edits(json).unwrap();
        assert_eq!(edits.len(), 2);
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let output = r#"
Here are the edits:

```json
{"path": "test.rs", "old": "a", "new": "b"}
```

Done!
"#;

        let edits = parse_edits(output).unwrap();
        assert_eq!(edits.len(), 1);
    }

    #[test]
    fn test_no_edits_found() {
        let output = "This is just text with no edits.";
        let result = parse_edits(output);
        assert!(matches!(result, Err(EditParseError::NoEditsFound)));
    }

    #[test]
    fn test_normalize_whitespace() {
        let input = "line1   \nline2\t\n  line3  ";
        let normalized = normalize_whitespace(input);
        assert_eq!(normalized, "line1\nline2\n  line3");
    }

    #[test]
    fn test_diff_line_types() {
        let diff = r#"
--- a/test.rs
+++ b/test.rs
@@ -1,3 +1,3 @@
 context
-removed
+added
"#;

        let edits = parse_edits(diff).unwrap();
        match &edits[0] {
            EditOperation::UnifiedDiff { hunks, .. } => {
                let lines = &hunks[0].lines;
                assert!(matches!(lines[0], DiffLine::Context(_)));
                assert!(matches!(lines[1], DiffLine::Remove(_)));
                assert!(matches!(lines[2], DiffLine::Add(_)));
            }
            _ => panic!("expected unified diff"),
        }
    }
}
