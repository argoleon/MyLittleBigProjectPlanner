//! Integration tests: readiness of the API (`/health/ready`).

#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use api::{AppState, HttpSettings};
use common::{json, multi, start};
use domain::tenancy::Tenancy;
use infra::health::Dependency;
use infra::sqlx::postgres::PgPoolOptions;
use testkit::TestDb;
use tokio::net::TcpListener;

#[tokio::test]
async fn ready_when_the_database_answers() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let res = api
        .client
        .get(format!("{}/health/ready", api.base))
        .send()
        .await
        .unwrap();
    let (status, body) = json(res).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        serde_json::json!({"status": "ready", "checks": {"postgres": "up"}})
    );
}

/// The API with `state`, on a random port; returns its base URL.
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

#[tokio::test]
async fn unavailable_when_a_dependency_is_down() {
    // A database nobody listens to, as when PostgreSQL is stopped.
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://app@127.0.0.1:1/mlbpp")
        .unwrap();
    let base = serve(AppState::new(pool, single())).await;

    let res = reqwest::get(format!("{base}/health/ready")).await.unwrap();
    let (status, body) = json(res).await;
    assert_eq!(status, 503);
    assert_eq!(
        body,
        serde_json::json!({"status": "unavailable", "checks": {"postgres": "down"}})
    );

    // Liveness does not depend on the database.
    let live = reqwest::get(format!("{base}/health/live")).await.unwrap();
    assert_eq!(live.status(), 200);
}

#[tokio::test]
async fn readiness_covers_the_shared_services() {
    let db = TestDb::new().await;
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_addr = closed.local_addr().unwrap().to_string();
    drop(closed);
    let state = AppState::new(db.app_pool.clone(), single()).with_dependencies([Dependency::Tcp {
        name: "valkey",
        address: closed_addr,
    }]);
    let base = serve(state).await;

    let (status, body) = json(reqwest::get(format!("{base}/health/ready")).await.unwrap()).await;
    assert_eq!(status, 503);
    assert_eq!(
        body["checks"],
        serde_json::json!({"postgres": "up", "valkey": "down"})
    );
}
