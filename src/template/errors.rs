//! Template error types with source locations and suggestions

use std::fmt;
use thiserror::Error;

/// Location in a template where an error occurred
#[derive(Debug, Clone, Default)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
    pub template_name: Option<String>,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref name) = self.template_name {
            write!(f, "{}:{}:{}", name, self.line, self.column)
        } else {
            write!(f, "line {}:{}", self.line, self.column)
        }
    }
}

/// Template rendering errors
#[derive(Debug, Error)]
pub enum TemplateError {
    /// Referenced variable doesn't exist
    #[error("undefined variable '{name}' at {location}{}", .suggestion.as_ref().map(|s| format!(", did you mean '{}'?", s)).unwrap_or_default())]
    UndefinedVariable {
        name: String,
        location: SourceLocation,
        suggestion: Option<String>,
    },

    /// Template syntax error
    #[error("syntax error at {location}: {message}")]
    SyntaxError {
        message: String,
        location: SourceLocation,
    },

    /// Filter execution error
    #[error("filter '{filter}' failed: {message}")]
    FilterError { filter: String, message: String },

    /// Type mismatch (e.g., trying to iterate non-array)
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },

    /// Expression evaluation error
    #[error("expression error: {message}")]
    ExpressionError { message: String },

    /// Wrapped minijinja error
    #[error("template error: {0}")]
    Internal(#[from] minijinja::Error),
}

impl TemplateError {
    /// Create an undefined variable error with optional suggestion
    pub fn undefined_variable(name: impl Into<String>, known_vars: &[&str]) -> Self {
        let name = name.into();
        let suggestion = suggest_correction(&name, known_vars);
        Self::UndefinedVariable {
            name,
            location: SourceLocation::default(),
            suggestion,
        }
    }

    /// Create an undefined variable error with location
    pub fn undefined_variable_at(
        name: impl Into<String>,
        line: usize,
        column: usize,
        known_vars: &[&str],
    ) -> Self {
        let name = name.into();
        let suggestion = suggest_correction(&name, known_vars);
        Self::UndefinedVariable {
            name,
            location: SourceLocation {
                line,
                column,
                template_name: None,
            },
            suggestion,
        }
    }

    /// Create a syntax error
    pub fn syntax(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self::SyntaxError {
            message: message.into(),
            location: SourceLocation {
                line,
                column,
                template_name: None,
            },
        }
    }

    /// Create a filter error
    pub fn filter(filter: impl Into<String>, message: impl Into<String>) -> Self {
        Self::FilterError {
            filter: filter.into(),
            message: message.into(),
        }
    }

    /// Create a type mismatch error
    pub fn type_mismatch(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Self::TypeMismatch {
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    /// Create an expression error
    pub fn expression(message: impl Into<String>) -> Self {
        Self::ExpressionError {
            message: message.into(),
        }
    }
}

/// Suggest a correction for a typo using Levenshtein distance
pub fn suggest_correction(typo: &str, candidates: &[&str]) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }

    let mut best_match = None;
    let mut best_distance = usize::MAX;
    let max_distance = (typo.len() / 2).max(2); // Allow up to half the length in edits

    for candidate in candidates {
        let distance = levenshtein_distance(typo, candidate);
        if distance < best_distance && distance <= max_distance {
            best_distance = distance;
            best_match = Some(candidate.to_string());
        }
    }

    best_match
}

/// Calculate Levenshtein distance between two strings
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for (i, row) in matrix.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[a_len][b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("analyze", "anaylze"), 2);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("same", "same"), 0);
    }

    #[test]
    fn test_suggest_correction() {
        let candidates = ["analyze", "apply", "fetch", "verify"];

        // Common typos
        assert_eq!(
            suggest_correction("anaylze", &candidates),
            Some("analyze".into())
        );
        assert_eq!(
            suggest_correction("aply", &candidates),
            Some("apply".into())
        );

        // No good match
        assert_eq!(
            suggest_correction("completely_different", &candidates),
            None
        );

        // Empty candidates
        assert_eq!(suggest_correction("anything", &[]), None);
    }

    #[test]
    fn test_error_display() {
        let err = TemplateError::undefined_variable_at("anaylze", 5, 10, &["analyze", "apply"]);
        let msg = err.to_string();
        assert!(msg.contains("undefined variable 'anaylze'"));
        assert!(msg.contains("line 5:10"));
        assert!(msg.contains("did you mean 'analyze'"));
    }

    #[test]
    fn test_source_location_display() {
        let loc = SourceLocation {
            line: 10,
            column: 5,
            template_name: None,
        };
        assert_eq!(loc.to_string(), "line 10:5");

        let loc_with_name = SourceLocation {
            line: 10,
            column: 5,
            template_name: Some("prompt.txt".into()),
        };
        assert_eq!(loc_with_name.to_string(), "prompt.txt:10:5");
    }
}
