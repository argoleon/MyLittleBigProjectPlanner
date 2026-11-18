//! Saved filters (specification §6, L06-US04): a query of the language kept
//! under a name and an icon, personal, or shared with groups or the whole
//! tenant. Every member saves their own; those a filter is shared with run
//! it, each seeing the tickets they may see; its owner alone edits it, and a
//! tenant admin may delete it once shared.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::saved_filters::{FilterFields, can_delete, can_edit, validate};
use domain::validation::FieldError;
use infra::accounts::TenantRole;
use infra::saved_filters::{self as repo, FilterInput, SavedFilter, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::search::{TAG, query_problem};
use crate::tenant::TenantTx;

const PATH: &str = "/api/v1/saved-filters";

/// Who saved a filter.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SavedFilterOwner)]
pub struct OwnerView {
    account_id: Uuid,
    /// Their name, or "Deleted user #N" once erased.
    #[schema(example = "Lucas Bernard")]
    display_name: String,
    /// Still a member of the tenant.
    member: bool,
}

/// A group a filter is shared with.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SavedFilterGroup)]
pub struct GroupView {
    id: Uuid,
    #[schema(example = "Platform")]
    name: String,
}

/// Who a filter is shared with: nobody but its owner when neither.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SavedFilterSharing)]
pub struct SharingView {
    /// Every member of the tenant.
    all: bool,
    /// These groups, by name.
    groups: Vec<GroupView>,
}

/// A saved filter, as the member reading it sees it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SavedFilter)]
pub struct SavedFilterView {
    id: Uuid,
    #[schema(example = "My open tickets")]
    name: String,
    /// A Material Symbols name, of `filter_alt`, `person`, `bug_report`,
    /// `schedule`, `shield`, `flag`, `star`, `support_agent`, `bolt`, `label`.
    #[schema(example = "person")]
    icon: String,
    /// The query, as saved: run it with `POST /search`.
    #[schema(example = "assignee = currentUser() AND status != done")]
    query: String,
    /// `null`: the account no longer exists.
    #[schema(required = true)]
    owner: Option<OwnerView>,
    sharing: SharingView,
    /// Saved by the member reading it.
    mine: bool,
    /// The member reading it may change it: its owner.
    can_edit: bool,
    /// The member reading it may delete it: its owner, or a tenant admin once
    /// it is shared.
    can_delete: bool,
    #[schema(example = 1)]
    version: i64,
}

impl SavedFilterView {
    fn of(filter: SavedFilter, tx: &TenantTx) -> Self {
        let mine = filter.owner_id == Some(tx.account_id());
        let admin = tx.member().tenant_role == TenantRole::TenantAdmin;
        let shared = filter.shared();
        Self {
            id: filter.id,
            name: filter.name,
            icon: filter.icon,
            query: filter.query,
            owner: filter.owner_id.map(|account_id| OwnerView {
                account_id,
                display_name: filter.owner_name.unwrap_or_default(),
                member: filter.owner_is_member,
            }),
            sharing: SharingView {
                all: filter.shared_with_all,
                groups: filter
                    .groups
                    .into_iter()
                    .map(|g| GroupView {
                        id: g.id,
                        name: g.name,
                    })
                    .collect(),
            },
            mine,
            can_edit: can_edit(mine),
            can_delete: can_delete(mine, admin, shared),
            version: filter.version,
        }
    }
}

/// The saved filters a member sees.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SavedFilterList {
    /// Theirs, those shared with the tenant and those shared with one of
    /// their groups, by name.
    items: Vec<SavedFilterView>,
}

/// Who a filter is shared with.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SavedFilterSharingFields)]
pub struct SharingBody {
    /// Every member of the tenant.
    #[serde(default)]
    all: bool,
    /// Groups of the tenant, up to 50.
    #[serde(default)]
    group_ids: Vec<Uuid>,
}

