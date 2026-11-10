//! Health endpoints shared by every service (specification §19).
//!
//! - `GET /health/live` answers 200 as long as the process runs.
//! - `GET /health/ready` answers 200 when every dependency the service needs
//!   to serve requests is reachable (PostgreSQL, Valkey, S3, the API for
//!   mcp), 503 otherwise, with the state of each one. Compose and Traefik
//!   use it: an instance that cannot serve gets no traffic.
//!
//! The container images have no shell nor curl (distroless), so their Docker
//! health check runs the binary itself (`api healthcheck [--ready]`, and the
//! same for worker and mcp), which calls [`probe`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use sqlx::postgres::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};

/// Path of the liveness endpoint.
pub const LIVE_PATH: &str = "/health/live";

/// Path of the readiness endpoint.
pub const READY_PATH: &str = "/health/ready";

/// Body of a healthy liveness answer.
pub const LIVE_BODY: &str = r#"{"status":"ok"}"#;

/// How long a probe waits for the answer.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a readiness check waits for one dependency.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

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

/// `GET path` on `target` (`SocketAddr` or `"host:port"`); succeeds on 200.
pub async fn probe<A: ToSocketAddrs>(target: A, path: &str, timeout: Duration) -> Result<(), ProbeError> {
    tokio::time::timeout(timeout, request(target, path))
        .await
        .map_err(|_| ProbeError::Timeout(timeout))?
}

