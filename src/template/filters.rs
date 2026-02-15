//! Custom template filters

use minijinja::value::Value;
use minijinja::{Error, ErrorKind, State};

/// Register all custom filters with a minijinja Environment
pub fn register_filters(env: &mut minijinja::Environment) {
    env.add_filter("shell_escape", filter_shell_escape);
    env.add_filter("json", filter_json);
    env.add_filter("join", filter_join);
    env.add_filter("first", filter_first);
    env.add_filter("last", filter_last);
    env.add_filter("default", filter_default);
    env.add_filter("trim", filter_trim);
    env.add_filter("lines", filter_lines);
    env.add_filter("strftime", filter_strftime);
}

/// Escape a string for safe shell interpolation
///
/// Uses single quotes and escapes any embedded single quotes.
/// Example: `hello 'world'` becomes `'hello '\''world'\''`
fn filter_shell_escape(_state: &State, value: Value) -> Result<Value, Error> {
    let s = value.to_string();

    // If string contains no special characters, return as-is in quotes
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/')
    {
        return Ok(Value::from(s));
    }

    // Escape using single quotes, handling embedded single quotes
    let mut escaped = String::with_capacity(s.len() + 10);
    escaped.push('\'');
    for c in s.chars() {
        if c == '\'' {
            // End quote, add escaped quote, start new quote
            escaped.push_str("'\\''");
        } else {
            escaped.push(c);
        }
    }
    escaped.push('\'');

    Ok(Value::from(escaped))
}

/// Serialize value to JSON string
fn filter_json(_state: &State, value: Value) -> Result<Value, Error> {
    // minijinja Values can be serialized via serde
    let json = serde_json::to_string(&value).map_err(|e| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("JSON serialization failed: {}", e),
        )
    })?;
    Ok(Value::from(json))
}

/// Join an array with a separator
fn filter_join(_state: &State, value: Value, sep: Option<Value>) -> Result<Value, Error> {
    let separator = sep.as_ref().and_then(|v| v.as_str()).unwrap_or(", ");

    if value.is_undefined() || value.is_none() {
        return Ok(Value::from(""));
    }

    match value.try_iter() {
        Ok(iter) => {
            let parts: Vec<String> = iter.map(|v| v.to_string()).collect();
            Ok(Value::from(parts.join(separator)))
        }
        Err(_) => {
            // Not iterable, just convert to string
            Ok(Value::from(value.to_string()))
        }
    }
}

/// Get the first element of an array
fn filter_first(_state: &State, value: Value) -> Result<Value, Error> {
    if value.is_undefined() || value.is_none() {
        return Ok(Value::UNDEFINED);
    }

    match value.try_iter() {
        Ok(mut iter) => Ok(iter.next().unwrap_or(Value::UNDEFINED)),
        Err(_) => Err(Error::new(
            ErrorKind::InvalidOperation,
            "first filter requires a sequence",
        )),
    }
}

/// Get the last element of an array
fn filter_last(_state: &State, value: Value) -> Result<Value, Error> {
    if value.is_undefined() || value.is_none() {
        return Ok(Value::UNDEFINED);
    }

    match value.try_iter() {
        Ok(iter) => Ok(iter.last().unwrap_or(Value::UNDEFINED)),
        Err(_) => Err(Error::new(
            ErrorKind::InvalidOperation,
            "last filter requires a sequence",
        )),
    }
}

/// Return a default value if the input is undefined/empty
fn filter_default(_state: &State, value: Value, default: Value) -> Result<Value, Error> {
    if value.is_undefined() || value.is_none() {
        Ok(default)
    } else if let Some(s) = value.as_str() {
        if s.is_empty() { Ok(default) } else { Ok(value) }
    } else {
        Ok(value)
    }
}

/// Trim whitespace from a string
fn filter_trim(_state: &State, value: Value) -> Result<Value, Error> {
    Ok(Value::from(value.to_string().trim().to_string()))
}

/// Split a string into lines
fn filter_lines(_state: &State, value: Value) -> Result<Value, Error> {
    let s = value.to_string();
    let lines: Vec<Value> = s.lines().map(|l| Value::from(l.to_string())).collect();
    Ok(Value::from_iter(lines))
}

