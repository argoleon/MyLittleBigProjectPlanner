//! `/api/v1/platform/tenants`: tenants, managed by the platform super admin
//! (specification §3).
//!
//! The super admin signs in like any member, on a tenant they belong to; the
//! [`PlatformAdmin`] extractor then requires their account to be a platform
//! super admin (403 `forbidden` otherwise). The database functions behind
//! these endpoints check it again (`infra::platform`).
//!
//! Same conventions as the reference resource (`reference_items.rs`), except
//! `?fields=` and `Accept-Language`: tenants have no translated label and
//! few fields.

use std::ops::{Deref, DerefMut};

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::tenancy::Tenancy;
use domain::tenant::{TenantSettingsInput, validate_new_tenant, validate_settings};
use domain::validation::FieldError;
use infra::idempotency::{Claim, StoredResponse};
use infra::platform::{self as repo, PlatformConflict, Tenant, TenantSettings, TenantStatus};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency;
use crate::pagination::{PageBody, PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// OpenAPI tag of the platform endpoints.
pub const TAG: &str = "platform";

const PATH: &str = "/api/v1/platform/tenants";

/// A transaction on behalf of a signed-in platform super admin.
#[derive(Debug)]
pub struct PlatformAdmin(TenantTx);

impl PlatformAdmin {
    /// The super admin's account.
    pub fn actor(&self) -> Uuid {
        self.0.account_id()
    }

    pub async fn commit(self) -> Result<(), Problem> {
        self.0.commit().await
    }
}

impl Deref for PlatformAdmin {
    type Target = TenantTx;
    fn deref(&self) -> &TenantTx {
        &self.0
    }
}

impl DerefMut for PlatformAdmin {
    fn deref_mut(&mut self) -> &mut TenantTx {
        &mut self.0
    }
}

impl FromRequestParts<AppState> for PlatformAdmin {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        let mut tx = TenantTx::from_request_parts(parts, state).await?;
        let account = tx.account_id();
        if repo::is_platform_admin(&mut tx, account).await? {
            Ok(Self(tx))
        } else {
            Err(Problem::new(ErrorCode::Forbidden))
        }
    }
}

// --- Bodies ---------------------------------------------------------------------

/// What happens when an assignment exceeds capacity plus tolerance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = OverloadMode)]
pub enum OverloadModeView {
    /// The assignment is refused.
    Blocking,
    /// The assignment is accepted and the overload flagged.
    Warning,
}

impl From<domain::tenant::OverloadMode> for OverloadModeView {
    fn from(mode: domain::tenant::OverloadMode) -> Self {
        match mode {
            domain::tenant::OverloadMode::Blocking => Self::Blocking,
            domain::tenant::OverloadMode::Warning => Self::Warning,
        }
    }
}

/// Settings of a tenant (`PUT`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = TenantSettings,
    example = json!({
        "name": "Acme Industrie",
        "defaultLocale": "fr",
        "timeZone": "Europe/Paris",
        "defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0],
        "overloadTolerancePercent": 0,
        "overloadMode": "blocking",
        "storageQuotaBytes": null,
        "apiRateLimitPerMinute": null
    })
)]
pub struct SettingsBody {
    /// 1 to 200 characters.
    #[schema(example = "Acme Industrie")]
    name: String,
    /// `fr` or `en`: fallback of translated labels, language of new members.
    #[schema(example = "fr")]
    default_locale: String,
    /// IANA time zone.
    #[schema(example = "Europe/Paris")]
    time_zone: String,
    /// Default weekly capacity of new resources: 7 values, minutes per
    /// weekday from Monday, 0 to 1440 each. Default: 7 h × 5 days.
    #[serde(default = "default_capacity")]
    #[schema(min_items = 7, max_items = 7, example = json!([420, 420, 420, 420, 420, 0, 0]))]
    default_capacity_minutes: Vec<i64>,
    /// Load accepted above capacity, in percent, 0 to 100. Default 0.
    #[serde(default)]
    #[schema(minimum = 0, maximum = 100, default = 0)]
    overload_tolerance_percent: i64,
    /// Default `blocking`.
    #[serde(default = "default_overload_mode")]
    #[schema(value_type = OverloadModeView)]
    overload_mode: String,
    /// Storage quota in bytes; `null` or absent: unlimited.
    #[schema(minimum = 1)]
    storage_quota_bytes: Option<i64>,
    /// API requests per minute; `null` or absent: unlimited.
    #[schema(minimum = 1, maximum = 1_000_000)]
    api_rate_limit_per_minute: Option<i64>,
}

