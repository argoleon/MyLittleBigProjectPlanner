//! Route table. Every public route lives under `/api/v1`.

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

pub fn router() -> Router {
    Router::new().nest("/api/v1", Router::new().route("/ping", get(ping)))
}

#[derive(Debug, Serialize)]
struct Ping {
    status: &'static str,
    version: &'static str,
}

async fn ping() -> Json<Ping> {
    Json(Ping {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}
