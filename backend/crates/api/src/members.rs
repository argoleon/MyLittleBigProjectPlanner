//! Administration of a tenant's members (specification §3, §14, L02-US06):
//! list and search them, change their account type and tenant role, remove
//! them. Tenant admins only: configuration admins manage configurations,
//! not members.
//!
//! Access changes are logged with the `audit` target (`audit_log` comes
//! with L05). A role change applies to the member's next request: the
//! request layer reads the membership every time. Removing a member, or
//! making them external, also ends their sessions in this tenant.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::members::{Access, keeps_an_admin, manages_members, validate_access};
use domain::permissions::PermissionChange;
use infra::groups::GroupRef;
use infra::members::{self as repo, MemberFilter, TenantMember, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::TenantRoleView;
use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::me::AccountTypeView;
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// OpenAPI tag of the administration of members, invitations and groups.
pub const TAG: &str = "members";

/// 403 unless the signed-in member manages the tenant's members.
pub(crate) fn require_member_admin(tx: &TenantTx) -> Result<(), Problem> {
    if manages_members(tx.member().tenant_role) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden))
    }
}

/// Whether a member is suspended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = MemberStatus)]
pub enum MemberStatusView {
    Active,
    /// Cannot sign in.
    Suspended,
}

/// A group a member belongs to.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = GroupRef)]
pub struct GroupRefView {
    pub id: Uuid,
    #[schema(example = "Platform")]
    pub name: String,
}

impl From<GroupRef> for GroupRefView {
    fn from(group: GroupRef) -> Self {
        Self {
            id: group.id,
            name: group.name,
        }
    }
}

/// A member of the tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Member)]
pub struct MemberView {
    account_id: Uuid,
    #[schema(example = "lucas.bernard@acme.example")]
    email: String,
    #[schema(example = "Lucas Bernard")]
    display_name: String,
    account_type: AccountTypeView,
    tenant_role: TenantRoleView,
    /// Signs in with a password even when the tenant requires SSO
    /// (contractors outside the directory).
    local_account: bool,
    status: MemberStatusView,
    /// Two-step verification is on.
    mfa_enabled: bool,
    /// By name.
    groups: Vec<GroupRefView>,
    /// Version of the membership, for `If-Match`.
    #[schema(example = 1)]
    version: i64,
}

impl MemberView {
    fn new(member: TenantMember, groups: Vec<GroupRefView>) -> Self {
        Self {
            account_id: member.account_id,
            email: member.email,
            display_name: member.display_name,
            account_type: member.account_type.into(),
            tenant_role: member.tenant_role.into(),
            local_account: member.local_account,
            status: if member.active {
                MemberStatusView::Active
            } else {
                MemberStatusView::Suspended
            },
            mfa_enabled: member.mfa_enabled,
            groups,
            version: member.version,
        }
    }
}

