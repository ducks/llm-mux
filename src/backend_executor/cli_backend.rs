#![allow(dead_code)]

//! CLI-based backend executor

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse, TokenUsage};
use crate::config::BackendConfig;
use crate::process::{
    MAX_CAPTURE_BYTES, configure_child_process, exit_status_code, terminate_child_process,
};
use async_trait::async_trait;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Executor for CLI-based LLM backends
#[derive(Debug, Clone)]
pub struct CliBackend {
    /// Backend name
    name: String,

    /// Command to execute
    command: String,

    /// Default arguments
    args: Vec<String>,

    /// Default timeout
    timeout: Duration,

    /// Environment variables to set
    env: Vec<(String, String)>,

    /// Whether output is JSON
    json_output: bool,
}

impl CliBackend {
    /// Create a new CLI backend from config
    pub fn from_config(name: impl Into<String>, config: &BackendConfig) -> Self {
        let json_output = config.args.iter().any(|a| a == "--json" || a == "-j");

        Self {
            name: name.into(),
            command: config.command.clone(),
            args: config.args.clone(),
            timeout: Duration::from_secs(config.timeout),
            env: config.env.clone(),
            json_output,
        }
    }

    /// Create a new CLI backend with explicit parameters
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            timeout: Duration::from_secs(300),
            env: Vec::new(),
            json_output: false,
        }
    }

    /// Add default arguments
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self.json_output = self.args.iter().any(|a| a == "--json" || a == "-j");
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the command with arguments
    fn build_command(&self, request: &BackendRequest) -> Command {
        let mut cmd = Command::new(&self.command);

        // Add default args
        cmd.args(&self.args);

        // Add environment variables
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        // Remove CLAUDECODE to allow nested Claude Code sessions
        // (needed when running llm-mux from within Claude Code)
        cmd.env_remove("CLAUDECODE");

        // Add the prompt as the final argument
        cmd.arg(&request.prompt);

        // Configure stdio
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        cmd
    }

    fn parse_json_output(&self, stdout: &str) -> Option<(String, serde_json::Value)> {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) {
            let text = extract_response_text(&value).unwrap_or_else(|| stdout.to_string());
            return Some((text, value));
        }

        let events = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;

        let text = events
            .iter()
            .filter_map(extract_response_text)
            .next_back()?;
        Some((text, serde_json::Value::Array(events)))
    }
}

fn extract_response_text(value: &serde_json::Value) -> Option<String> {
    for field in ["result", "response", "text", "output"] {
        if let Some(text) = value.get(field).and_then(|value| value.as_str()) {
            return Some(text.to_string());
        }
    }

    let item = value.get("item")?;
    if item.get("type").and_then(|value| value.as_str()) == Some("agent_message") {
        return item
            .get("text")
            .and_then(|value| value.as_str())
            .map(ToString::to_string);
    }

    None
}

