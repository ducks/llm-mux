//! Process utilities for child process management.

use tokio::process::Child;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

/// Extract exit code from ExitStatus, using 128+signal for signal-terminated processes on Unix.
pub(crate) fn exit_status_code(status: &std::process::ExitStatus) -> Option<i32> {
    if let Some(code) = status.code() {
        return Some(code);
    }
    #[cfg(unix)]
    {
        if let Some(signal) = status.signal() {
            return Some(128 + signal);
        }
    }
    None
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