/// Format a timestamp using strftime format string
///
/// If the input is the string "now", uses the current UTC time.
/// Otherwise, attempts to parse the input as an RFC3339 timestamp.
///
/// Example: `{{ "now" | strftime("%Y-%m-%d %H:%M") }}`
fn filter_strftime(_state: &State, value: Value, format: Value) -> Result<Value, Error> {
    let format_str = format.as_str().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidOperation,
            "strftime filter requires format string as argument",
        )
    })?;

    let datetime = if let Some(s) = value.as_str() {
        if s == "now" {
            chrono::Utc::now()
        } else {
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Failed to parse datetime: {}", e),
                    )
                })?
                .with_timezone(&chrono::Utc)
        }
    } else {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            "strftime filter requires string input (\"now\" or RFC3339 timestamp)",
        ));
    };

    Ok(Value::from(datetime.format(format_str).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Environment;

    fn render(template: &str, ctx: Value) -> String {
        let mut env = Environment::new();
        register_filters(&mut env);
        env.add_template("test", template).unwrap();
        env.get_template("test").unwrap().render(ctx).unwrap()
    }

    #[test]
    fn test_shell_escape_simple() {
        let result = render(
            "{{ value | shell_escape }}",
            minijinja::context! { value => "hello" },
        );
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_shell_escape_spaces() {
        let result = render(
            "{{ value | shell_escape }}",
            minijinja::context! { value => "hello world" },
        );
        assert_eq!(result, "'hello world'");
    }

    #[test]
    fn test_shell_escape_quotes() {
        let result = render(
            "{{ value | shell_escape }}",
            minijinja::context! { value => "it's a test" },
        );
        assert_eq!(result, "'it'\\''s a test'");
    }

    #[test]
    fn test_shell_escape_special_chars() {
        let result = render(
            "{{ value | shell_escape }}",
            minijinja::context! { value => "$(rm -rf /)" },
        );
        // Should be safely quoted
        assert!(result.starts_with('\''));
        assert!(result.ends_with('\''));
    }

    #[test]
    fn test_json_filter() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert("key", "value");
        let result = render("{{ value | json }}", minijinja::context! { value => map });
        assert!(result.contains("\"key\""));
        assert!(result.contains("\"value\""));
    }

    #[test]
    fn test_json_filter_array() {
        let result = render(
            "{{ value | json }}",
            minijinja::context! { value => vec!["a", "b", "c"] },
        );
        assert_eq!(result, "[\"a\",\"b\",\"c\"]");
    }

    #[test]
    fn test_join_filter() {
        let result = render(
            "{{ items | join(', ') }}",
            minijinja::context! { items => vec!["a", "b", "c"] },
        );
        assert_eq!(result, "a, b, c");
    }

    #[test]
    fn test_join_filter_default_separator() {
        let result = render(
            "{{ items | join }}",
            minijinja::context! { items => vec!["a", "b"] },
        );
        assert_eq!(result, "a, b");
    }

    #[test]
    fn test_join_filter_custom_separator() {
        let result = render(
            "{{ items | join(' | ') }}",
            minijinja::context! { items => vec!["a", "b", "c"] },
        );
        assert_eq!(result, "a | b | c");
    }

    #[test]
    fn test_first_filter() {
        let result = render(
            "{{ items | first }}",
            minijinja::context! { items => vec!["a", "b", "c"] },
        );
        assert_eq!(result, "a");
    }

    #[test]
    fn test_first_filter_empty() {
        let result = render(
            "{{ items | first | default('none') }}",
            minijinja::context! { items => Vec::<String>::new() },
        );
        assert_eq!(result, "none");
    }

    #[test]
    fn test_last_filter() {
        let result = render(
            "{{ items | last }}",
            minijinja::context! { items => vec!["a", "b", "c"] },
        );
        assert_eq!(result, "c");
    }

    #[test]
    fn test_default_filter() {
        let result = render(
            "{{ missing | default('fallback') }}",
            minijinja::context! {},
        );
        assert_eq!(result, "fallback");
    }

    #[test]
    fn test_default_filter_with_value() {
        let result = render(
            "{{ value | default('fallback') }}",
            minijinja::context! { value => "actual" },
        );
        assert_eq!(result, "actual");
    }

    #[test]
    fn test_trim_filter() {
        let result = render(
            "{{ value | trim }}",
            minijinja::context! { value => "  hello  " },
        );
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_lines_filter() {
        let result = render(
            "{{ value | lines | first }}",
            minijinja::context! { value => "line1\nline2\nline3" },
        );
        assert_eq!(result, "line1");
    }

    #[test]
    fn test_strftime_filter_now() {
        let result = render("{{ \"now\" | strftime(\"%Y\") }}", minijinja::context! {});
        // Should render current year as 4 digits
        assert_eq!(result.len(), 4);
        assert!(result.parse::<i32>().is_ok());
    }

    #[test]
    fn test_strftime_filter_rfc3339() {
        let result = render(
            "{{ timestamp | strftime(\"%Y-%m-%d\") }}",
            minijinja::context! { timestamp => "2026-02-14T12:34:56Z" },
        );
        assert_eq!(result, "2026-02-14");
    }

    #[test]
    fn test_strftime_filter_custom_format() {
        let result = render(
            "{{ timestamp | strftime(\"%H:%M\") }}",
            minijinja::context! { timestamp => "2026-02-14T15:30:00Z" },
        );
        assert_eq!(result, "15:30");
    }
}
