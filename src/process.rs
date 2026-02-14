//! Process utilities for child process management.

use tokio::io::AsyncReadExt;
use tokio::process::Child;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

fn exit_status_code_parts(code: Option<i32>, _signal: Option<i32>) -> Option<i32> {
    if let Some(code) = code {
        return Some(code);
    }
    #[cfg(unix)]
    {
        if let Some(signal) = _signal {
            return Some(128 + signal);
        }
    }
    None
}

/// Extract exit code from ExitStatus, using 128+signal for signal-terminated processes on Unix.
pub(crate) fn exit_status_code(status: &std::process::ExitStatus) -> Option<i32> {
    let code = status.code();
    #[cfg(unix)]
    let signal = status.signal();
    #[cfg(not(unix))]
    let signal = None;
    exit_status_code_parts(code, signal)
}

/// Attempt to capture the exit code from a child process.
/// Tries non-blocking first, falls back to blocking wait if process hasn't exited.
/// On Unix, signal-terminated processes return 128 + signal number.
pub(crate) async fn capture_exit_code(child: &mut Child) -> Option<i32> {
    match child.try_wait() {
        Ok(Some(status)) => exit_status_code(&status),
        Ok(None) => child.wait().await.ok().and_then(|status| exit_status_code(&status)),
        Err(_) => None,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub(crate) enum OutputWaitError {
    Read {
        stream: OutputStream,
        source: std::io::Error,
        exit_code: Option<i32>,
    },
    Wait {
        source: std::io::Error,
    },
}

/// Wait for child output, reading stdout/stderr concurrently to avoid deadlock.
pub(crate) async fn wait_for_child_output(
    child: &mut Child,
) -> Result<(String, String, std::process::ExitStatus), OutputWaitError> {
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
            return Err(OutputWaitError::Read {
                stream: OutputStream::Stdout,
                source: e,
                exit_code,
            });
        }
    };

    let stderr = match stderr_result {
        Ok(s) => s,
        Err(e) => {
            let _ = child.kill().await;
            let exit_code = capture_exit_code(child).await;
            return Err(OutputWaitError::Read {
                stream: OutputStream::Stderr,
                source: e,
                exit_code,
            });
        }
    };

    let status = child
        .wait()
        .await
        .map_err(|e| OutputWaitError::Wait { source: e })?;

    Ok((stdout, stderr, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_passthrough() {
        assert_eq!(exit_status_code_parts(Some(0), None), Some(0));
        assert_eq!(exit_status_code_parts(Some(1), None), Some(1));
        assert_eq!(exit_status_code_parts(Some(42), None), Some(42));
        assert_eq!(exit_status_code_parts(Some(255), None), Some(255));
    }

    #[cfg(unix)]
    #[test]
    fn signal_exit_code() {
        // SIGKILL (9) -> 128 + 9 = 137
        assert_eq!(exit_status_code_parts(None, Some(9)), Some(137));
        // SIGTERM (15) -> 128 + 15 = 143
        assert_eq!(exit_status_code_parts(None, Some(15)), Some(143));
        // SIGSEGV (11) -> 128 + 11 = 139
        assert_eq!(exit_status_code_parts(None, Some(11)), Some(139));
    }

    #[cfg(not(unix))]
    #[test]
    fn signal_ignored_on_non_unix() {
        assert_eq!(exit_status_code_parts(None, Some(9)), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_capture_exit_code() {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("exit 42")
            .spawn()
            .expect("failed to spawn");

        let code = capture_exit_code(&mut child).await;
        assert_eq!(code, Some(42));
    }
}
