//! Parse and validate backend output
//!
//! Handles JSON extraction from LLM output, including markdown code blocks.

use crate::config::OutputSchema;
use serde_json::Value;

/// Result of parsing backend output
#[derive(Debug, Clone)]
pub struct ParsedOutput {
    /// Raw text output
    pub raw: String,

    /// Extracted JSON (if found)
    pub json: Option<Value>,

    /// Whether the JSON matched the expected schema
    pub schema_valid: Option<bool>,

    /// Schema validation errors (if any)
    pub schema_errors: Vec<String>,
}

impl ParsedOutput {
    /// Create a new parsed output with just raw text
    pub fn raw(text: impl Into<String>) -> Self {
        Self {
            raw: text.into(),
            json: None,
            schema_valid: None,
            schema_errors: Vec::new(),
        }
    }

    /// Create with extracted JSON
    pub fn with_json(raw: impl Into<String>, json: Value) -> Self {
        Self {
            raw: raw.into(),
            json: Some(json),
            schema_valid: None,
            schema_errors: Vec::new(),
        }
    }
}

/// Parse output text, extracting JSON if present
pub fn parse_output(text: &str, schema: Option<&OutputSchema>) -> ParsedOutput {
    let mut output = ParsedOutput::raw(text);

    // Try to extract JSON
    if let Some(json) = extract_json(text) {
        output.json = Some(json.clone());

        // Validate against schema if provided
        if let Some(schema) = schema {
            let errors = validate_schema(&json, schema);
            output.schema_valid = Some(errors.is_empty());
            output.schema_errors = errors;
        }
    }

    output
}

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

/// Validate JSON against an output schema
fn validate_schema(json: &Value, schema: &OutputSchema) -> Vec<String> {
    let mut errors = Vec::new();

    // Check type
    let actual_type = match json {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    };

    if actual_type != schema.schema_type {
        errors.push(format!(
            "expected type '{}', got '{}'",
            schema.schema_type, actual_type
        ));
        return errors; // Can't validate further if type is wrong
    }

    // For objects, check required fields and property types
    if let Value::Object(obj) = json {
        // Check required fields
        for required_field in &schema.required {
            if !obj.contains_key(required_field) {
                errors.push(format!("missing required field '{}'", required_field));
            }
        }

        // Check property types
        for (prop_name, prop_schema) in &schema.properties {
            if let Some(value) = obj.get(prop_name) {
                let value_type = match value {
                    Value::Object(_) => "object",
                    Value::Array(_) => "array",
                    Value::String(_) => "string",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    Value::Null => "null",
                };

                if value_type != prop_schema.prop_type {
                    errors.push(format!(
                        "property '{}': expected type '{}', got '{}'",
                        prop_name, prop_schema.prop_type, value_type
                    ));
                }
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PropertySchema;
    use std::collections::HashMap;

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

    #[test]
    fn test_validate_schema_type() {
        let schema = OutputSchema {
            schema_type: "object".into(),
            required: vec![],
            properties: HashMap::new(),
        };

        let valid = serde_json::json!({"key": "value"});
        let invalid = serde_json::json!(["array"]);

        assert!(validate_schema(&valid, &schema).is_empty());
        assert!(!validate_schema(&invalid, &schema).is_empty());
    }

    #[test]
    fn test_validate_schema_required() {
        let schema = OutputSchema {
            schema_type: "object".into(),
            required: vec!["action".into(), "files".into()],
            properties: HashMap::new(),
        };

        let valid = serde_json::json!({"action": "fix", "files": []});
        let missing_action = serde_json::json!({"files": []});
        let missing_files = serde_json::json!({"action": "fix"});

        assert!(validate_schema(&valid, &schema).is_empty());
        assert!(!validate_schema(&missing_action, &schema).is_empty());
        assert!(!validate_schema(&missing_files, &schema).is_empty());
    }

    #[test]
    fn test_validate_schema_property_types() {
        let mut properties = HashMap::new();
        properties.insert(
            "count".into(),
            PropertySchema {
                prop_type: "number".into(),
                items: None,
            },
        );
        properties.insert(
            "name".into(),
            PropertySchema {
                prop_type: "string".into(),
                items: None,
            },
        );

        let schema = OutputSchema {
            schema_type: "object".into(),
            required: vec![],
            properties,
        };

        let valid = serde_json::json!({"count": 42, "name": "test"});
        let wrong_count = serde_json::json!({"count": "not a number", "name": "test"});
        let wrong_name = serde_json::json!({"count": 42, "name": 123});

        assert!(validate_schema(&valid, &schema).is_empty());
        assert!(!validate_schema(&wrong_count, &schema).is_empty());
        assert!(!validate_schema(&wrong_name, &schema).is_empty());
    }

    #[test]
    fn test_parse_output_no_json() {
        let output = parse_output("Just some plain text", None);
        assert_eq!(output.raw, "Just some plain text");
        assert!(output.json.is_none());
    }

    #[test]
    fn test_parse_output_with_json() {
        let text = r#"Here's the result: {"status": "ok"}"#;
        let output = parse_output(text, None);

        assert_eq!(output.raw, text);
        assert!(output.json.is_some());
        assert_eq!(output.json.as_ref().unwrap()["status"], "ok");
    }

    #[test]
    fn test_parse_output_with_schema_validation() {
        let text = r#"{"action": "fix"}"#;
        let schema = OutputSchema {
            schema_type: "object".into(),
            required: vec!["action".into()],
            properties: HashMap::new(),
        };

        let output = parse_output(text, Some(&schema));
        assert!(output.json.is_some());
        assert_eq!(output.schema_valid, Some(true));
        assert!(output.schema_errors.is_empty());
    }

    #[test]
    fn test_parse_output_schema_validation_fails() {
        let text = r#"{"wrong": "field"}"#;
        let schema = OutputSchema {
            schema_type: "object".into(),
            required: vec!["action".into()],
            properties: HashMap::new(),
        };

        let output = parse_output(text, Some(&schema));
        assert!(output.json.is_some());
        assert_eq!(output.schema_valid, Some(false));
        assert!(!output.schema_errors.is_empty());
    }
}