fn default_capacity() -> Vec<i64> {
    domain::tenant::DEFAULT_CAPACITY_MINUTES
        .iter()
        .map(|m| i64::from(*m))
        .collect()
}

fn default_overload_mode() -> String {
    domain::tenant::OverloadMode::Blocking.as_str().to_owned()
}

impl From<SettingsBody> for TenantSettingsInput {
    fn from(body: SettingsBody) -> Self {
        Self {
            name: body.name,
            default_locale: body.default_locale,
            time_zone: body.time_zone,
            default_capacity_minutes: body.default_capacity_minutes,
            overload_tolerance_percent: body.overload_tolerance_percent,
            overload_mode: body.overload_mode,
            storage_quota_bytes: body.storage_quota_bytes,
            api_rate_limit_per_minute: body.api_rate_limit_per_minute,
        }
    }
}

/// Body of `POST`: the tenant's host name label and its settings.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewTenant,
    example = json!({"slug": "acme", "name": "Acme Industrie", "defaultLocale": "fr", "timeZone": "Europe/Paris"})
)]
pub struct NewTenantBody {
    /// Host name label: `acme` serves `acme.<domain>`. 1 to 63 characters
    /// among `a-z`, `0-9` and `-`; cannot change later. In single-tenant
    /// mode, only the configured default slug is accepted.
    #[schema(example = "acme", pattern = "^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")]
    slug: String,
    /// 1 to 200 characters.
    #[schema(example = "Acme Industrie")]
    name: String,
    /// `fr` or `en`.
    #[schema(example = "fr")]
    default_locale: String,
    /// IANA time zone.
    #[schema(example = "Europe/Paris")]
    time_zone: String,
    /// 7 values, minutes per weekday from Monday, 0 to 1440 each. Default: 7 h × 5 days.
    #[serde(default = "default_capacity")]
    #[schema(min_items = 7, max_items = 7, example = json!([420, 420, 420, 420, 420, 0, 0]))]
    default_capacity_minutes: Vec<i64>,
    /// 0 to 100. Default 0.
    #[serde(default)]
    #[schema(minimum = 0, maximum = 100, default = 0)]
    overload_tolerance_percent: i64,
    /// Default `blocking`.
    #[serde(default = "default_overload_mode")]
    #[schema(value_type = OverloadModeView)]
    overload_mode: String,
    /// `null` or absent: unlimited.
    #[schema(minimum = 1)]
    storage_quota_bytes: Option<i64>,
    /// `null` or absent: unlimited.
    #[schema(minimum = 1, maximum = 1_000_000)]
    api_rate_limit_per_minute: Option<i64>,
}

impl NewTenantBody {
    fn into_parts(self) -> (String, TenantSettingsInput) {
        let settings = TenantSettingsInput {
            name: self.name,
            default_locale: self.default_locale,
            time_zone: self.time_zone,
            default_capacity_minutes: self.default_capacity_minutes,
            overload_tolerance_percent: self.overload_tolerance_percent,
            overload_mode: self.overload_mode,
            storage_quota_bytes: self.storage_quota_bytes,
            api_rate_limit_per_minute: self.api_rate_limit_per_minute,
        };
        (self.slug, settings)
    }
}

