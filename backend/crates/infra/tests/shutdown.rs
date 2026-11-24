//! Integration test of the shutdown signals: a SIGTERM received while the
//! process starts, before anything waits for it, must not be lost nor kill
//! the process (the worker's `stops_cleanly_on_sigterm` failed so under
//! load: its handler was only installed once startup was over).
//!
//! In a test binary of its own: it sends SIGTERM to its own process.

#![allow(clippy::unwrap_used)]

use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
#[tokio::test]
async fn a_sigterm_received_before_anything_waits_is_kept_for_the_wait() {
    let shutdown = infra::shutdown::listen();

    // Startup goes on; the orchestrator stops the process meanwhile.
    let sent = Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Still alive, and the wait sees the signal.
    tokio::time::timeout(Duration::from_secs(5), shutdown.wait())
        .await
        .expect("the SIGTERM sent during startup was lost");
}
