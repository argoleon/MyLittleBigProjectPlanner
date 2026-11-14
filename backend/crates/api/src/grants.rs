//! Grants of roles on hierarchy nodes and effective access (specification
//! §4 and §14, L03-US03).
//!
//! - A role is granted to a member or a group on a node, and applies to the
//!   node and its descendants (`domain::grants`). The effective access of a
//!   node lists every grant that applies there, direct or inherited, with
//!   the node it comes from.
//! - Tenant admins, and the members with "manage members" on the node, see
//!   its access and change its grants; the latter only handle roles within
//!   their own permissions.
//! - Granting or revoking forgets the tenant's cached permissions once
//!   committed: it applies to the next request.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use domain::grants::{may_grant, may_see_access};
use domain::permissions::{Permission, PermissionChange, PermissionSet};
use domain::validation::FieldError;
use infra::grants::{self as repo, Access, Candidate, GrantConflict, Grantee, Holder, NewGrant};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::nodes::{ParentView, Sight, TAG, is_tenant_admin, visible_node};
use crate::permissions::{self, PermissionView};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

/// Whether a role is held by a member or by a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = HolderKind)]
pub enum HolderKindView {
    Member,
    Group,
}

/// Who holds a role: a member or a group.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AccessHolder)]
pub struct HolderView {
    kind: HolderKindView,
    /// Account of the member, or id of the group.
    id: Uuid,
    #[schema(example = "Managers")]
    name: String,
}

impl From<Holder> for HolderView {
    fn from(holder: Holder) -> Self {
        match holder {
            Holder::Member {
                account_id,
                display_name,
            } => Self {
                kind: HolderKindView::Member,
                id: account_id,
                name: display_name,
            },
            Holder::Group { id, name } => Self {
                kind: HolderKindView::Group,
                id,
                name,
            },
        }
    }
}

/// A role, by its label.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AccessRole)]
pub struct RoleRefView {
    pub(crate) id: Uuid,
    /// In the language of the request (`Accept-Language`), else the
    /// tenant's default, else any.
    #[schema(example = "Project manager")]
    pub(crate) label: Option<String>,
    pub(crate) label_translations: BTreeMap<String, String>,
}

/// A grant that applies on a node.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeAccess)]
pub struct AccessView {
    /// Id of the grant, to revoke it on the node it was made on.
    grant_id: Uuid,
    holder: HolderView,
    role: RoleRefView,
    /// The node the role was granted on: this node, or an ancestor; `null`
    /// for an ancestor the member does not see.
    #[schema(required = true)]
    origin: Option<ParentView>,
    /// Granted on this node; otherwise inherited from `origin`.
    direct: bool,
}

fn access_view(access: Access, language: &Language, sight: &Sight) -> AccessView {
    AccessView {
        grant_id: access.grant_id,
        holder: access.holder.into(),
        role: RoleRefView {
            id: access.role_id,
            label: language.label(&access.role_label),
            label_translations: access.role_label,
        },
        // The name of a node they do not see does not leave.
        origin: sight.sees(access.origin.id).then(|| access.origin.into()),
        direct: access.direct,
    }
}

/// A role to grant on a node, to a member or to a group.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewNodeGrant,
    example = json!({"roleId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01", "groupId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b02"})
)]
pub struct GrantBody {
    role_id: Uuid,
    /// The member who receives the role; either this or `groupId`.
    #[serde(default)]
    account_id: Option<Uuid>,
    /// The group that receives the role; either this or `accountId`.
    #[serde(default)]
    group_id: Option<Uuid>,
}

/// A member or a group a role can be granted to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = GrantCandidate)]
pub struct CandidateView {
    kind: HolderKindView,
    /// Account of the member, or id of the group.
    id: Uuid,
    name: String,
    /// Members only.
    email: Option<String>,
}

impl From<Candidate> for CandidateView {
    fn from(candidate: Candidate) -> Self {
        match candidate {
            Candidate::Member {
                account_id,
                display_name,
                email,
            } => Self {
                kind: HolderKindView::Member,
                id: account_id,
                name: display_name,
                email: Some(email),
            },
            Candidate::Group { id, name } => Self {
                kind: HolderKindView::Group,
                id,
                name,
                email: None,
            },
        }
    }
}