/// Lifecycle of a tenant.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = TenantStatus)]
pub enum TenantStatusView {
    Active,
    /// No longer reachable; its data is being purged.
    Deleting,
}

/// A tenant as the platform super admin sees it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PlatformTenant)]
pub struct TenantView {
    id: Uuid,
    #[schema(example = "acme")]
    slug: String,
    /// Address of the tenant's application.
    #[schema(example = "https://acme.planner.example.org")]
    url: String,
    #[schema(example = "Acme Industrie")]
    name: String,
    #[schema(example = "fr")]
    default_locale: String,
    #[schema(example = "Europe/Paris")]
    time_zone: String,
    /// Minutes per weekday, Monday first.
    #[schema(example = json!([420, 420, 420, 420, 420, 0, 0]))]
    default_capacity_minutes: Vec<i32>,
    overload_tolerance_percent: i16,
    overload_mode: OverloadModeView,
    /// `null`: unlimited.
    #[schema(required = true)]
    storage_quota_bytes: Option<i64>,
    /// `null`: unlimited.
    #[schema(required = true)]
    api_rate_limit_per_minute: Option<i32>,
    status: TenantStatusView,
    /// Incremented on every change; the `ETag` is this value in quotes.
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TenantView {
    fn new(tenant: Tenant, state: &AppState) -> Self {
        Self {
            id: tenant.id,
            url: state.auth.public_url().for_tenant(&tenant.slug),
            slug: tenant.slug,
            name: tenant.name,
            default_locale: tenant.default_locale,
            time_zone: tenant.time_zone,
            default_capacity_minutes: tenant.default_capacity_minutes,
            overload_tolerance_percent: tenant.overload_tolerance_percent,
            overload_mode: tenant.overload_mode.into(),
            storage_quota_bytes: tenant.storage_quota_bytes,
            api_rate_limit_per_minute: tenant.api_rate_limit_per_minute,
            status: match tenant.status {
                TenantStatus::Active => TenantStatusView::Active,
                TenantStatus::Deleting => TenantStatusView::Deleting,
            },
            version: tenant.version,
            created_at: tenant.created_at,
            updated_at: tenant.updated_at,
        }
    }
}

// Documentation only (`///` comments here would end up in the contract).
/// A page of tenants, in creation order.
#[derive(Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PlatformTenantPage)]
#[allow(dead_code)]
pub struct TenantPage {
    items: Vec<TenantView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?limit=&cursor=` of the list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// `?confirm=` of the deletion.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeleteParams {
    /// The tenant's slug, typed by the super admin to confirm.
    #[param(example = "acme")]
    pub confirm: Option<String>,
}

// --- Helpers --------------------------------------------------------------------

fn conflict_problem(conflict: PlatformConflict) -> Problem {
    match conflict {
        PlatformConflict::Forbidden => Problem::new(ErrorCode::Forbidden),
        PlatformConflict::NotFound => Problem::new(ErrorCode::NotFound),
        PlatformConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        PlatformConflict::SlugTaken => {
            Problem::new(ErrorCode::TenantSlugTaken).with_errors(vec![FieldError::new("slug", "taken")])
        }
    }
}

/// Checks `input` and its time zone against the database, then converts it.
async fn checked_settings(
    tx: &mut PlatformAdmin,
    input: &TenantSettingsInput,
    rules: Result<(), Vec<FieldError>>,
) -> Result<TenantSettings, Problem> {
    let mut errors = rules.err().unwrap_or_default();
    let shape_ok = !errors.iter().any(|e| e.field == "timeZone");
    if shape_ok && !repo::time_zone_exists(tx, &input.time_zone).await? {
        errors.push(FieldError::new("timeZone", "unknown_time_zone"));
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    TenantSettings::from_valid(input).ok_or_else(|| Problem::new(ErrorCode::ValidationFailed))
}

fn single_slug(state: &AppState) -> Option<&str> {
    match state.tenancy.as_ref() {
        Tenancy::Single { default_slug } => Some(default_slug),
        Tenancy::Multi { .. } => None,
    }
}

fn location(id: Uuid) -> String {
    format!("{PATH}/{id}")
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response.headers_mut().insert(header::ETAG, etag(version));
    response
}

fn to_json(view: &TenantView) -> Result<serde_json::Value, Problem> {
    serde_json::to_value(view).map_err(|e| Problem::internal(&e))
}

// --- Endpoints ------------------------------------------------------------------

/// `GET /api/v1/platform/tenants`, in creation order.
#[utoipa::path(
    get,
    path = "/platform/tenants",
    tag = TAG,
    operation_id = "listTenants",
    summary = "List every tenant",
    description = "Platform super admin only. In creation order; follow `nextCursor` until it is `null`.",
    params(ListParams),
    responses(
        (status = 200, description = "A page of tenants", body = TenantPage),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a platform super admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: PlatformAdmin,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<Response, Problem> {
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let actor = tx.actor();
    let page = repo::list(&mut tx, actor, after, limit)
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    let body = PageBody {
        items: page
            .items
            .into_iter()
            .map(|t| TenantView::new(t, &state))
            .collect(),
        next_cursor: page.next.map(encode_cursor),
    };
    Ok(ApiJson(body).into_response())
}

/// `POST /api/v1/platform/tenants`: create; idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/platform/tenants",
    tag = TAG,
    operation_id = "createTenant",
    summary = "Create a tenant",
    description = "Platform super admin only. The tenant is reachable at once, at its `url`. \
                   Replaying the same request with the same `Idempotency-Key` within 24 hours \
                   returns the first response (`Idempotent-Replayed: true`).",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
    ),
    request_body = NewTenantBody,
    responses(
        (status = 201, description = "Created", body = TenantView, headers(
            ("ETag" = String, description = "Version of the tenant"),
            ("Location" = String, description = "URL of the tenant"),
        )),
        (status = 400, description = "Invalid fields, JSON or `Idempotency-Key`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`tenant.slug_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 415, description = "Body is not JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: PlatformAdmin,
    headers: HeaderMap,
    ApiJson(body): ApiJson<NewTenantBody>,
) -> Result<Response, Problem> {
    let key = idempotency::key(&headers)?;
    if let Some(key) = &key {
        let hash = idempotency::request_hash(&format!("POST {PATH}"), &body)?;
        match infra::idempotency::claim(&mut tx, key, &hash).await? {
            Claim::New => {}
            Claim::Replay(stored) => return Ok(idempotency::replay(stored)),
            Claim::Mismatch => return Err(Problem::new(ErrorCode::IdempotencyKeyReused)),
            Claim::InFlight => return Err(Problem::new(ErrorCode::IdempotencyKeyInFlight)),
        }
    }

    let (slug, input) = body.into_parts();
    let rules = validate_new_tenant(&slug, &input, single_slug(&state));
    let settings = checked_settings(&mut tx, &input, rules).await?;

    let actor = tx.actor();
    let tenant = repo::create(&mut tx, actor, infra::id::new_id(), &slug, &settings)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(tenant = %tenant.id, slug = %tenant.slug, "tenant created");

    let (id, version) = (tenant.id, tenant.version);
    let json = to_json(&TenantView::new(tenant, &state))?;
    if let Some(key) = &key {
        let stored = StoredResponse {
            status: StatusCode::CREATED.as_u16(),
            body: json.clone(),
            location: Some(location(id)),
            etag: Some(format!("\"{version}\"")),
        };
        infra::idempotency::complete(&mut tx, key, &stored).await?;
    }
    tx.commit().await?;

    let mut response = with_etag((StatusCode::CREATED, axum::Json(json)).into_response(), version);
    if let Ok(value) = HeaderValue::from_str(&location(id)) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    Ok(response)
}

/// `GET /api/v1/platform/tenants/{id}`.
#[utoipa::path(
    get,
    path = "/platform/tenants/{id}",
    tag = TAG,
    operation_id = "getTenant",
    summary = "Read a tenant",
    description = "Platform super admin only.",
    params(
        ("id" = Uuid, Path, description = "Id of the tenant"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The tenant", body = TenantView, headers(
            ("ETag" = String, description = "Version of the tenant"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a platform super admin", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    mut tx: PlatformAdmin,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let actor = tx.actor();
    let tenant = repo::get(&mut tx, actor, id).await?.map_err(conflict_problem)?;
    tx.commit().await?;
    let version = tenant.version;
    if if_none_match(&headers, version) {
        return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), version));
    }
    Ok(with_etag(
        ApiJson(TenantView::new(tenant, &state)).into_response(),
        version,
    ))
}