async fn request<A: ToSocketAddrs>(target: A, path: &str) -> Result<(), ProbeError> {
    let mut stream = TcpStream::connect(target).await?;
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

/// The `healthcheck` command of a service bound to `bind`: probes `path`
/// ([`LIVE_PATH`] or [`READY_PATH`]) and reports the result as an exit code.
pub async fn healthcheck(service: &str, bind: SocketAddr, path: &str) -> ExitCode {
    match probe(probe_target(bind), path, PROBE_TIMEOUT).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{service}: {path}: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The endpoint probed by `healthcheck [--ready]`.
pub fn healthcheck_path(args: &[&str]) -> Option<&'static str> {
    match args {
        [] => Some(LIVE_PATH),
        ["--ready"] => Some(READY_PATH),
        _ => None,
    }
}

// --- Readiness ---------------------------------------------------------------

/// A dependency a service needs to serve requests.
#[derive(Debug, Clone)]
pub enum Dependency {
    /// PostgreSQL, through the service's pool: `SELECT 1`.
    Postgres(PgPool),
    /// A TCP service (Valkey, S3 storage): its port accepts connections.
    Tcp { name: &'static str, address: String },
    /// Another of our HTTP services: its liveness endpoint answers 200.
    Http { name: &'static str, address: String },
}

impl Dependency {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Postgres(_) => "postgres",
            Self::Tcp { name, .. } | Self::Http { name, .. } => name,
        }
    }

    async fn check(&self) -> Result<(), String> {
        let check = async {
            match self {
                Self::Postgres(pool) => sqlx::query("SELECT 1")
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                Self::Tcp { address, .. } => TcpStream::connect(address.as_str())
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                Self::Http { address, .. } => probe(address.as_str(), LIVE_PATH, CHECK_TIMEOUT)
                    .await
                    .map_err(|e| e.to_string()),
            }
        };
        tokio::time::timeout(CHECK_TIMEOUT, check)
            .await
            .unwrap_or_else(|_| Err(format!("no answer within {CHECK_TIMEOUT:?}")))
    }
}

/// `host:port` of a dependency URL such as `redis://:secret@valkey:6379/0` or
/// `http://seaweedfs:8333`; the port defaults to the scheme's. Credentials
/// are dropped. `None` if the URL is not understood.
pub fn address_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    if host_port.is_empty() {
        return None;
    }
    let has_port = match host_port.rsplit_once(':') {
        // An IPv6 literal without port ends with ']'.
        Some((_, port)) => !port.ends_with(']') && port.parse::<u16>().is_ok(),
        None => false,
    };
    if has_port {
        return Some(host_port.to_owned());
    }
    let default_port = match scheme {
        "redis" | "rediss" | "valkey" | "valkeys" => 6379,
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    Some(format!("{host_port}:{default_port}"))
}

/// Result of a readiness check: the state of each dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyReport {
    pub checks: Vec<(&'static str, bool)>,
}

impl ReadyReport {
    /// Ready when every dependency is up (and when there is none).
    pub fn is_ready(&self) -> bool {
        self.checks.iter().all(|(_, up)| *up)
    }

    /// 200 when ready, 503 otherwise.
    pub fn status_code(&self) -> u16 {
        if self.is_ready() { 200 } else { 503 }
    }

    /// `{"status":"ready","checks":{"postgres":"up"}}`. Error details are
    /// logged, never returned: the endpoint needs no authentication.
    pub fn to_json(&self) -> String {
        let checks: serde_json::Map<String, serde_json::Value> = self
            .checks
            .iter()
            .map(|(name, up)| ((*name).to_owned(), (if *up { "up" } else { "down" }).into()))
            .collect();
        serde_json::json!({
            "status": if self.is_ready() { "ready" } else { "unavailable" },
            "checks": checks,
        })
        .to_string()
    }
}

/// Checks every dependency concurrently, each within [`CHECK_TIMEOUT`];
/// failures are logged with their cause.
pub async fn readiness(dependencies: &[Dependency]) -> ReadyReport {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, dependency) in dependencies.iter().cloned().enumerate() {
        tasks.spawn(async move { (index, dependency.name(), dependency.check().await) });
    }
    let mut results = vec![None; dependencies.len()];
    while let Some(joined) = tasks.join_next().await {
        if let Ok((index, name, result)) = joined {
            if let Err(cause) = &result {
                tracing::warn!(dependency = name, %cause, "dependency unavailable");
            }
            results[index] = Some((name, result.is_ok()));
        }
    }
    // A check that panicked counts as down.
    let checks = results
        .into_iter()
        .zip(dependencies)
        .map(|(result, dependency)| result.unwrap_or((dependency.name(), false)))
        .collect();
    ReadyReport { checks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn healthcheck_arguments_select_the_endpoint() {
        assert_eq!(healthcheck_path(&[]), Some(LIVE_PATH));
        assert_eq!(healthcheck_path(&["--ready"]), Some(READY_PATH));
        assert_eq!(healthcheck_path(&["--live", "x"]), None);
    }

    #[test]
    fn addresses_of_dependency_urls() {
        assert_eq!(address_of("redis://valkey:6379").as_deref(), Some("valkey:6379"));
        assert_eq!(
            address_of("redis://:s3cret@valkey/0").as_deref(),
            Some("valkey:6379")
        );
        assert_eq!(
            address_of("redis://user:p@ss@cache:7000").as_deref(),
            Some("cache:7000")
        );
        assert_eq!(
            address_of("http://seaweedfs:8333").as_deref(),
            Some("seaweedfs:8333")
        );
        assert_eq!(
            address_of("https://s3.example.org/bucket").as_deref(),
            Some("s3.example.org:443")
        );
        assert_eq!(address_of("http://[::1]:9000").as_deref(), Some("[::1]:9000"));
        assert_eq!(address_of("http://[::1]").as_deref(), Some("[::1]:80"));
        assert_eq!(address_of("valkey:6379"), None, "a URL needs a scheme");
        assert_eq!(address_of("ftp://files"), None, "no default port");
        assert_eq!(address_of("http://"), None);
    }

    #[test]
    fn readiness_aggregates_the_checks() {
        let ready = ReadyReport {
            checks: vec![("postgres", true), ("valkey", true)],
        };
        assert!(ready.is_ready());
        assert_eq!(ready.status_code(), 200);
        assert_eq!(
            ready.to_json(),
            r#"{"checks":{"postgres":"up","valkey":"up"},"status":"ready"}"#
        );

        let one_down = ReadyReport {
            checks: vec![("postgres", true), ("s3", false)],
        };
        assert!(!one_down.is_ready());
        assert_eq!(one_down.status_code(), 503);
        assert_eq!(
            one_down.to_json(),
            r#"{"checks":{"postgres":"up","s3":"down"},"status":"unavailable"}"#
        );

        let none = ReadyReport { checks: vec![] };
        assert!(none.is_ready(), "a service without dependency is ready");
    }

    #[tokio::test]
    async fn readiness_checks_each_dependency() {
        let open = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open_addr = open.local_addr().unwrap().to_string();
        // Nothing listens on port 1. (A freed ephemeral port could be taken
        // again by a test running in parallel, or by the server below.)
        let closed_addr = "127.0.0.1:1".to_owned();
        let live = server(b"HTTP/1.1 200 OK\r\n\r\n").await.to_string();

        let report = readiness(&[
            Dependency::Tcp {
                name: "valkey",
                address: open_addr,
            },
            Dependency::Tcp {
                name: "s3",
                address: closed_addr,
            },
            Dependency::Http {
                name: "api",
                address: live,
            },
        ])
        .await;
        assert_eq!(
            report.checks,
            vec![("valkey", true), ("s3", false), ("api", true)]
        );
        assert_eq!(report.status_code(), 503);
    }

    #[tokio::test]
    async fn an_unreachable_database_is_down_within_the_timeout() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(30))
            .connect_lazy("postgres://nobody@10.255.255.1:5432/none")
            .unwrap();
        let started = std::time::Instant::now();
        let report = readiness(&[Dependency::Postgres(pool)]).await;
        assert_eq!(report.checks, vec![("postgres", false)]);
        assert!(started.elapsed() < CHECK_TIMEOUT + Duration::from_secs(1));
    }

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
