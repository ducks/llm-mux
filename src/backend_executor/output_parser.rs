//! Parse and validate backend output
//!
//! Handles JSON extraction from LLM output, including markdown code blocks.

use serde_json::Value;

/// Extract JSON from text, handling various formats
pub fn extract_json(text: &str) -> Option<Value> {
    // Try markdown code blocks first
    if let Some(json) = extract_from_code_block(text, "json") {
        return Some(json);
    }

    // Try generic code blocks
    if let Some(json) = extract_from_code_block(text, "") {
        return Some(json);
    }

    // Try parsing the whole text as JSON
    if let Ok(json) = serde_json::from_str(text) {
        return Some(json);
    }

    // Try finding JSON-like content
    if let Some(json) = find_json_in_text(text) {
        return Some(json);
    }

    None
}

/// Extract JSON from a markdown code block
fn extract_from_code_block(text: &str, lang: &str) -> Option<Value> {
    let start_patterns: Vec<String> = if lang.is_empty() {
        vec!["```\n".into(), "```".into()]
    } else {
        vec![
            format!("```{}\n", lang),
            format!("```{}\r\n", lang),
            format!("```{} ", lang), // Some LLMs add space after lang
        ]
    };

    for start_pattern in &start_patterns {
        if let Some(start) = text.find(start_pattern.as_str()) {
            let content_start = start + start_pattern.len();
            let remaining = &text[content_start..];

            if let Some(end) = remaining.find("```") {
                let json_str = remaining[..end].trim();
                if let Ok(json) = serde_json::from_str(json_str) {
                    return Some(json);
                }
            }
        }
    }

    None
}

/// Find JSON object or array in text
fn find_json_in_text(text: &str) -> Option<Value> {
    // Look for JSON objects
    if let Some(start) = text.find('{') {
        if let Some(json) = try_parse_from_position(text, start, '{', '}') {
            return Some(json);
        }
    }

    // Look for JSON arrays
    if let Some(start) = text.find('[') {
        if let Some(json) = try_parse_from_position(text, start, '[', ']') {
            return Some(json);
        }
    }

    None
}

/// Try to parse JSON starting from a given position
fn try_parse_from_position(text: &str, start: usize, open: char, close: char) -> Option<Value> {
    let remaining = &text[start..];
    let mut depth = 0;
    let mut end = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, c) in remaining.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                end = i + 1;
                break;
            }
        }
    }

    if end > 0 {
        let json_str = &remaining[..end];
        if let Ok(json) = serde_json::from_str(json_str) {
            return Some(json);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_from_code_block() {
        let text = r#"
Here is the JSON:

```json
{"key": "value", "count": 42}
```

That's all.
"#;

        let json = extract_json(text).unwrap();
        assert_eq!(json["key"], "value");
        assert_eq!(json["count"], 42);
    }

    #[test]
    fn test_extract_json_from_generic_block() {
        let text = r#"
```
{"key": "value"}
```
"#;

        let json = extract_json(text).unwrap();
        assert_eq!(json["key"], "value");
    }

    #[test]
    fn test_extract_json_direct() {
        let text = r#"{"key": "value"}"#;

        let json = extract_json(text).unwrap();
        assert_eq!(json["key"], "value");
    }

    #[test]
    fn test_extract_json_embedded() {
        let text = r#"
The result is: {"action": "fix", "files": ["main.rs"]} and that's it.
"#;

        let json = extract_json(text).unwrap();
        assert_eq!(json["action"], "fix");
    }

    #[test]
    fn test_extract_json_array() {
        let text = r#"Files: ["a.rs", "b.rs", "c.rs"]"#;

        let json = extract_json(text).unwrap();
        assert!(json.is_array());
        assert_eq!(json[0], "a.rs");
    }

    #[test]
    fn test_extract_json_none() {
        let text = "This is just plain text with no JSON";
        assert!(extract_json(text).is_none());
    }

    #[test]
    fn test_extract_json_nested() {
        let text = r#"
```json
{
  "outer": {
    "inner": {
      "value": 123
    }
  }
}
```
"#;

        let json = extract_json(text).unwrap();
        assert_eq!(json["outer"]["inner"]["value"], 123);
    }
}