/// A filter to save.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SavedFilterFields, example = json!({
    "name": "My open tickets",
    "icon": "person",
    "query": "assignee = currentUser() AND status != done",
    "sharing": {"all": false, "groupIds": []}
}))]
pub struct SavedFilterBody {
    /// 1 to 100 characters, unique among the member's filters whatever the
    /// case.
    name: String,
    /// One of the icons of [`SavedFilterView::icon`]; `filter_alt` when left
    /// out.
    #[serde(default = "default_icon")]
    icon: String,
    /// A query of the language, checked as `POST /search/parse` does.
    query: String,
    /// Private when left out.
    #[serde(default)]
    sharing: SharingBody,
}

fn default_icon() -> String {
    domain::saved_filters::ICONS[0].to_owned()
}

/// `?search=&limit=` of the groups a filter can be shared with.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ShareGroupParams {
    /// Part of the group's name, any case.
    pub search: Option<String>,
    /// Most groups, from 1 to 50 (default 10).
    #[param(minimum = 1, maximum = 50)]
    pub limit: Option<usize>,
}

/// Checks `body`, its query included.
fn checked(body: &SavedFilterBody) -> Result<FilterInput<'_>, Problem> {
    validate(&FilterFields {
        name: body.name.clone(),
        icon: body.icon.clone(),
        groups: body.sharing.group_ids.len(),
    })
    .map_err(Problem::validation)?;
    domain::query::parse(&body.query).map_err(|e| query_problem(&e))?;
    Ok(FilterInput {
        name: &body.name,
        icon: &body.icon,
        query: &body.query,
        shared_with_all: body.sharing.all,
        groups: &body.sharing.group_ids,
    })
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::NameTaken => {
            Problem::new(ErrorCode::SavedFilterNameTaken).with_errors(vec![FieldError::new("name", "taken")])
        }
        WriteConflict::UnknownGroup => {
            Problem::validation(vec![FieldError::new("sharing.groupIds", "unknown_group")])
        }
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

/// The filter `id`, if the member sees it.
async fn visible(tx: &mut TenantTx, id: Uuid) -> Result<SavedFilter, Problem> {
    let reader = tx.account_id();
    repo::find(tx, reader, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound))
}

