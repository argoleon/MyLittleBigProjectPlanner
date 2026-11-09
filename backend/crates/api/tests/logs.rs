//! Integration test: one JSON log line per request, with its correlation ID.
//! Alone in its test binary: `tracing` caches per process which spans and
//! events a subscriber wants, so tests running without one would disable
//! the request span for this one.

#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use api::{AppState, HttpSettings};
use domain::tenancy::Tenancy;
use infra::config::LogFormat;
use infra::sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;

/// The API on a random port; returns its base URL.
async fn serve(state: AppState) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let settings = HttpSettings {
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 1024,
    };
    let app = api::app(state, &settings);
    tokio::spawn(api::serve(
        listener,
        app,
        std::future::pending(),
        Duration::from_secs(1),
    ));
    base
}

fn single() -> Tenancy {
    Tenancy::Single {
        default_slug: "default".to_owned(),
    }
}

/// Captures what the log layer writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn lines(&self) -> Vec<serde_json::Value> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|_| panic!("not a JSON line: {line}")))
            .collect()
    }
}

// Current-thread runtime: the server tasks run on this thread, where the
// capturing subscriber is installed.
#[tokio::test(flavor = "current_thread")]
async fn every_request_is_logged_as_one_json_line_with_its_correlation_id() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .with(infra::telemetry::log_layer(LogFormat::Json, move || {
            writer.clone()
        }));
    let _guard = tracing::subscriber::set_default(subscriber);

    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    let base = serve(AppState::new(pool, single())).await;
    let client = reqwest::Client::new();

    // A request without correlation ID gets one.
    let res = client
        .get(format!("{base}/api/v1/ping"))
        .header("x-forwarded-for", "203.0.113.7, 10.0.0.2")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let generated = res.headers()["x-request-id"].to_str().unwrap().to_owned();
    assert!(uuid::Uuid::parse_str(&generated).is_ok(), "{generated}");

    // A caller's usable correlation ID is kept; an unusable one is replaced.
    let kept = client
        .get(format!("{base}/api/v1/ping"))
        .header("x-request-id", "caller-42")
        .send()
        .await
        .unwrap();
    assert_eq!(kept.headers()["x-request-id"], "caller-42");
    let replaced = client
        .get(format!("{base}/api/v1/ping"))
        .header("x-request-id", "x".repeat(200))
        .send()
        .await
        .unwrap();
    let replacement = replaced.headers()["x-request-id"].to_str().unwrap().to_owned();
    assert!(uuid::Uuid::parse_str(&replacement).is_ok(), "{replacement}");

    let finished: Vec<serde_json::Value> = captured
        .lines()
        .into_iter()
        .filter(|line| line["message"] == "finished processing request")
        .collect();
    assert_eq!(finished.len(), 3, "one line per request: {finished:#?}");

    let first = &finished[0];
    assert_eq!(first["level"], "INFO");
    assert_eq!(first["status"], 200);
    assert!(first["latency"].is_string(), "{first}");
    assert!(first["timestamp"].is_string(), "{first}");
    let span = &first["span"];
    assert_eq!(span["request_id"], generated, "{first}");
    assert_eq!(span["method"], "GET");
    assert_eq!(span["path"], "/api/v1/ping");
    assert_eq!(span["route"], "/api/v1/ping");
    assert_eq!(span["client_ip"], "203.0.113.7");

    assert_eq!(finished[1]["span"]["request_id"], "caller-42");
    assert_eq!(finished[2]["span"]["request_id"], replacement);
}