/// `?search=&limit=` of the grant candidates.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CandidateParams {
    /// Part of the name (or of a member's email), any case.
    pub search: Option<String>,
    /// At most this many groups and this many members, from 1 to 50
    /// (default 10).
    #[param(minimum = 1, maximum = 50)]
    pub limit: Option<usize>,
}

/// What the signed-in member may do with the grants of a node.
#[derive(Debug, Clone, Copy)]
struct Manager {
    tenant_admin: bool,
    /// Their effective permissions on the node.
    own: PermissionSet,
}

impl Manager {
    fn grants(self, role: PermissionSet) -> bool {
        may_grant(self.tenant_admin, self.own, role)
    }
}

/// The signed-in member as a manager of the access of the node `id`: 404 if
/// they do not see it, 403 `permission.denied` if they do not manage its
/// members.
async fn manager(state: &AppState, tx: &mut TenantTx, id: Uuid) -> Result<Manager, Problem> {
    visible_node(state, tx, id, "no such node").await?;
    let tenant_admin = is_tenant_admin(tx);
    let own = if tenant_admin {
        PermissionSet::all()
    } else {
        permissions::effective(state, tx, Some(id)).await?
    };
    if !may_see_access(tenant_admin, own) {
        permissions::check(own, Permission::ManageMembers)?;
    }
    Ok(Manager { tenant_admin, own })
}

fn beyond_own_permissions() -> Problem {
    Problem::new(ErrorCode::Forbidden).with_detail("this role holds permissions you do not have on this node")
}

fn grant_problem(conflict: GrantConflict) -> Problem {
    let field =
        |name: &'static str, code: &'static str| Problem::validation(vec![FieldError::new(name, code)]);
    match conflict {
        GrantConflict::NodeNotFound => Problem::new(ErrorCode::NotFound).with_detail("no such node"),
        GrantConflict::RoleNotFound => field("roleId", "not_found"),
        GrantConflict::UnknownMember => field("accountId", "unknown_member"),
        GrantConflict::GroupNotFound => field("groupId", "not_found"),
        GrantConflict::Exists => Problem::new(ErrorCode::NodeGrantExists),
    }
}

