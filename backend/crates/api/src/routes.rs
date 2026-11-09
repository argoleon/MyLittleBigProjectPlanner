//! Route table. Every public route lives under `/api/v1`.
//! JSON field names are camelCase.

use axum::routing::get;
use axum::{Json, Router};

use crate::problem::ErrorCode;
use crate::reference_items;
use infra::uuid::Uuid;
use serde::Serialize;

use crate::problem::Problem;
use crate::tenant::{AppState, TenantTx};

pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/ping", get(ping))
        .route("/tenant", get(current_tenant))
        .route(
            "/reference-items",
            get(reference_items::list).post(reference_items::create),
        )
        .route(
            "/reference-items/{id}",
            get(reference_items::read)
                .put(reference_items::update)
                .delete(reference_items::delete),
        );
    Router::new()
        .nest("/api/v1", v1)
        .fallback(|| async { Problem::new(ErrorCode::NotFound) })
        .with_state(state)
}

#[derive(Debug, Serialize)]
struct Ping {
    status: &'static str,
    version: &'static str,
}

/// Liveness of the HTTP stack; touches no database and needs no tenant.
async fn ping() -> Json<Ping> {
    Json(Ping {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TenantView {
    id: Uuid,
    slug: String,
    name: String,
    default_locale: String,
    time_zone: String,
}

/// The tenant addressed by the request. The query has no tenant filter on
/// purpose: row-level security alone restricts it to the current tenant.
async fn current_tenant(mut tx: TenantTx) -> Result<Json<TenantView>, Problem> {
    let tenant = sqlx::query_as!(
        TenantView,
        "SELECT id, slug, name, default_locale, time_zone FROM tenant"
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(tenant))
}
