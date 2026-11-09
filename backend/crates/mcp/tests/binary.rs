//! Integration tests: the compiled `mcp` binary, as Docker runs it.

#![allow(clippy::unwrap_used)]

use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_mcp");

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn healthcheck(port: u16) -> ExitStatus {
    Command::new(BIN)
        .env_clear()
        .env("MCP_BIND_ADDR", format!("0.0.0.0:{port}"))
        .arg("healthcheck")
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

#[test]
fn exits_with_an_error_naming_the_missing_variable() {
    let output = Command::new(BIN).env_clear().output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MCP_API_BASE_URL"), "stderr was: {stderr}");
}

#[test]
fn rejects_a_base_url_without_scheme() {
    let output = Command::new(BIN)
        .env_clear()
        .env("MCP_BIND_ADDR", "127.0.0.1:1")
        .env("MCP_API_BASE_URL", "api:8080")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MCP_API_BASE_URL"), "stderr was: {stderr}");
}

#[test]
fn unexpected_arguments_print_usage_and_exit_with_2() {
    let output = Command::new(BIN).env_clear().arg("serve-me").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: mcp"));
}

#[test]
fn healthcheck_requires_the_bind_address() {
    let output = Command::new(BIN).env_clear().arg("healthcheck").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("MCP_BIND_ADDR"));
}

#[test]
fn is_healthy_while_running() {
    let port = free_port();
    assert!(!healthcheck(port).success(), "nothing listens yet");

    let mut child = Command::new(BIN)
        .env_clear()
        .env("MCP_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("MCP_API_BASE_URL", "http://127.0.0.1:1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !healthcheck(port).success() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("mcp never became healthy");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    child.kill().unwrap();
    child.wait().unwrap();
}