/// A page of members.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberPage {
    items: Vec<MemberView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?search=&accountType=&limit=&cursor=` of the members list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct MemberListParams {
    /// Part of the name or email, any case.
    pub search: Option<String>,
    /// Only internal, or only external members.
    pub account_type: Option<AccountTypeView>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// What a tenant admin sets on a member.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = MemberAccess,
    example = json!({"accountType": "internal", "tenantRole": "configuration_admin", "localAccount": false})
)]
pub struct AccessBody {
    account_type: AccountTypeView,
    /// An external member is always `member`.
    tenant_role: TenantRoleView,
    /// Signs in with a password even when the tenant requires SSO.
    local_account: bool,
}

impl AccessBody {
    fn access(self) -> Access {
        Access {
            account_type: self.account_type.into(),
            tenant_role: self.tenant_role.into(),
        }
    }
}

/// The views of `members`, with their groups.
async fn views(tx: &mut TenantTx, members: Vec<TenantMember>) -> Result<Vec<MemberView>, Problem> {
    let ids: Vec<Uuid> = members.iter().map(|m| m.account_id).collect();
    let mut groups: HashMap<Uuid, Vec<GroupRefView>> = HashMap::new();
    for (account_id, group) in infra::groups::of_members(tx, &ids).await? {
        groups.entry(account_id).or_default().push(group.into());
    }
    Ok(members
        .into_iter()
        .map(|m| {
            let of_member = groups.remove(&m.account_id).unwrap_or_default();
            MemberView::new(m, of_member)
        })
        .collect())
}

async fn view_of(tx: &mut TenantTx, account_id: Uuid) -> Result<Option<MemberView>, Problem> {
    let Some(member) = repo::find(tx, account_id).await? else {
        return Ok(None);
    };
    Ok(views(tx, vec![member]).await?.pop())
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

pub(crate) fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
    }
}

/// `GET /api/v1/tenant/members`, in the order they joined.
#[utoipa::path(
    get,
    path = "/tenant/members",
    tag = TAG,
    operation_id = "listMembers",
    summary = "List the members of the tenant",
    description = "Tenant admins only. In the order they joined; follow `nextCursor` until it is \
                   `null`. Pending invitations are listed by `/tenant/invitations`.",
    params(MemberListParams),
    responses(
        (status = 200, description = "A page of members", body = MemberPage),
        (status = 400, description = "Invalid `limit`, `cursor` or `accountType`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<MemberListParams>,
) -> Result<ApiJson<MemberPage>, Problem> {
    require_member_admin(&tx)?;
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let search = params.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let filter = MemberFilter {
        search,
        account_type: params.account_type.map(Into::into),
    };
    let page = repo::list(&mut tx, &filter, after, limit).await?;
    let items = views(&mut tx, page.items).await?;
    tx.commit().await?;
    Ok(ApiJson(MemberPage {
        items,
        next_cursor: page.next.map(encode_cursor),
    }))
}

/// `GET /api/v1/tenant/members/{accountId}`.
#[utoipa::path(
    get,
    path = "/tenant/members/{accountId}",
    tag = TAG,
    operation_id = "getMember",
    summary = "Read a member of the tenant",
    description = "Tenant admins only.",
    params(
        ("accountId" = Uuid, Path, description = "Account of the member"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The member", body = MemberView, headers(
            ("ETag" = String, description = "Version of the membership"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such member in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(account_id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let member = view_of(&mut tx, account_id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tx.commit().await?;
    let version = member.version;
    if if_none_match(&headers, version) {
        return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), version));
    }
    Ok(with_etag(ApiJson(member).into_response(), version))
}

/// `PUT /api/v1/tenant/members/{accountId}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/tenant/members/{accountId}",
    tag = TAG,
    operation_id = "updateMember",
    summary = "Change a member's account type and tenant role",
    description = "Tenant admins only. Applies to the member's next request. A member made \
                   external is signed out of this tenant. The last active tenant admin cannot be \
                   demoted.",
    params(
        ("accountId" = Uuid, Path, description = "Account of the member"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = AccessBody,
    responses(
        (status = 200, description = "The member", body = MemberView, headers(
            ("ETag" = String, description = "New version of the membership"),
        )),
        (status = 400, description = "`validation_failed`: an external member with an admin role", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such member in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`member.last_admin`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The membership changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AccessBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let access = body.access();
    validate_access(access).map_err(Problem::validation)?;

    let current = repo::find(&mut tx, account_id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    let stays_admin = access.tenant_role == domain::account::TenantRole::TenantAdmin
        && access.account_type.reaches_internal_app();
    check_an_admin_stays(&mut tx, &current, stays_admin).await?;
    repo::update_access(
        &mut tx,
        account_id,
        if_match.versions(),
        access,
        body.local_account,
    )
    .await?
    .map_err(conflict_problem)?;
    if !access.account_type.reaches_internal_app() {
        // The internal application is out of reach from now on.
        state
            .auth
            .sessions
            .delete_in_tenant(account_id, tx.tenant_id())
            .await?;
    }
    let member = view_of(&mut tx, account_id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        member = %account_id,
        from_type = current.account_type.as_str(),
        to_type = access.account_type.as_str(),
        from_role = current.tenant_role.as_str(),
        to_role = access.tenant_role.as_str(),
        local_account = body.local_account,
        "member access changed"
    );
    let tenant = tx.tenant_id();
    tx.commit().await?;
    crate::permissions::changed(&state, tenant, PermissionChange::MemberAccessChanged).await;
    let version = member.version;
    Ok(with_etag(ApiJson(member).into_response(), version))
}

/// 409 `member.last_admin` if `current` is the last active tenant admin and
/// stops being one.
pub(crate) async fn check_an_admin_stays(
    tx: &mut TenantTx,
    current: &TenantMember,
    stays_admin: bool,
) -> Result<(), Problem> {
    let was_admin = current.active
        && current.account_type.reaches_internal_app()
        && current.tenant_role == domain::account::TenantRole::TenantAdmin;
    if !was_admin || stays_admin {
        return Ok(());
    }
    let others = repo::other_active_admins(tx, current.account_id).await?;
    if keeps_an_admin(was_admin, stays_admin, others) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::MemberLastAdmin))
    }
}

/// `DELETE /api/v1/tenant/members/{accountId}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/tenant/members/{accountId}",
    tag = TAG,
    operation_id = "removeMember",
    summary = "Remove a member from the tenant",
    description = "Tenant admins only. The member leaves the tenant and its groups, and their \
                   sessions in this tenant end at once. Their account stays, with their other \
                   tenants. The last active tenant admin cannot be removed.",
    params(
        ("accountId" = Uuid, Path, description = "Account of the member"),
        ("If-Match" = String, Header, description = "ETag of the membership, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such member in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`member.last_admin`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The membership changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn remove(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(account_id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let current = repo::find(&mut tx, account_id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    check_an_admin_stays(&mut tx, &current, false).await?;
    repo::remove(&mut tx, account_id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    // Before the commit: if it fails, the member is merely signed out.
    let ended = state
        .auth
        .sessions
        .delete_in_tenant(account_id, tx.tenant_id())
        .await?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        member = %account_id,
        sessions_ended = ended,
        "member removed"
    );
    let tenant = tx.tenant_id();
    tx.commit().await?;
    crate::permissions::changed(&state, tenant, PermissionChange::MemberRemoved).await;
    Ok(StatusCode::NO_CONTENT)
}
