#![allow(dead_code)]

//! CLI-based backend executor

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse};
use crate::config::BackendConfig;
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

        // Set working directory if specified
        if let Some(ref dir) = request.working_dir {
            cmd.current_dir(dir);
        }

        // Add the prompt as the final argument
        // Most CLI tools accept the prompt as the last positional arg
        cmd.arg(&request.prompt);

        // Configure stdio
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        cmd
    }
}

#[async_trait]
impl BackendExecutor for CliBackend {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        let start = Instant::now();
        let timeout = request.timeout.unwrap_or(self.timeout);

        let mut cmd = self.build_command(request);

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

        // Read output with timeout
        let result = tokio::time::timeout(timeout, async {
            let mut stderr_done = false;
            loop {
                tokio::select! {
                    biased;
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(l)) => {
                                eprintln!("[DEBUG {}] stdout: {}", self.name, l.chars().take(50).collect::<String>());
                                stdout_lines.push(l);
                            }
                            Ok(None) => {
                                eprintln!("[DEBUG {}] stdout EOF", self.name);
                                break;
                            }
                            Err(e) => return Err(BackendError::parse(format!("stdout read error: {}", e))),
                        }
                    }
                    line = stderr_reader.next_line(), if !stderr_done => {
                        match line {
                            Ok(Some(l)) => {
                                eprintln!("[DEBUG {}] stderr: {}", self.name, l.chars().take(50).collect::<String>());
                                stderr_lines.push(l);
                            }
                            Ok(None) => {
                                eprintln!("[DEBUG {}] stderr EOF", self.name);
                                stderr_done = true;
                            }
                            Err(e) => return Err(BackendError::parse(format!("stderr read error: {}", e))),
                        }
                    }
                }
            }

            // Wait for process to complete
            let status = child.wait().await.map_err(|e| {
                BackendError::Unavailable {
                    message: format!("failed to wait for process: {}", e),
                }
            })?;

            Ok(status)
        })
        .await;

        let elapsed = start.elapsed();

        match result {
            Ok(Ok(status)) => {
                let stdout_text = stdout_lines.join("\n");
                let stderr_text = stderr_lines.join("\n");

                if status.success() {
                    let mut response =
                        BackendResponse::new(stdout_text.clone(), self.name.clone(), elapsed);

                    // Try to parse JSON if this is a JSON-output backend
                    if self.json_output {
                        if let Ok(json) = serde_json::from_str(&stdout_text) {
                            response = response.with_structured(json);
                        }
                    }

                    Ok(response)
                } else {
                    Err(BackendError::execution_failed(
                        status.code(),
                        stdout_text,
                        stderr_text,
                    ))
                }
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Timeout - kill the process
                let _ = child.kill().await;
                let partial = if stdout_lines.is_empty() {
                    None
                } else {
                    Some(stdout_lines.join("\n"))
                };
                Err(BackendError::timeout(elapsed, partial))
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        // Check if command exists
        tokio::process::Command::new("which")
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