/// `GET /api/v1/saved-filters`.
#[utoipa::path(
    get,
    path = "/saved-filters",
    tag = TAG,
    operation_id = "listSavedFilters",
    summary = "List the saved filters the member sees",
    description = "Theirs, those shared with the whole tenant, and those shared with one of their \
                   groups, by name. A member keeps a few dozen at most: no pages.",
    responses(
        (status = 200, description = "The filters", body = SavedFilterList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(mut tx: TenantTx) -> Result<ApiJson<SavedFilterList>, Problem> {
    let reader = tx.account_id();
    let filters = repo::list(&mut tx, reader).await?;
    let items = filters.into_iter().map(|f| SavedFilterView::of(f, &tx)).collect();
    tx.commit().await?;
    Ok(ApiJson(SavedFilterList { items }))
}

/// `POST /api/v1/saved-filters`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/saved-filters",
    tag = TAG,
    operation_id = "createSavedFilter",
    summary = "Save a filter",
    description = "Any member, as its owner. The query is checked as `POST /search/parse` does; \
                   the fields and values it names are only looked up when it runs.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
    ),
    request_body = SavedFilterBody,
    responses(
        (status = 201, description = "Saved", body = SavedFilterView, headers(
            ("ETag" = String, description = "Version of the filter"),
            ("Location" = String, description = "URL of the filter"),
        )),
        (status = 400, description = "`validation_failed` (`sharing.groupIds`: `unknown_group` for a group not \
                                      of the tenant), or `query.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`saved_filter.name_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiJson(body): ApiJson<SavedFilterBody>,
) -> Result<Response, Problem> {
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = checked(&body)?;
    let owner = tx.account_id();
    let filter = repo::insert(&mut tx, infra::id::new_id(), owner, &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %owner, filter = %filter.id, shared = filter.shared(), "filter saved");
    let (id, version) = (filter.id, filter.version);
    let view = SavedFilterView::of(filter, &tx);
    let response =
        idempotency::created(&mut tx, key.as_deref(), &view, format!("{PATH}/{id}"), version).await?;
    tx.commit().await?;
    Ok(response)
}

/// `GET /api/v1/saved-filters/{id}`.
#[utoipa::path(
    get,
    path = "/saved-filters/{id}",
    tag = TAG,
    operation_id = "getSavedFilter",
    summary = "Read a saved filter",
    description = "One the member sees: theirs, or shared with them.",
    params(
        ("id" = Uuid, Path, description = "Id of the filter"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The filter", body = SavedFilterView, headers(
            ("ETag" = String, description = "Version of the filter"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such filter in this tenant, or not shared with the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let filter = visible(&mut tx, id).await?;
    let version = filter.version;
    let view = SavedFilterView::of(filter, &tx);
    tx.commit().await?;
    if if_none_match(&headers, version) {
        return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), version));
    }
    Ok(with_etag(ApiJson(view).into_response(), version))
}

/// `PUT /api/v1/saved-filters/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/saved-filters/{id}",
    tag = TAG,
    operation_id = "updateSavedFilter",
    summary = "Change a saved filter",
    description = "Its owner only: its name, icon, query and sharing, all replaced.",
    params(
        ("id" = Uuid, Path, description = "Id of the filter"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = SavedFilterBody,
    responses(
        (status = 200, description = "The filter", body = SavedFilterView, headers(
            ("ETag" = String, description = "New version of the filter"),
        )),
        (status = 400, description = "`validation_failed`, or `query.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: shared with the member, not theirs; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such filter in this tenant, or not shared with the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`saved_filter.name_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The filter changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SavedFilterBody>,
) -> Result<Response, Problem> {
    let filter = visible(&mut tx, id).await?;
    let owner = tx.account_id();
    if !can_edit(filter.owner_id == Some(owner)) {
        return Err(Problem::new(ErrorCode::Forbidden).with_detail("only its owner changes a filter"));
    }
    let if_match = require_if_match(&headers)?;
    let input = checked(&body)?;
    let filter = repo::update(&mut tx, id, owner, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %owner, filter = %id, shared = filter.shared(), "filter changed");
    let version = filter.version;
    let view = SavedFilterView::of(filter, &tx);
    tx.commit().await?;
    Ok(with_etag(ApiJson(view).into_response(), version))
}

/// `DELETE /api/v1/saved-filters/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/saved-filters/{id}",
    tag = TAG,
    operation_id = "deleteSavedFilter",
    summary = "Delete a saved filter",
    description = "Its owner; or a tenant admin, once it is shared.",
    params(
        ("id" = Uuid, Path, description = "Id of the filter"),
        ("If-Match" = String, Header, description = "ETag of the filter, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither its owner nor a tenant admin; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such filter in this tenant, or not shared with the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The filter changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    let filter = visible(&mut tx, id).await?;
    let actor = tx.account_id();
    let admin = tx.member().tenant_role == TenantRole::TenantAdmin;
    if !can_delete(filter.owner_id == Some(actor), admin, filter.shared()) {
        return Err(Problem::new(ErrorCode::Forbidden)
            .with_detail("only its owner or a tenant admin deletes a shared filter"));
    }
    let if_match = require_if_match(&headers)?;
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, filter = %id, owner = ?filter.owner_id, "filter deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/saved-filters/share-groups`.
#[utoipa::path(
    get,
    path = "/saved-filters/share-groups",
    tag = TAG,
    operation_id = "listSavedFilterShareGroups",
    summary = "Search the groups a filter can be shared with",
    description = "The groups of the tenant whose name holds `search`, by name. Any member.",
    params(ShareGroupParams),
    responses(
        (status = 200, description = "The groups", body = Vec<GroupView>),
        (status = 400, description = "Invalid `limit`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn share_groups(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<ShareGroupParams>,
) -> Result<ApiJson<Vec<GroupView>>, Problem> {
    let limit = params.limit.unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("limit: 1 to 50"));
    }
    let search = params.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let groups = repo::share_groups(&mut tx, search, limit).await?;
    tx.commit().await?;
    Ok(ApiJson(
        groups
            .into_iter()
            .map(|g| GroupView {
                id: g.id,
                name: g.name,
            })
            .collect(),
    ))
}
