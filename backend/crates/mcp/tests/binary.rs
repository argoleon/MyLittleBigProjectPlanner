//! Integration tests: the compiled `mcp` binary.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_mcp");

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