/// `GET /api/v1/nodes/{id}/access`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/access",
    tag = TAG,
    operation_id = "listNodeAccess",
    summary = "List who has access to a node",
    description = "Every grant that applies on the node: those made on it (`direct`), then those \
                   inherited from an ancestor (`origin`), by holder name. A grant made above a \
                   confidential node does not reach it; the origin of a grant inherited from a \
                   node the member does not see is `null`. Tenant admins, and the members with \
                   `manage_members` on the node.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    responses(
        (status = 200, description = "The grants that apply", body = Vec<AccessView>, headers(
            ("Content-Language" = String, description = "Language of the role labels"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: `manage_members` is missing on this node", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn access(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    manager(&state, &mut tx, id).await?;
    let language = Language::of(&tx, &accept);
    let entries = repo::access(&mut tx, id).await?;
    let sight = Sight::of(&mut tx).await?;
    tx.commit().await?;
    let views: Vec<AccessView> = entries
        .into_iter()
        .map(|entry| access_view(entry, &language, &sight))
        .collect();
    let mut response = ApiJson(views).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/nodes/{id}/grants`.
#[utoipa::path(
    post,
    path = "/nodes/{id}/grants",
    tag = TAG,
    operation_id = "grantNodeRole",
    summary = "Grant a role on a node to a member or a group",
    description = "The role applies on the node and on its descendants, from the next request. \
                   Tenant admins, and the members with `manage_members` on the node, for roles \
                   within their own permissions there.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    request_body = GrantBody,
    responses(
        (status = 201, description = "Granted", body = AccessView),
        (status = 400, description = "`validation_failed`: `roleId` `not_found`; `accountId` (`required`, \
                                      `unknown_member`); `groupId` (`not_found`, `not_allowed` with `accountId`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: `manage_members` is missing on this node; \
                                      `forbidden`: the role is beyond the member's own permissions; \
                                      or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.grant_exists`: already granted to them on this node", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn grant(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<GrantBody>,
) -> Result<Response, Problem> {
    let manager = manager(&state, &mut tx, id).await?;
    let grantee = match (body.account_id, body.group_id) {
        (Some(account), None) => Grantee::Member(account),
        (None, Some(group)) => Grantee::Group(group),
        (None, None) => {
            return Err(Problem::validation(vec![FieldError::new(
                "accountId",
                "required",
            )]));
        }
        (Some(_), Some(_)) => {
            return Err(Problem::validation(vec![FieldError::new(
                "groupId",
                "not_allowed",
            )]));
        }
    };
    let Some(role) = infra::roles::find(&mut tx, body.role_id).await? else {
        return Err(grant_problem(GrantConflict::RoleNotFound));
    };
    if !manager.grants(role.permissions) {
        return Err(beyond_own_permissions());
    }
    let new = NewGrant {
        id: infra::id::new_id(),
        node: id,
        role: role.id,
        grantee,
        created_by: tx.account_id(),
    };
    repo::insert(&mut tx, &new).await?.map_err(grant_problem)?;
    let granted = repo::access(&mut tx, id)
        .await?
        .into_iter()
        .find(|entry| entry.grant_id == new.id)
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        node = %id,
        role = %role.id,
        grantee = ?grantee,
        "node role granted"
    );
    let language = Language::of(&tx, &accept);
    let sight = Sight::of(&mut tx).await?;
    let tenant = tx.tenant_id();
    tx.commit().await?;
    permissions::changed(&state, tenant, PermissionChange::GrantAdded).await;
    let view = access_view(granted, &language, &sight);
    let mut response = (StatusCode::CREATED, ApiJson(view)).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `DELETE /api/v1/nodes/{id}/grants/{grantId}`.
#[utoipa::path(
    delete,
    path = "/nodes/{id}/grants/{grantId}",
    tag = TAG,
    operation_id = "revokeNodeRole",
    summary = "Revoke a role granted on a node",
    description = "Only a grant made on this node: an inherited one is revoked on its `origin`. \
                   Applies from the next request, on the node and its descendants. Tenant admins, \
                   and the members with `manage_members` on the node, for roles within their own \
                   permissions there.",
    params(
        ("id" = Uuid, Path, description = "Id of the node"),
        ("grantId" = Uuid, Path, description = "Id of the grant"),
    ),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: `manage_members` is missing on this node; \
                                      `forbidden`: the role is beyond the member's own permissions; \
                                      or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, not visible to the member, or no such grant on it", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn revoke(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id, grant_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    let manager = manager(&state, &mut tx, id).await?;
    let no_grant = || Problem::new(ErrorCode::NotFound).with_detail("no such grant on this node");
    let role_id = repo::role_of(&mut tx, id, grant_id).await?.ok_or_else(no_grant)?;
    let role = infra::roles::find(&mut tx, role_id).await?.ok_or_else(no_grant)?;
    if !manager.grants(role.permissions) {
        return Err(beyond_own_permissions());
    }
    if !repo::delete(&mut tx, id, grant_id).await? {
        return Err(no_grant());
    }
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        node = %id,
        role = %role_id,
        grant = %grant_id,
        "node role revoked"
    );
    let tenant = tx.tenant_id();
    tx.commit().await?;
    permissions::changed(&state, tenant, PermissionChange::GrantRemoved).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/nodes/{id}/grant-candidates`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/grant-candidates",
    tag = TAG,
    operation_id = "listNodeGrantCandidates",
    summary = "Search the members and groups a role can be granted to",
    description = "The groups, then the active members of the tenant, whose name (or email) holds \
                   `search`, each by name. Tenant admins, and the members with `manage_members` \
                   on the node.",
    params(("id" = Uuid, Path, description = "Id of the node"), CandidateParams),
    responses(
        (status = 200, description = "The candidates", body = Vec<CandidateView>),
        (status = 400, description = "Invalid `limit`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: `manage_members` is missing on this node", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn candidates(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<CandidateParams>,
) -> Result<ApiJson<Vec<CandidateView>>, Problem> {
    let limit = params.limit.unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("limit: 1 to 50"));
    }
    manager(&state, &mut tx, id).await?;
    let search = params.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let found = repo::candidates(&mut tx, search, limit).await?;
    tx.commit().await?;
    Ok(ApiJson(found.into_iter().map(CandidateView::from).collect()))
}

/// `GET /api/v1/nodes/{id}/grantable-roles`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/grantable-roles",
    tag = TAG,
    operation_id = "listNodeGrantableRoles",
    summary = "List the roles the member may grant on a node",
    description = "Every role of the tenant for tenant admins; for the members with \
                   `manage_members` on the node, the roles within their own permissions there. \
                   From the narrowest default role to the newest.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    responses(
        (status = 200, description = "The roles", body = Vec<RoleRefView>, headers(
            ("Content-Language" = String, description = "Language of the role labels"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: `manage_members` is missing on this node", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn grantable_roles(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let manager = manager(&state, &mut tx, id).await?;
    let language = Language::of(&tx, &accept);
    let roles = infra::roles::list(&mut tx, None, MAX_ROLES).await?.items;
    tx.commit().await?;
    let views: Vec<RoleRefView> = roles
        .into_iter()
        .filter(|role| manager.grants(role.permissions))
        .map(|role| RoleRefView {
            id: role.id,
            label: language.label(&role.label_translations),
            label_translations: role.label_translations,
        })
        .collect();
    let mut response = ApiJson(views).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// Roles offered at most in the grant dialog.
const MAX_ROLES: usize = 200;

/// `GET /api/v1/nodes/{id}/my-permissions`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/my-permissions",
    tag = TAG,
    operation_id = "listMyNodePermissions",
    summary = "List the signed-in member's permissions on a node",
    description = "The union of the roles that reach them there: granted to them or to one of \
                   their groups, on the node or inherited from an ancestor. Tenant admins have \
                   every permission. In catalog order.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    responses(
        (status = 200, description = "Their permissions", body = Vec<PermissionView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn my_permissions(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<ApiJson<Vec<PermissionView>>, Problem> {
    visible_node(&state, &mut tx, id, "no such node").await?;
    let own = if is_tenant_admin(&tx) {
        PermissionSet::all()
    } else {
        permissions::effective(&state, &mut tx, Some(id)).await?
    };
    tx.commit().await?;
    Ok(ApiJson(permissions::views(own)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicts_become_stable_problems() {
        let cases = [
            (GrantConflict::NodeNotFound, 404, "not_found"),
            (GrantConflict::RoleNotFound, 400, "validation_failed"),
            (GrantConflict::UnknownMember, 400, "validation_failed"),
            (GrantConflict::GroupNotFound, 400, "validation_failed"),
            (GrantConflict::Exists, 409, "node.grant_exists"),
        ];
        for (conflict, status, code) in cases {
            let problem = grant_problem(conflict);
            assert_eq!(
                (problem.code.status().as_u16(), problem.code.as_str()),
                (status, code),
                "{conflict:?}"
            );
        }
        let beyond = beyond_own_permissions();
        assert_eq!(
            (beyond.code.status().as_u16(), beyond.code.as_str()),
            (403, "forbidden")
        );
    }

    #[test]
    fn managers_only_hand_out_what_they_have() {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        let manager = Manager {
            tenant_admin: false,
            own: reader.with(Permission::ManageMembers),
        };
        assert!(manager.grants(reader));
        assert!(!manager.grants(PermissionSet::all()));
        let admin = Manager {
            tenant_admin: true,
            own: PermissionSet::all(),
        };
        assert!(admin.grants(PermissionSet::all()));
    }

    #[test]
    fn holders_and_candidates_say_their_kind() {
        let group = HolderView::from(Holder::Group {
            id: Uuid::nil(),
            name: "Managers".to_owned(),
        });
        let json = serde_json::to_value(&group).unwrap_or_default();
        assert_eq!(
            (&json["kind"], &json["name"]),
            (&"group".into(), &"Managers".into())
        );
        let member = CandidateView::from(Candidate::Member {
            account_id: Uuid::nil(),
            display_name: "Camille Martin".to_owned(),
            email: "camille@acme.example".to_owned(),
        });
        let json = serde_json::to_value(&member).unwrap_or_default();
        assert_eq!(
            (&json["kind"], &json["email"]),
            (&"member".into(), &"camille@acme.example".into())
        );
    }
}