/// `PUT /api/v1/platform/tenants/{id}`: replace the settings; requires `If-Match`.
#[utoipa::path(
    put,
    path = "/platform/tenants/{id}",
    tag = TAG,
    operation_id = "updateTenant",
    summary = "Change the settings and quotas of a tenant",
    description = "Platform super admin only. The slug cannot change.",
    params(
        ("id" = Uuid, Path, description = "Id of the tenant"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = SettingsBody,
    responses(
        (status = 200, description = "The updated tenant", body = TenantView, headers(
            ("ETag" = String, description = "New version of the tenant"),
        )),
        (status = 400, description = "Invalid fields or JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such tenant, or it is being deleted", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The tenant changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: PlatformAdmin,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SettingsBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let input = TenantSettingsInput::from(body);
    let rules = validate_settings(&input);
    let settings = checked_settings(&mut tx, &input, rules).await?;
    let actor = tx.actor();
    let tenant = repo::update(&mut tx, actor, id, if_match.versions(), &settings)
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    tracing::info!(tenant = %tenant.id, "tenant settings changed");
    let version = tenant.version;
    Ok(with_etag(
        ApiJson(TenantView::new(tenant, &state)).into_response(),
        version,
    ))
}

/// `DELETE /api/v1/platform/tenants/{id}?confirm=<slug>`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/platform/tenants/{id}",
    tag = TAG,
    operation_id = "deleteTenant",
    summary = "Delete a tenant and all its data",
    description = "Platform super admin only, confirmed by typing the tenant's slug in `confirm`. \
                   The tenant stops resolving at once; a background job then removes all its data, \
                   and the accounts that belonged to it only. The tenant of the request itself cannot \
                   be deleted.",
    params(
        ("id" = Uuid, Path, description = "Id of the tenant"),
        DeleteParams,
        ("If-Match" = String, Header, description = "ETag of the version being deleted, or `*`"),
    ),
    responses(
        (status = 202, description = "Deletion started: the tenant, now `deleting`", body = TenantView),
        (status = 400, description = "`tenant.confirmation_mismatch`: `confirm` is not the slug", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such tenant, or already being deleted", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`tenant.delete_current`: the tenant of the request", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The tenant changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: PlatformAdmin,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<DeleteParams>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let actor = tx.actor();
    let tenant = repo::get(&mut tx, actor, id).await?.map_err(conflict_problem)?;
    if params.confirm.as_deref() != Some(tenant.slug.as_str()) {
        return Err(Problem::new(ErrorCode::TenantConfirmationMismatch));
    }
    if tenant.id == tx.tenant_id() {
        return Err(Problem::new(ErrorCode::TenantDeleteCurrent));
    }
    let deleting = repo::delete(&mut tx, actor, id, if_match.versions(), infra::id::new_id())
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    tracing::info!(tenant = %deleting.id, slug = %deleting.slug, "tenant deletion started");
    Ok((StatusCode::ACCEPTED, ApiJson(TenantView::new(deleting, &state))).into_response())
}
