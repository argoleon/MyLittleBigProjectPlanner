//! Integration tests: the API served on a real TCP socket.

// Test helpers sit outside `#[test]` functions; a panic there is a test failure.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use infra::config::{ApiConfig, CommonConfig};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn config() -> ApiConfig {
    ApiConfig {
        common: CommonConfig {
            shutdown_grace: Duration::from_secs(5),
        },
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 1024,
    }
}

async fn listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (listener, base)
}

#[tokio::test]
async fn ping_answers_json_on_a_random_port() {
    let (listener, base) = listener().await;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(api::serve(
        listener,
        api::app(&config()),
        async move {
            let _ = stop_rx.await;
        },
        Duration::from_secs(5),
    ));

    let res = reqwest::get(format!("{base}/api/v1/ping")).await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers().contains_key("x-request-id"));
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    let missing = reqwest::get(format!("{base}/ping")).await.unwrap();
    assert_eq!(missing.status(), 404, "routes live under /api/v1");

    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn in_flight_request_completes_after_shutdown_signal() {
    let (listener, base) = listener().await;
    let slow = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            "done"
        }),
    );
    let app = api::with_middleware(slow, &api::HttpSettings::from(&config()));
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(api::serve(
        listener,
        app,
        async move {
            let _ = stop_rx.await;
        },
        Duration::from_secs(5),
    ));

    let request = tokio::spawn(reqwest::get(format!("{base}/slow")));
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_tx.send(()).unwrap();

    let res = request.await.unwrap().unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "done");
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_does_not_wait_longer_than_the_grace_period() {
    let (listener, base) = listener().await;
    let stuck = Router::new().route(
        "/stuck",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            "never"
        }),
    );
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(api::serve(
        listener,
        stuck,
        async move {
            let _ = stop_rx.await;
        },
        Duration::from_millis(300),
    ));

    let _request = tokio::spawn(reqwest::get(format!("{base}/stuck")));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
}
