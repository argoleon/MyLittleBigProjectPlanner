//! Integration test of `infra::telemetry::init`: a real OTLP/HTTP collector
//! (a minimal local server) must receive the traces and the metrics. One
//! test only: it installs the global logger of this test process.

#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use infra::config::{LogFormat, TelemetryConfig};
use infra::telemetry::{TelemetryError, init};

/// A minimal OTLP/HTTP collector: answers 200 to every POST and records
/// its path.
fn collector() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let paths = received.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            paths.lock().unwrap().push(request_line.trim().to_owned());
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .unwrap();
        }
    });
    (endpoint, received)
}

#[test]
fn exports_traces_and_metrics_to_the_collector() {
    let (endpoint, received) = collector();
    let config = TelemetryConfig {
        log_format: LogFormat::Json,
        otlp_endpoint: Some(endpoint),
    };
    // As in the services: initialized outside of any Tokio runtime.
    let telemetry = init("test", &config).unwrap();
    assert!(telemetry.exports());
    assert!(matches!(
        init("test", &config),
        Err(TelemetryError::AlreadyInstalled(_))
    ));

    // A span, as the request layer creates them, and a metric.
    tracing::info_span!("request", otel.name = "GET /test").in_scope(|| tracing::info!("handled"));
    // Kept alive until the flush, as the services keep their instruments.
    let counter = opentelemetry::global::meter("test")
        .u64_counter("test.requests")
        .build();
    counter.add(1, &[]);

    let started = Instant::now();
    telemetry.shutdown(); // flushes both
    drop(counter);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "shutdown took {:?}",
        started.elapsed()
    );

    let paths = received.lock().unwrap().clone();
    assert!(paths.iter().any(|p| p == "POST /v1/traces HTTP/1.1"), "{paths:?}");
    assert!(
        paths.iter().any(|p| p == "POST /v1/metrics HTTP/1.1"),
        "{paths:?}"
    );
}