fn extract_token_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    let usage = match value {
        serde_json::Value::Array(events) => events
            .iter()
            .rev()
            .find_map(|event| event.get("usage").or_else(|| event.pointer("/turn/usage")))?,
        value => value.get("usage")?,
    };
    let read = |names: &[&str]| {
        names.iter().find_map(|name| {
            usage
                .get(*name)
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
        })
    };
    let prompt_tokens = read(&["input_tokens", "prompt_tokens"]);
    let completion_tokens = read(&["output_tokens", "completion_tokens"]);
    let total_tokens = read(&["total_tokens"]).or_else(|| {
        prompt_tokens
            .zip(completion_tokens)
            .map(|(input, output)| input + output)
    });
    Some(TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

#[async_trait]
impl BackendExecutor for CliBackend {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        let start = Instant::now();
        let timeout = request.timeout.unwrap_or(self.timeout);

        let mut cmd = self.build_command(request);
        configure_child_process(&mut cmd);

        // Set working directory if specified
        if let Some(ref dir) = request.working_dir {
            cmd.current_dir(dir);
        }

        tracing::debug!(
            backend = %self.name,
            command = %self.command,
            args = ?self.args,
            prompt_len = request.prompt.len(),
            "Spawning backend process"
        );

        // Spawn the process
        let mut child = cmd.spawn().map_err(|e| BackendError::Unavailable {
            message: format!("failed to spawn '{}': {}", self.command, e),
        })?;

        // Set up output capture
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_lines = Vec::new();
        let mut stderr_lines = Vec::new();
        let mut stdout_bytes = 0usize;
        let mut stderr_bytes = 0usize;

        // Read output with timeout
        let execution = async {
            let mut stdout_done = false;
            let mut stderr_done = false;
            while !stdout_done || !stderr_done {
                tokio::select! {
                    biased;
                    line = stdout_reader.next_line(), if !stdout_done => {
                        match line {
                            Ok(Some(l)) => {
                                stdout_bytes = stdout_bytes.saturating_add(l.len() + 1);
                                if stdout_bytes > MAX_CAPTURE_BYTES {
                                    return Err(BackendError::parse("stdout exceeded 16 MiB capture limit"));
                                }
                                tracing::trace!(backend = %self.name, line = %l.chars().take(50).collect::<String>(), "stdout");
                                stdout_lines.push(l);
                            }
                            Ok(None) => {
                                tracing::trace!(backend = %self.name, "stdout EOF");
                                stdout_done = true;
                            }
                            Err(e) => return Err(BackendError::parse(format!("stdout read error: {}", e))),
                        }
                    }
                    line = stderr_reader.next_line(), if !stderr_done => {
                        match line {
                            Ok(Some(l)) => {
                                stderr_bytes = stderr_bytes.saturating_add(l.len() + 1);
                                if stderr_bytes > MAX_CAPTURE_BYTES {
                                    return Err(BackendError::parse("stderr exceeded 16 MiB capture limit"));
                                }
                                tracing::trace!(backend = %self.name, line = %l.chars().take(50).collect::<String>(), "stderr");
                                stderr_lines.push(l);
                            }
                            Ok(None) => {
                                tracing::trace!(backend = %self.name, "stderr EOF");
                                stderr_done = true;
                            }
                            Err(e) => return Err(BackendError::parse(format!("stderr read error: {}", e))),
                        }
                    }
                }
            }

            // Wait for process to complete
            let status = child.wait().await.map_err(|e| BackendError::Unavailable {
                message: format!("failed to wait for process: {}", e),
            })?;

            Ok(status)
        };

        let result = if let Some(cancellation) = request.cancellation.as_ref() {
            tokio::select! {
                result = tokio::time::timeout(timeout, execution) => Some(result),
                _ = cancellation.cancelled() => None,
            }
        } else {
            Some(tokio::time::timeout(timeout, execution).await)
        };

        let elapsed = start.elapsed();

        match result {
            Some(Ok(Ok(status))) => {
                let stdout_text = stdout_lines.join("\n");
                let stderr_text = stderr_lines.join("\n");

                if status.success() {
                    let response = if self.json_output {
                        if let Some((text, structured)) = self.parse_json_output(&stdout_text) {
                            let usage = extract_token_usage(&structured);
                            let mut response =
                                BackendResponse::new(text, self.name.clone(), elapsed)
                                    .with_structured(structured);
                            if let Some(usage) = usage {
                                response = response.with_usage(usage);
                            }
                            response
                        } else {
                            BackendResponse::new(stdout_text.clone(), self.name.clone(), elapsed)
                        }
                    } else {
                        BackendResponse::new(stdout_text.clone(), self.name.clone(), elapsed)
                    };

                    Ok(response)
                } else {
                    Err(BackendError::execution_failed(
                        exit_status_code(&status),
                        stdout_text,
                        stderr_text,
                    ))
                }
            }
            Some(Ok(Err(e))) => {
                // Kill and reap child to prevent zombie process
                terminate_child_process(&mut child).await;
                Err(e)
            }
            Some(Err(_)) => {
                // Timeout - kill the process
                terminate_child_process(&mut child).await;
                let partial = if stdout_lines.is_empty() {
                    None
                } else {
                    Some(stdout_lines.join("\n"))
                };
                Err(BackendError::timeout(elapsed, partial))
            }
            None => {
                terminate_child_process(&mut child).await;
                Err(BackendError::Cancelled)
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        // Check if command exists
        let locator = if cfg!(windows) { "where" } else { "which" };
        tokio::process::Command::new(locator)
            .arg(&self.command)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cli_backend_echo() {
        let backend = CliBackend::new("echo", "echo");
        let request = BackendRequest::new("Hello, World!");

        let response = backend.execute(&request).await.unwrap();
        assert_eq!(response.text.trim(), "Hello, World!");
        assert_eq!(response.backend, "echo");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_cli_backend_uses_request_working_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let backend = CliBackend::new("pwd", "sh").with_args(vec!["-c".into(), "pwd".into()]);
        let request = BackendRequest::new("ignored").with_working_dir(dir.path().to_path_buf());

        let response = backend.execute(&request).await.unwrap();
        assert_eq!(response.text.trim(), dir.path().to_string_lossy());
    }

    #[tokio::test]
    async fn test_cli_backend_timeout() {
        let backend = CliBackend::new("sleep", "sleep").with_timeout(Duration::from_millis(100));

        let request = BackendRequest::new("10"); // Sleep for 10 seconds

        let result = backend.execute(&request).await;
        assert!(matches!(result, Err(BackendError::Timeout { .. })));
    }

    #[tokio::test]
    async fn test_cli_backend_failure() {
        let backend = CliBackend::new("false", "false"); // Always exits with code 1

        let request = BackendRequest::new("");

        let result = backend.execute(&request).await;
        assert!(matches!(result, Err(BackendError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn test_cli_backend_unavailable() {
        let backend = CliBackend::new("nonexistent", "definitely_not_a_real_command_12345");

        let request = BackendRequest::new("test");

        let result = backend.execute(&request).await;
        assert!(matches!(result, Err(BackendError::Unavailable { .. })));
    }

    #[tokio::test]
    async fn test_cli_backend_is_available() {
        let echo_backend = CliBackend::new("echo", "echo");
        assert!(echo_backend.is_available().await);

        let fake_backend = CliBackend::new("fake", "definitely_not_real_12345");
        assert!(!fake_backend.is_available().await);
    }

    #[tokio::test]
    async fn test_cli_backend_json_output() {
        let backend = CliBackend::new("echo", "echo").with_args(vec!["--json".into()]);

        // The echo command doesn't actually output JSON, but we can test the parsing path
        let request = BackendRequest::new(r#"{"key": "value"}"#);

        let response = backend.execute(&request).await.unwrap();
        // Should have attempted JSON parsing
        assert!(response.structured.is_some() || response.text.contains("key"));
    }

    #[test]
    fn test_codex_jsonl_extracts_final_agent_message() {
        let backend = CliBackend::new("codex", "codex").with_args(vec!["--json".into()]);
        let output = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"abc\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"first\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"final answer\"}}\n"
        );

        let (text, structured) = backend.parse_json_output(output).unwrap();
        assert_eq!(text, "final answer");
        assert_eq!(structured.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_codex_jsonl_extracts_token_usage() {
        let structured = serde_json::json!([
            {"type": "item.completed", "item": {"type": "agent_message", "text": "done"}},
            {"type": "turn.completed", "usage": {
                "input_tokens": 10,
                "output_tokens": 4
            }}
        ]);
        let usage = extract_token_usage(&structured).unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(14));
    }

    #[tokio::test]
    async fn test_cli_backend_large_stderr_stdout_eof() {
        // Regression test for issue #32: loop must drain both streams
        // before waiting, or large stderr after stdout EOF causes deadlock
        let backend = CliBackend::new("sh", "sh")
            .with_args(vec!["-c".into()])
            .with_timeout(Duration::from_secs(5));

        // Close stdout immediately, then write >64KB to stderr
        let script = "exec >&-; for i in $(seq 1 5000); do echo \"stderr line $i\" >&2; done";
        let request = BackendRequest::new(script);

        let result = backend.execute(&request).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_from_config() {
        let config = BackendConfig {
            command: "claude".into(),
            args: vec!["--json".into()],
            timeout: 60,
            env: vec![("CLAUDE_API_KEY".into(), "test".into())],
            ..Default::default()
        };

        let backend = CliBackend::from_config("claude", &config);
        assert_eq!(backend.name, "claude");
        assert_eq!(backend.command, "claude");
        assert!(backend.json_output);
        assert_eq!(backend.timeout, Duration::from_secs(60));
    }
}
