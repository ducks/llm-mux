//! Process utilities for child process management.

use tokio::process::Child;

/// Attempt to capture the exit code from a child process.
/// Tries non-blocking first, falls back to blocking wait if process hasn't exited.
pub(crate) async fn capture_exit_code(child: &mut Child) -> Option<i32> {
    match child.try_wait() {
        Ok(Some(status)) => status.code(),
        Ok(None) => child.wait().await.ok().and_then(|status| status.code()),
        Err(_) => None,
    }
}
