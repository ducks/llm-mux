//! Environment variable expansion in config values.
//!
//! Supports `${VAR}` syntax inside string values. Used for secrets like
//! API keys that shouldn't be committed to config files.

use anyhow::{Result, anyhow};
use std::env;

/// Expand every `${VAR}` occurrence in `input` with the value of the named
/// environment variable. Returns an error if any referenced variable is unset
/// or empty.
///
/// Literal `$` characters that aren't followed by `{` pass through unchanged,
/// so plain literal API keys (e.g. `sk-...`) still work.
pub fn expand(input: &str) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let end = after_open
            .find('}')
            .ok_or_else(|| anyhow!("unterminated `${{` in value: {input:?}"))?;
        let var_name = &after_open[..end];
        if var_name.is_empty() {
            return Err(anyhow!(
                "empty variable name in `${{}}` in value: {input:?}"
            ));
        }
        let value = env::var(var_name).map_err(|_| {
            anyhow!("environment variable `{var_name}` is not set (referenced in config)")
        })?;
        if value.is_empty() {
            return Err(anyhow!(
                "environment variable `{var_name}` is set but empty"
            ));
        }
        out.push_str(&value);
        rest = &after_open[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env<F: FnOnce()>(key: &str, value: &str, f: F) {
        // SAFETY: tests in this module run single-threaded under cargo test
        // by default; we restore the previous value on exit.
        let prev = env::var(key).ok();
        unsafe {
            env::set_var(key, value);
        }
        f();
        unsafe {
            match prev {
                Some(v) => env::set_var(key, v),
                None => env::remove_var(key),
            }
        }
    }

    #[test]
    fn expand_literal_passthrough() {
        assert_eq!(expand("sk-literal-key").unwrap(), "sk-literal-key");
    }

    #[test]
    fn expand_empty_string() {
        assert_eq!(expand("").unwrap(), "");
    }

    #[test]
    fn expand_single_var() {
        with_env("LLM_MUX_TEST_KEY_1", "secret-value", || {
            assert_eq!(expand("${LLM_MUX_TEST_KEY_1}").unwrap(), "secret-value");
        });
    }

    #[test]
    fn expand_var_within_string() {
        with_env("LLM_MUX_TEST_KEY_2", "abc", || {
            assert_eq!(
                expand("prefix-${LLM_MUX_TEST_KEY_2}-suffix").unwrap(),
                "prefix-abc-suffix"
            );
        });
    }

    #[test]
    fn expand_multiple_vars() {
        with_env("LLM_MUX_TEST_A", "1", || {
            with_env("LLM_MUX_TEST_B", "2", || {
                assert_eq!(
                    expand("${LLM_MUX_TEST_A}/${LLM_MUX_TEST_B}").unwrap(),
                    "1/2"
                );
            });
        });
    }

    #[test]
    fn expand_unset_var_errors() {
        let err = expand("${LLM_MUX_DEFINITELY_NOT_SET_XYZ}").unwrap_err();
        assert!(err.to_string().contains("not set"));
        assert!(err.to_string().contains("LLM_MUX_DEFINITELY_NOT_SET_XYZ"));
    }

    #[test]
    fn expand_empty_var_errors() {
        with_env("LLM_MUX_TEST_EMPTY", "", || {
            let err = expand("${LLM_MUX_TEST_EMPTY}").unwrap_err();
            assert!(err.to_string().contains("empty"));
        });
    }

    #[test]
    fn expand_unterminated_brace_errors() {
        let err = expand("${UNCLOSED").unwrap_err();
        assert!(err.to_string().contains("unterminated"));
    }

    #[test]
    fn expand_empty_var_name_errors() {
        let err = expand("${}").unwrap_err();
        assert!(err.to_string().contains("empty variable name"));
    }

    #[test]
    fn expand_dollar_without_brace_passthrough() {
        // Bare `$` is not a placeholder marker; pass through unchanged.
        assert_eq!(expand("price: $5.00").unwrap(), "price: $5.00");
    }
}
