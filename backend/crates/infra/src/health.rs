//! Liveness probe shared by every service (specification §19).
//!
//! Each service answers `GET /health/live` with 200 as long as its process
//! runs. The container images have no shell nor curl (distroless), so their
//! Docker `HEALTHCHECK` runs the binary itself (`api healthcheck`,
//! `worker healthcheck`, `mcp healthcheck`), which calls [`probe`].
//!
//! Readiness (`/health/ready`: PostgreSQL, Valkey and S3 reachable) comes
//! with L01-US08.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Path of the liveness endpoint.
pub const LIVE_PATH: &str = "/health/live";

/// Body of a healthy liveness answer.
pub const LIVE_BODY: &str = r#"{"status":"ok"}"#;

/// How long a probe waits for the answer.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a probe failed.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("no answer within {0:?}")]
    Timeout(Duration),
    #[error("connection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected answer: {0}")]
    Status(String),
}

/// Address to probe for a server bound to `bind`: a wildcard address
/// (`0.0.0.0`, `::`) is reached through the loopback.
pub fn probe_target(bind: SocketAddr) -> SocketAddr {
    let ip = match bind.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, bind.port())
}

/// `GET path` on `addr`; succeeds on a 200 answer.
pub async fn probe(addr: SocketAddr, path: &str, timeout: Duration) -> Result<(), ProbeError> {
    tokio::time::timeout(timeout, request(addr, path))
        .await
        .map_err(|_| ProbeError::Timeout(timeout))?
}

async fn request(addr: SocketAddr, path: &str) -> Result<(), ProbeError> {
    let mut stream = TcpStream::connect(addr).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    // The status line is enough: "HTTP/1.1 200 OK".
    let mut head = [0_u8; 64];
    let mut read = 0;
    while read < head.len() {
        let n = stream.read(&mut head[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
        if head[..read].contains(&b'\n') {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head[..read]);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    match (parts.next(), parts.next()) {
        (Some(version), Some("200")) if version.starts_with("HTTP/1.") => Ok(()),
        _ => Err(ProbeError::Status(line.to_owned())),
    }
}

/// The `healthcheck` command of a service bound to `bind`: probes its
/// liveness endpoint and reports the result as an exit code.
pub async fn healthcheck(service: &str, bind: SocketAddr) -> ExitCode {
    match probe(probe_target(bind), LIVE_PATH, PROBE_TIMEOUT).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{service}: unhealthy: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn wildcard_addresses_are_probed_on_the_loopback() {
        let v4: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert_eq!(probe_target(v4), "127.0.0.1:8080".parse().unwrap());
        let v6: SocketAddr = "[::]:8080".parse().unwrap();
        assert_eq!(probe_target(v6), "[::1]:8080".parse().unwrap());
        let fixed: SocketAddr = "10.0.0.5:9000".parse().unwrap();
        assert_eq!(probe_target(fixed), fixed);
    }

    /// A one-shot server answering `answer` to the first request.
    async fn server(answer: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 256];
            let n = socket.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /health/live HTTP/1.1\r\n"));
            socket.write_all(answer).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn a_200_answer_is_healthy() {
        let addr = server(b"HTTP/1.1 200 OK\r\ncontent-length: 15\r\n\r\n{\"status\":\"ok\"}").await;
        probe(addr, LIVE_PATH, PROBE_TIMEOUT).await.unwrap();
    }

    #[tokio::test]
    async fn another_status_is_unhealthy() {
        let addr = server(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").await;
        let err = probe(addr, LIVE_PATH, PROBE_TIMEOUT).await.unwrap_err();
        assert!(
            matches!(&err, ProbeError::Status(line) if line.contains("503")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn garbage_is_unhealthy() {
        let addr = server(b"hello\r\n").await;
        assert!(matches!(
            probe(addr, LIVE_PATH, PROBE_TIMEOUT).await,
            Err(ProbeError::Status(_))
        ));
    }

    #[tokio::test]
    async fn a_closed_port_is_unhealthy() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        assert!(matches!(
            probe(addr, LIVE_PATH, PROBE_TIMEOUT).await,
            Err(ProbeError::Io(_))
        ));
    }

    #[tokio::test]
    async fn a_silent_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _keep = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        assert!(matches!(
            probe(addr, LIVE_PATH, Duration::from_millis(100)).await,
            Err(ProbeError::Timeout(_))
        ));
    }
}
