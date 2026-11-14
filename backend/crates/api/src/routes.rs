//! Route table. Every public route lives under `/api/v1`.
//! JSON field names are camelCase.
//!
//! Routes are declared with `utoipa_axum::routes!`, which registers each
//! handler and its `#[utoipa::path]` documentation together: a route cannot
//! be served without appearing in the OpenAPI contract. The only exceptions
//! are the health probes, meant for the platform, not for API clients.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use infra::uuid::Uuid;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::account;
use crate::auth;
use crate::config_fields;
use crate::configs;
use crate::gdpr;
use crate::grants;
use crate::groups;
use crate::hierarchy;
use crate::invitations;
use crate::issue_types;
use crate::me;
use crate::members;
use crate::mfa;
use crate::nodes;
use crate::openapi;
use crate::platform;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::reference_items;
use crate::roles;
use crate::sso;
use crate::tenant::{AppState, PublicTenantTx};
use crate::transition_rules;
use crate::workflows;

/// OpenAPI tag of the technical endpoints.
const SYSTEM_TAG: &str = "system";

pub fn router(state: AppState) -> Router {
    let (api, contract) = documented_routes().split_for_parts();
    api.merge(openapi::docs(contract))
        .route(infra::health::LIVE_PATH, get(live))
        .route(infra::health::READY_PATH, get(ready))
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

/// `GET /health/ready`: 200 when PostgreSQL and the shared services answer,
/// 503 otherwise, with the state of each one.
async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let report = infra::health::readiness(&state.dependencies).await;
    let status = StatusCode::from_u16(report.status_code()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        report.to_json(),
    )
}

/// Every documented route, with the contract they make up.
pub(crate) fn documented_routes() -> OpenApiRouter<AppState> {
    let v1 = OpenApiRouter::new()
        .routes(routes!(ping))
        .routes(routes!(current_tenant))
        .routes(routes!(auth::sign_in))
        .routes(routes!(auth::sign_out))
        .routes(routes!(auth::current_session))
        .routes(routes!(auth::request_password_reset))
        .routes(routes!(auth::confirm_password_reset))
        .routes(routes!(auth::verify_sign_in))
        .routes(routes!(me::arrive))
        .routes(routes!(me::me))
        .routes(routes!(me::memberships))
        .routes(routes!(me::switch))
        .routes(routes!(account::update_preferences))
        .routes(routes!(account::change_password))
        .routes(routes!(account::sessions))
        .routes(routes!(account::revoke_session))
        .routes(routes!(gdpr::create_export))
        .routes(routes!(gdpr::get_export))
        .routes(routes!(gdpr::download_export))
        .routes(routes!(gdpr::erase_me))
        .routes(routes!(gdpr::erase_member))
        .routes(routes!(gdpr::erase_account))
        .routes(routes!(members::list))
        .routes(routes!(members::read, members::update, members::remove))
        .routes(routes!(invitations::list, invitations::create))
        .routes(routes!(invitations::revoke))
        .routes(routes!(invitations::resend))
        .routes(routes!(invitations::preview))
        .routes(routes!(invitations::accept))
        .routes(routes!(groups::list, groups::create))
        .routes(routes!(groups::read, groups::update, groups::delete))
        .routes(routes!(groups::add_member, groups::remove_member))
        .routes(routes!(roles::list, roles::create))
        .routes(routes!(nodes::list, nodes::create))
        .routes(routes!(nodes::read, nodes::update))
        .routes(routes!(nodes::set_confidentiality))
        .routes(routes!(nodes::set_archiving))
        .routes(routes!(nodes::parents))
        .routes(routes!(nodes::children))
        .routes(routes!(nodes::link_parent, nodes::unlink_parent))
        .routes(routes!(nodes::parent_candidates))
        .routes(routes!(hierarchy::browse))
        .routes(routes!(hierarchy::aggregate))
        .routes(routes!(grants::access))
        .routes(routes!(grants::grant))
        .routes(routes!(grants::revoke))
        .routes(routes!(grants::candidates))
        .routes(routes!(grants::grantable_roles))
        .routes(routes!(grants::my_permissions))
        .routes(routes!(roles::read, roles::update, roles::delete))
        .routes(routes!(configs::list, configs::create))
        .routes(routes!(configs::read, configs::rename, configs::delete))
        .routes(routes!(configs::projects))
        .routes(routes!(configs::duplicate))
        .routes(routes!(configs::open_draft, configs::discard_draft))
        .routes(routes!(configs::impact))
        .routes(routes!(configs::publish))
        .routes(routes!(configs::node_config, configs::set_node_config))
        .routes(routes!(issue_types::list, issue_types::create))
        .routes(routes!(issue_types::reorder))
        .routes(routes!(
            issue_types::read,
            issue_types::update,
            issue_types::delete
        ))
        .routes(routes!(config_fields::list, config_fields::create))
        .routes(routes!(
            config_fields::read,
            config_fields::update,
            config_fields::delete
        ))
        .routes(routes!(transition_rules::roles))
        .routes(routes!(transition_rules::candidates))
        .routes(routes!(workflows::list, workflows::create))
        .routes(routes!(workflows::update, workflows::delete))
        .routes(routes!(workflows::create_status))
        .routes(routes!(workflows::update_status, workflows::delete_status))
        .routes(routes!(workflows::create_transition))
        .routes(routes!(
            workflows::update_transition,
            workflows::delete_transition
        ))
        .routes(routes!(mfa::status))
        .routes(routes!(mfa::setup))
        .routes(routes!(mfa::confirm))
        .routes(routes!(mfa::disable))
        .routes(routes!(sso::get_policy, sso::update_policy))
        .routes(routes!(sso::list_providers, sso::create_provider))
        .routes(routes!(
            sso::read_provider,
            sso::update_provider,
            sso::delete_provider
        ))
        .routes(routes!(sso::test_provider))
        .routes(routes!(sso::sign_in_providers))
        .routes(routes!(sso::start))
        .routes(routes!(sso::callback))
        .routes(routes!(platform::list, platform::create))
        .routes(routes!(platform::read, platform::update, platform::delete))
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
    security(()),
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
    description = "Resolved from the request's host name in multi-tenant mode. Needs no session: \
                   the sign-in page shows the tenant's name.",
    security(()),
    responses(
        (status = 200, description = "The current tenant", body = TenantView),
        (status = 404, description = "`tenant.not_found`: no tenant for this host", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
async fn current_tenant(mut tx: PublicTenantTx) -> Result<Json<TenantView>, Problem> {
    let tenant = sqlx::query_as!(
        TenantView,
        "SELECT id, slug, name, default_locale, time_zone FROM tenant"
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(tenant))
}
