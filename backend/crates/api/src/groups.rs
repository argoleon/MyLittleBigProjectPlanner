//! Groups of a tenant's members (specification §14, L02-US06): roles are
//! granted to groups as well as to members (L02-US07). Tenant admins only.
//! Changes of a group's members are logged with the `audit` target.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::members::{GroupInput, validate_group};
use domain::validation::FieldError;
use infra::groups::{self as repo, Group, GroupMember, MemberChange, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::members::{TAG, require_member_admin};
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::TenantTx;

const PATH: &str = "/api/v1/tenant/groups";

/// A group of members.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Group)]
pub struct GroupView {
    id: Uuid,
    #[schema(example = "Platform")]
    name: String,
    #[schema(example = "Platform team")]
    description: Option<String>,
    #[schema(example = 4)]
    member_count: i64,
    #[schema(example = 1)]
    version: i64,
}

impl From<Group> for GroupView {
    fn from(group: Group) -> Self {
        Self {
            id: group.id,
            name: group.name,
            description: group.description,
            member_count: group.member_count,
            version: group.version,
        }
    }
}

/// A member of a group.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = GroupMember)]
pub struct GroupMemberView {
    account_id: Uuid,
    #[schema(example = "lucas.bernard@acme.example")]
    email: String,
    #[schema(example = "Lucas Bernard")]
    display_name: String,
}

impl From<GroupMember> for GroupMemberView {
    fn from(member: GroupMember) -> Self {
        Self {
            account_id: member.account_id,
            email: member.email,
            display_name: member.display_name,
        }
    }
}

/// A group with its members.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = GroupDetail)]
pub struct GroupDetailView {
    #[serde(flatten)]
    group: GroupView,
    /// By name.
    members: Vec<GroupMemberView>,
}

/// A page of groups.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupPage {
    items: Vec<GroupView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?limit=&cursor=` of the groups list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GroupListParams {
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// A group's name and description.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = GroupFields, example = json!({"name": "Platform", "description": "Platform team"}))]
pub struct GroupBody {
    /// 1 to 100 characters, unique in the tenant whatever the case.
    name: String,
    /// Up to 1000 characters.
    description: Option<String>,
}

impl From<GroupBody> for GroupInput {
    fn from(body: GroupBody) -> Self {
        Self {
            name: body.name,
            description: body.description,
        }
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::NameTaken => {
            Problem::new(ErrorCode::GroupNameTaken).with_errors(vec![FieldError::new("name", "taken")])
        }
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

/// `GET /api/v1/tenant/groups`, in creation order.
#[utoipa::path(
    get,
    path = "/tenant/groups",
    tag = TAG,
    operation_id = "listGroups",
    summary = "List the groups of the tenant",
    description = "Tenant admins only. In creation order; follow `nextCursor` until it is `null`.",
    params(GroupListParams),
    responses(
        (status = 200, description = "A page of groups", body = GroupPage),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<GroupListParams>,
) -> Result<ApiJson<GroupPage>, Problem> {
    require_member_admin(&tx)?;
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let page = repo::list(&mut tx, after, limit).await?;
    tx.commit().await?;
    Ok(ApiJson(GroupPage {
        items: page.items.into_iter().map(GroupView::from).collect(),
        next_cursor: page.next.map(encode_cursor),
    }))
}

/// `POST /api/v1/tenant/groups`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/tenant/groups",
    tag = TAG,
    operation_id = "createGroup",
    summary = "Create a group",
    description = "Tenant admins only, without members: add them one by one.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
    ),
    request_body = GroupBody,
    responses(
        (status = 201, description = "Created", body = GroupView, headers(
            ("ETag" = String, description = "Version of the group"),
            ("Location" = String, description = "URL of the group"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`group.name_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiJson(body): ApiJson<GroupBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = GroupInput::from(body);
    validate_group(&input).map_err(Problem::validation)?;
    let group = repo::insert(&mut tx, infra::id::new_id(), &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), group = %group.id, "group created");
    let (id, version) = (group.id, group.version);
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &GroupView::from(group),
        format!("{PATH}/{id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `GET /api/v1/tenant/groups/{id}`, with its members.
#[utoipa::path(
    get,
    path = "/tenant/groups/{id}",
    tag = TAG,
    operation_id = "getGroup",
    summary = "Read a group and its members",
    description = "Tenant admins only.",
    params(
        ("id" = Uuid, Path, description = "Id of the group"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The group", body = GroupDetailView, headers(
            ("ETag" = String, description = "Version of the group, members included"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such group in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let group = repo::find(&mut tx, id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    let version = group.version;
    if if_none_match(&headers, version) {
        tx.commit().await?;
        return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), version));
    }
    let members = repo::members(&mut tx, id).await?;
    tx.commit().await?;
    let detail = GroupDetailView {
        group: group.into(),
        members: members.into_iter().map(GroupMemberView::from).collect(),
    };
    Ok(with_etag(ApiJson(detail).into_response(), version))
}

/// `PUT /api/v1/tenant/groups/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/tenant/groups/{id}",
    tag = TAG,
    operation_id = "updateGroup",
    summary = "Rename a group or change its description",
    description = "Tenant admins only.",
    params(
        ("id" = Uuid, Path, description = "Id of the group"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = GroupBody,
    responses(
        (status = 200, description = "The group", body = GroupView, headers(
            ("ETag" = String, description = "New version of the group"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such group in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`group.name_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The group changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<GroupBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = GroupInput::from(body);
    validate_group(&input).map_err(Problem::validation)?;
    let group = repo::update(&mut tx, id, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), group = %id, "group changed");
    tx.commit().await?;
    let version = group.version;
    Ok(with_etag(
        ApiJson(GroupView::from(group)).into_response(),
        version,
    ))
}

/// `DELETE /api/v1/tenant/groups/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/tenant/groups/{id}",
    tag = TAG,
    operation_id = "deleteGroup",
    summary = "Delete a group",
    description = "Tenant admins only. Its members stay in the tenant.",
    params(
        ("id" = Uuid, Path, description = "Id of the group"),
        ("If-Match" = String, Header, description = "ETag of the group, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such group in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The group changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), group = %id, "group deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/tenant/groups/{id}/members/{accountId}`.
#[utoipa::path(
    put,
    path = "/tenant/groups/{id}/members/{accountId}",
    tag = TAG,
    operation_id = "addGroupMember",
    summary = "Add a member to a group",
    description = "Tenant admins only. Adding a member already in the group changes nothing.",
    params(
        ("id" = Uuid, Path, description = "Id of the group"),
        ("accountId" = Uuid, Path, description = "Account of a member of the tenant"),
    ),
    responses(
        (status = 204, description = "In the group"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such group, or no such member, in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn add_member(
    mut tx: TenantTx,
    ApiPath((id, account_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    match repo::add_member(&mut tx, id, account_id).await? {
        MemberChange::Done => {
            tracing::info!(target: "audit", actor = %tx.account_id(), group = %id, member = %account_id, "group member added");
        }
        MemberChange::Unchanged => {}
        MemberChange::NoGroup => return Err(Problem::new(ErrorCode::NotFound).with_detail("no such group")),
        MemberChange::NoMember => return Err(Problem::new(ErrorCode::NotFound).with_detail("no such member")),
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/tenant/groups/{id}/members/{accountId}`.
#[utoipa::path(
    delete,
    path = "/tenant/groups/{id}/members/{accountId}",
    tag = TAG,
    operation_id = "removeGroupMember",
    summary = "Remove a member from a group",
    description = "Tenant admins only. The member stays in the tenant.",
    params(
        ("id" = Uuid, Path, description = "Id of the group"),
        ("accountId" = Uuid, Path, description = "Account of the member"),
    ),
    responses(
        (status = 204, description = "Out of the group"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such group in this tenant, or the member is not in it", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn remove_member(
    mut tx: TenantTx,
    ApiPath((id, account_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    match repo::remove_member(&mut tx, id, account_id).await? {
        MemberChange::Done => {
            tracing::info!(target: "audit", actor = %tx.account_id(), group = %id, member = %account_id, "group member removed");
        }
        MemberChange::Unchanged | MemberChange::NoMember => {
            return Err(Problem::new(ErrorCode::NotFound).with_detail("not in this group"));
        }
        MemberChange::NoGroup => return Err(Problem::new(ErrorCode::NotFound).with_detail("no such group")),
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
