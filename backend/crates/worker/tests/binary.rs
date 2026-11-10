//! Integration tests: the compiled `worker` binary, as Docker runs it.

#![allow(clippy::unwrap_used)]

use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_worker");

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The settings the worker requires. Nothing listens at these addresses:
/// the worker starts anyway (it connects lazily) and logs that it cannot
/// claim jobs.
fn required_env(command: &mut Command) -> &mut Command {
    command
        .env("DATABASE_URL", "postgres://nobody@127.0.0.1:1/none")
        .env("DATABASE_ACQUIRE_TIMEOUT_SECS", "1")
        .env("SMTP_URL", "smtp://127.0.0.1:1")
        .env("MAIL_FROM", "Planner <noreply@example.org>")
}

fn healthcheck(port: u16) -> ExitStatus {
    Command::new(BIN)
        .env_clear()
        .env("WORKER_HEALTH_ADDR", format!("0.0.0.0:{port}"))
        .arg("healthcheck")
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

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
    let mut child = required_env(Command::new(BIN).env_clear())
        .env("WORKER_POLL_INTERVAL_MS", "50")
        // A free port, so parallel tests never compete for the default one.
        .env("WORKER_HEALTH_ADDR", format!("127.0.0.1:{}", free_port()))
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

#[test]
fn unexpected_arguments_print_usage_and_exit_with_2() {
    let output = Command::new(BIN).env_clear().arg("work").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: worker"));
}

#[cfg(unix)]
#[test]
fn is_healthy_while_running_and_stops_cleanly_on_sigterm() {
    let port = free_port();
    assert!(!healthcheck(port).success(), "nothing listens yet");

    let mut child = required_env(Command::new(BIN).env_clear())
        .env("WORKER_HEALTH_ADDR", format!("127.0.0.1:{port}"))
        .env("SHUTDOWN_GRACE_SECS", "5")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !healthcheck(port).success() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the worker never became healthy");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

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
            panic!("the worker did not stop after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status: {status}");
    assert!(!healthcheck(port).success(), "the health endpoint stopped too");
}
