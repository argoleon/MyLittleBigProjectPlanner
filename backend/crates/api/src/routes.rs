//! Route table. Every public route lives under `/api/v1`.
//! JSON field names are camelCase.
//!
//! Routes are declared with `utoipa_axum::routes!`, which registers each
//! handler and its `#[utoipa::path]` documentation together: a route cannot
//! be served without appearing in the OpenAPI contract. The only exception
//! is the liveness probe, meant for the platform, not for API clients.

use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use infra::uuid::Uuid;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::openapi;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::reference_items;
use crate::tenant::{AppState, TenantTx};

/// OpenAPI tag of the technical endpoints.
const SYSTEM_TAG: &str = "system";

pub fn router(state: AppState) -> Router {
    let (api, contract) = documented_routes().split_for_parts();
    api.merge(openapi::docs(contract))
        .route(infra::health::LIVE_PATH, get(live))
        .fallback(|| async { Problem::new(ErrorCode::NotFound) })
        .with_state(state)
}

/// `GET /health/live`: the process answers. Touches no database.
async fn live() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        infra::health::LIVE_BODY,
    )
}

/// Every documented route, with the contract they make up.
pub(crate) fn documented_routes() -> OpenApiRouter<AppState> {
    let v1 = OpenApiRouter::new()
        .routes(routes!(ping))
        .routes(routes!(current_tenant))
        .routes(routes!(reference_items::list, reference_items::create))
        .routes(routes!(
            reference_items::read,
            reference_items::update,
            reference_items::delete
        ));
    OpenApiRouter::with_openapi(openapi::base()).nest("/api/v1", v1)
}

#[derive(Debug, Serialize, ToSchema)]
struct Ping {
    #[schema(example = "ok")]
    status: &'static str,
    /// Version of the API binary.
    #[schema(example = "0.1.0")]
    version: &'static str,
}

/// Liveness of the HTTP stack; touches no database and needs no tenant.
#[utoipa::path(
    get,
    path = "/ping",
    tag = SYSTEM_TAG,
    operation_id = "ping",
    summary = "Check that the API answers",
    responses((status = 200, description = "The API is up", body = Ping))
)]
async fn ping() -> Json<Ping> {
    Json(Ping {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Tenant)]
struct TenantView {
    id: Uuid,
    #[schema(example = "acme")]
    slug: String,
    #[schema(example = "Acme")]
    name: String,
    /// Language used when the request names no supported language.
    #[schema(example = "fr")]
    default_locale: String,
    #[schema(example = "Europe/Paris")]
    time_zone: String,
}

/// The tenant addressed by the request. The query has no tenant filter on
/// purpose: row-level security alone restricts it to the current tenant.
#[utoipa::path(
    get,
    path = "/tenant",
    tag = SYSTEM_TAG,
    operation_id = "getCurrentTenant",
    summary = "The tenant addressed by the request",
    description = "Resolved from the request's host name in multi-tenant mode.",
    responses(
        (status = 200, description = "The current tenant", body = TenantView),
        (status = 404, description = "`tenant.not_found`: no tenant for this host", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
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
