//! Integration tests: the compiled `worker` binary, as Docker runs it.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_worker");

#[test]
fn exits_with_an_error_naming_a_malformed_variable() {
    let output = Command::new(BIN)
        .env_clear()
        .env("WORKER_POLL_INTERVAL_MS", "often")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("WORKER_POLL_INTERVAL_MS"), "stderr was: {stderr}");
}

#[cfg(unix)]
#[test]
fn stops_cleanly_on_sigterm() {
    let mut child = Command::new(BIN)
        .env_clear()
        .env("WORKER_POLL_INTERVAL_MS", "50")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Give the runtime time to install its signal handlers.
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "worker is running");

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("worker did not stop after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status: {status}");
}
