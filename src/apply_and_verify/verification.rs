//! Verification command execution

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use crate::process::{capture_exit_code, exit_status_code};
use tokio::process::Command;
use tokio::time::timeout;

/// Errors during verification
#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("verification command failed to spawn: {0}")]
    SpawnFailed(std::io::Error),

    #[error("verification timed out after {0:?}")]
    Timeout(Duration),

    #[error("failed to read output (exit code {exit_code:?}): {source}")]
    OutputError { source: std::io::Error, exit_code: Option<i32> },
}

/// Result of running a verification command
#[derive(Debug, Clone)]
pub struct VerifyResult {
    /// Whether the command succeeded (exit code 0)
    pub success: bool,
    /// Exit code if available
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// How long the command took
    pub duration: Duration,
}

impl VerifyResult {
    /// Create a successful result
    pub fn success(stdout: String, stderr: String, duration: Duration) -> Self {
        Self {
            success: true,
            exit_code: Some(0),
            stdout,
            stderr,
            duration,
        }
    }

    /// Create a failed result
    pub fn failure(
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        duration: Duration,
    ) -> Self {
        Self {
            success: false,
            exit_code,
            stdout,
            stderr,
            duration,
        }
    }

    /// Get combined output for error context
    pub fn combined_output(&self) -> String {
        let mut output = String::new();
        if !self.stdout.is_empty() {
            output.push_str("=== stdout ===\n");
            output.push_str(&self.stdout);
            output.push('\n');
        }
        if !self.stderr.is_empty() {
            output.push_str("=== stderr ===\n");
            output.push_str(&self.stderr);
        }
        output
    }
}

/// Run a verification command
pub async fn run_verify(
    command: &str,
    working_dir: &Path,
    timeout_duration: Option<Duration>,
) -> Result<VerifyResult, VerifyError> {
    let start = Instant::now();

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(VerifyError::SpawnFailed)?;

    // Wrap in timeout if specified
    let result = if let Some(dur) = timeout_duration {
        match timeout(dur, wait_for_output(&mut child)).await {
            Ok(r) => r,
            Err(_) => {
                // Kill the process on timeout
                let _ = child.kill().await;
                return Err(VerifyError::Timeout(dur));
            }
        }
    } else {
        wait_for_output(&mut child).await
    };

    let duration = start.elapsed();
    let (stdout, stderr, status) = result?;

    Ok(if status.success() {
        VerifyResult::success(stdout, stderr, duration)
    } else {
        VerifyResult::failure(exit_status_code(&status), stdout, stderr, duration)
    })
}

/// Wait for command output
/// Reads stdout and stderr concurrently to avoid deadlock when child produces
/// >64KB on one stream while we're blocked reading the other.
async fn wait_for_output(
    child: &mut tokio::process::Child,
) -> Result<(String, String, std::process::ExitStatus), VerifyError> {
    // Take ownership of pipes to read concurrently
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_fut = async {
        if let Some(mut out) = stdout_pipe {
            let mut buf = String::new();
            out.read_to_string(&mut buf).await.map(|_| buf)
        } else {
            Ok(String::new())
        }
    };

    let stderr_fut = async {
        if let Some(mut err) = stderr_pipe {
            let mut buf = String::new();
            err.read_to_string(&mut buf).await.map(|_| buf)
        } else {
            Ok(String::new())
        }
    };

    let (stdout_result, stderr_result) = tokio::join!(stdout_fut, stderr_fut);

    let stdout = match stdout_result {
        Ok(s) => s,
        Err(e) => {
            let _ = child.kill().await;
            let exit_code = capture_exit_code(child).await;
            return Err(VerifyError::OutputError { source: e, exit_code });
        }
    };

    let stderr = match stderr_result {
        Ok(s) => s,
        Err(e) => {
            let _ = child.kill().await;
            let exit_code = capture_exit_code(child).await;
            return Err(VerifyError::OutputError { source: e, exit_code });
        }
    };

    let status = child.wait().await.map_err(VerifyError::SpawnFailed)?;

    Ok((stdout, stderr, status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_successful_command() {
        let dir = TempDir::new().unwrap();

        let result = run_verify("echo 'success'", dir.path(), None)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("success"));
    }

    #[tokio::test]
    async fn test_failing_command() {
        let dir = TempDir::new().unwrap();

        let result = run_verify("exit 1", dir.path(), None).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, Some(1));
    }

    #[tokio::test]
    async fn test_command_with_stderr() {
        let dir = TempDir::new().unwrap();

        let result = run_verify("echo 'error' >&2 && exit 1", dir.path(), None)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.stderr.contains("error"));
    }

    #[tokio::test]
    async fn test_timeout() {
        let dir = TempDir::new().unwrap();

        let result = run_verify("sleep 10", dir.path(), Some(Duration::from_millis(100))).await;

        assert!(matches!(result, Err(VerifyError::Timeout(_))));
    }

    #[tokio::test]
    async fn test_combined_output() {
        let dir = TempDir::new().unwrap();

        let result = run_verify("echo 'out' && echo 'err' >&2", dir.path(), None)
            .await
            .unwrap();

        let combined = result.combined_output();
        assert!(combined.contains("out"));
        assert!(combined.contains("err"));
    }

    #[tokio::test]
    async fn test_working_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "content").unwrap();

        let result = run_verify("cat test.txt", dir.path(), None).await.unwrap();

        assert!(result.success);
        assert!(result.stdout.contains("content"));
    }

    #[test]
    fn test_verify_result_constructors() {
        let success = VerifyResult::success("out".into(), "err".into(), Duration::from_secs(1));
        assert!(success.success);
        assert_eq!(success.exit_code, Some(0));

        let failure =
            VerifyResult::failure(Some(2), "out".into(), "err".into(), Duration::from_secs(1));
        assert!(!failure.success);
        assert_eq!(failure.exit_code, Some(2));
    }
}
