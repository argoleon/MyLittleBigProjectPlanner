//! Integration tests: the compiled `api` binary, as Docker runs it.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_api");

#[test]
fn exits_with_an_error_naming_the_missing_variable() {
    let output = Command::new(BIN).env_clear().output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("API_BIND_ADDR"), "stderr was: {stderr}");
}

#[test]
fn exits_with_an_error_on_a_malformed_variable() {
    let output = Command::new(BIN)
        .env_clear()
        .env("API_BIND_ADDR", "127.0.0.1:1")
        .env("API_REQUEST_TIMEOUT_SECS", "soon")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("API_REQUEST_TIMEOUT_SECS"),
        "stderr was: {stderr}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn serves_ping_and_stops_cleanly_on_sigterm() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Command::new(BIN)
        .env_clear()
        .env("API_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("SHUTDOWN_GRACE_SECS", "5")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let url = format!("http://127.0.0.1:{port}/api/v1/ping");
    let deadline = Instant::now() + Duration::from_secs(10);
    let res = loop {
        match reqwest::get(&url).await {
            Ok(res) => break res,
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(err) => {
                let _ = child.kill();
                panic!("api did not start: {err}");
            }
        }
    };
    assert_eq!(res.status(), 200);

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
            panic!("api did not stop after SIGTERM");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(status.success(), "exit status: {status}");
}
