//! Process utilities for child process management.

use tokio::process::Child;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

fn exit_status_code_parts(code: Option<i32>, signal: Option<i32>) -> Option<i32> {
    if let Some(code) = code {
        return Some(code);
    }
    #[cfg(unix)]
    {
        if let Some(signal) = signal {
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
