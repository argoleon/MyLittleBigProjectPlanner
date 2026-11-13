//! Node roles of the tenant, as sets of permissions (specification §14,
//! L02-US07). Tenant admins define them; they are granted to members and
//! groups on hierarchy nodes (`crate::grants`). Every tenant starts with four default roles
//! (`domain::permissions::DEFAULT_ROLES`).
//!
//! Changing or deleting a role invalidates the permission cache of the
//! tenant once committed: it applies to the next request. A role used by
//! grants cannot be deleted.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::permissions::{PermissionChange, RoleInput, validate_role};
use domain::validation::FieldError;
use infra::roles::{self as repo, Role, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::members::{TAG, require_member_admin};
use crate::pagination::{PageParams, encode_cursor};
use crate::permissions::{self, PermissionView};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

const PATH: &str = "/api/v1/tenant/roles";

/// A node role.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Role)]
pub struct RoleView {
    id: Uuid,
    /// In the language of the request (`Accept-Language`), else the
    /// tenant's default, else any.
    #[schema(example = "Project manager")]
    label: Option<String>,
    #[schema(example = json!({"fr": "Chef de projet", "en": "Project manager"}))]
    label_translations: BTreeMap<String, String>,
    /// In catalog order.
    permissions: Vec<PermissionView>,
    /// Grants of this role on nodes; a role in use cannot be deleted.
    #[schema(example = 0)]
    grant_count: u64,
    #[schema(example = 1)]
    version: i64,
}

/// A page of roles.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolePage {
    items: Vec<RoleView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?limit=&cursor=` of the roles list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RoleListParams {
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// A role's labels and permissions.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = RoleFields,
    example = json!({
        "labelTranslations": {"fr": "Chef de projet", "en": "Project manager"},
        "permissions": ["view", "create_ticket", "edit", "comment", "log_time", "assign_resources", "force_overload", "manage_sprints"]
    })
)]
pub struct RoleBody {
    /// One label per language (`fr`, `en`), at least one; 100 characters at
    /// most, unique in the tenant whatever the case.
    label_translations: BTreeMap<String, String>,
    /// At least one.
    permissions: Vec<PermissionView>,
}

impl From<RoleBody> for RoleInput {
    fn from(body: RoleBody) -> Self {
        Self {
            permissions: permissions::set_of(&body.permissions),
            label_translations: body.label_translations,
        }
    }
}

/// Language of the labels for this request and tenant.
pub(crate) struct Language {
    locale: &'static str,
    fallback: String,
}

impl Language {
    pub(crate) fn of(tx: &TenantTx, accept: &AcceptLanguage) -> Self {
        let fallback = tx.default_locale().to_owned();
        let locale = domain::i18n::negotiate(accept.0.as_deref(), &fallback);
        Self { locale, fallback }
    }

    /// The label of `translations` in this language, else the tenant's
    /// default, else any.
    pub(crate) fn label(&self, translations: &BTreeMap<String, String>) -> Option<String> {
        domain::i18n::resolve(translations, self.locale, &self.fallback).map(str::to_owned)
    }

    fn view(&self, role: Role, grant_count: u64) -> RoleView {
        let label = self.label(&role.label_translations);
        RoleView {
            id: role.id,
            label,
            label_translations: role.label_translations,
            permissions: permissions::views(role.permissions),
            grant_count,
            version: role.version,
        }
    }

    pub(crate) fn headers(&self, response: &mut Response) {
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(self.locale));
        headers.insert(header::VARY, HeaderValue::from_static("accept-language"));
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::LabelTaken => Problem::new(ErrorCode::RoleLabelTaken)
            .with_errors(vec![FieldError::new("labelTranslations", "taken")]),
    }
}

async fn grant_count(state: &AppState, tx: &mut TenantTx, role_id: Uuid) -> Result<u64, Problem> {
    Ok(state.grants.grants_of_role(tx, role_id).await?)
}

/// `GET /api/v1/tenant/roles`, in creation order.
#[utoipa::path(
    get,
    path = "/tenant/roles",
    tag = TAG,
    operation_id = "listRoles",
    summary = "List the node roles of the tenant",
    description = "Tenant admins only. In creation order; follow `nextCursor` until it is `null`.",
    params(
        RoleListParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "A page of roles", body = RolePage, headers(
            ("Content-Language" = String, description = "Language of `label`"),
        )),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<RoleListParams>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let language = Language::of(&tx, &accept);
    let page = repo::list(&mut tx, after, limit).await?;
    let mut items = Vec::with_capacity(page.items.len());
    for role in page.items {
        let count = grant_count(&state, &mut tx, role.id).await?;
        items.push(language.view(role, count));
    }
    tx.commit().await?;
    let mut response = ApiJson(RolePage {
        items,
        next_cursor: page.next.map(encode_cursor),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/tenant/roles`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/tenant/roles",
    tag = TAG,
    operation_id = "createRole",
    summary = "Create a node role",
    description = "Tenant admins only.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = RoleBody,
    responses(
        (status = 201, description = "Created", body = RoleView, headers(
            ("ETag" = String, description = "Version of the role"),
            ("Location" = String, description = "URL of the role"),
        )),
        (status = 400, description = "`validation_failed`, or an unknown permission (`invalid_json`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`role.label_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiJson(body): ApiJson<RoleBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = RoleInput::from(body);
    validate_role(&input).map_err(Problem::validation)?;
    let role = repo::insert(&mut tx, infra::id::new_id(), &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), role = %role.id, "role created");
    let language = Language::of(&tx, &accept);
    let (id, version) = (role.id, role.version);
    let mut response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &language.view(role, 0),
        format!("{PATH}/{id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/tenant/roles/{id}`.
#[utoipa::path(
    get,
    path = "/tenant/roles/{id}",
    tag = TAG,
    operation_id = "getRole",
    summary = "Read a node role",
    description = "Tenant admins only.",
    params(
        ("id" = Uuid, Path, description = "Id of the role"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "The role", body = RoleView, headers(
            ("ETag" = String, description = "Version of the role"),
            ("Content-Language" = String, description = "Language of `label`"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such role in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let language = Language::of(&tx, &accept);
    let role = repo::find(&mut tx, id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    let count = grant_count(&state, &mut tx, id).await?;
    tx.commit().await?;
    let version = role.version;
    let mut response = if if_none_match(&headers, version) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ApiJson(language.view(role, count)).into_response()
    };
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/tenant/roles/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/tenant/roles/{id}",
    tag = TAG,
    operation_id = "updateRole",
    summary = "Change a node role's labels and permissions",
    description = "Tenant admins only. Applies to the next request of every member the role \
                   reaches: the permission cache of the tenant is invalidated.",
    params(
        ("id" = Uuid, Path, description = "Id of the role"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = RoleBody,
    responses(
        (status = 200, description = "The role", body = RoleView, headers(
            ("ETag" = String, description = "New version of the role"),
        )),
        (status = 400, description = "`validation_failed`, or an unknown permission (`invalid_json`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such role in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`role.label_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The role changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<RoleBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = RoleInput::from(body);
    validate_role(&input).map_err(Problem::validation)?;
    let language = Language::of(&tx, &accept);
    let role = repo::update(&mut tx, id, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    let count = grant_count(&state, &mut tx, id).await?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        role = %id,
        permissions = ?input.permissions.iter().map(domain::permissions::Permission::as_str).collect::<Vec<_>>(),
        "role changed"
    );
    let tenant = tx.tenant_id();
    tx.commit().await?;
    permissions::changed(&state, tenant, PermissionChange::RoleChanged).await;
    let version = role.version;
    let mut response = ApiJson(language.view(role, count)).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    Ok(response)
}

/// `DELETE /api/v1/tenant/roles/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/tenant/roles/{id}",
    tag = TAG,
    operation_id = "deleteRole",
    summary = "Delete a node role",
    description = "Tenant admins only. Refused while grants use the role (`role.in_use`, their \
                   number in `detail`).",
    params(
        ("id" = Uuid, Path, description = "Id of the role"),
        ("If-Match" = String, Header, description = "ETag of the role, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such role in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`role.in_use`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The role changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    if repo::find(&mut tx, id).await?.is_none() {
        return Err(Problem::new(ErrorCode::NotFound));
    }
    let used = grant_count(&state, &mut tx, id).await?;
    if used > 0 {
        return Err(Problem::new(ErrorCode::RoleInUse).with_detail(format!("used by {used} grants")));
    }
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), role = %id, "role deleted");
    let tenant = tx.tenant_id();
    tx.commit().await?;
    permissions::changed(&state, tenant, PermissionChange::RoleDeleted).await;
    Ok(StatusCode::NO_CONTENT)
}
