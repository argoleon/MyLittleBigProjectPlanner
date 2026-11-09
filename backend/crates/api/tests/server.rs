//! Integration tests: the API served on a real TCP socket.

// Test helpers sit outside `#[test]` functions; a panic there is a test failure.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use api::{AppState, HttpSettings};
use axum::Router;
use axum::routing::get;
use domain::tenancy::Tenancy;
use infra::sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn settings() -> HttpSettings {
    HttpSettings {
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 1024,
    }
}

/// State whose pool never connects: these tests do not touch the database.
fn offline_state() -> AppState {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    AppState::new(
        pool,
        Tenancy::Single {
            default_slug: "default".to_owned(),
        },
    )
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
        api::app(offline_state(), &settings()),
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
    let app = api::with_middleware(slow, &settings());
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

/// Serves `app` on a random port; returns the base URL and the stop handle.
async fn serve_app(app: Router) -> (String, oneshot::Sender<()>) {
    let (listener, base) = listener().await;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(api::serve(
        listener,
        app,
        async move {
            let _ = stop_rx.await;
        },
        Duration::from_secs(1),
    ));
    (base, stop_tx)
}

async fn problem(res: reqwest::Response) -> (u16, serde_json::Value) {
    let status = res.status().as_u16();
    assert_eq!(res.headers()["content-type"], "application/problem+json");
    assert!(res.headers().contains_key("x-request-id"));
    (status, res.json().await.unwrap())
}

#[tokio::test]
async fn framework_errors_are_problem_details() {
    let (base, _stop) = serve_app(api::app(offline_state(), &settings())).await;
    let client = reqwest::Client::new();

    let (status, body) = problem(client.get(format!("{base}/api/v1/nowhere")).send().await.unwrap()).await;
    assert_eq!((status, body["code"].as_str().unwrap()), (404, "not_found"));

    let res = client.delete(format!("{base}/api/v1/ping")).send().await.unwrap();
    assert!(res.headers().contains_key("allow"), "Allow header kept");
    let (status, body) = problem(res).await;
    assert_eq!(
        (status, body["code"].as_str().unwrap()),
        (405, "method_not_allowed")
    );

    let res = client
        .post(format!("{base}/api/v1/reference-items"))
        .header("content-type", "application/json")
        .body(vec![b' '; 2048])
        .send()
        .await
        .unwrap();
    let (status, body) = problem(res).await;
    assert_eq!(
        (status, body["code"].as_str().unwrap()),
        (413, "payload_too_large")
    );
}

#[tokio::test]
async fn a_panicking_handler_is_a_generic_500() {
    let routes = Router::new().route(
        "/boom",
        get(|| async {
            if std::hint::black_box(true) {
                panic!("secret internal state");
            }
            "unreachable"
        }),
    );
    let (base, _stop) = serve_app(api::with_middleware(routes, &settings())).await;
    let res = reqwest::get(format!("{base}/boom")).await.unwrap();
    let (status, body) = problem(res).await;
    assert_eq!(status, 500);
    assert_eq!(body["code"], "internal");
    assert!(!body.to_string().contains("secret"), "{body}");
}
