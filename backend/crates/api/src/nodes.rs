//! The hierarchy's nodes (specification §4, L03-US01): portfolios, programs
//! and projects.
//!
//! - Tenant admins create nodes anywhere; the administrators of a node
//!   ("administer node", directly or inherited) create children under it and
//!   change it. Grants on nodes come with L03-US03.
//! - A node is visible to tenant admins and to the members with "view" on
//!   it; to anyone else it does not exist (404).
//! - The attachment matrix is checked here and by the database.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, NaiveDate};
use domain::account::TenantRole;
use domain::hierarchy::{
    Health, NodeInput, NodeType, Rights, may_attach, may_change, may_create, validate_node,
};
use domain::permissions::Permission;
use domain::validation::FieldError;
use infra::nodes::{self as repo, NewNode, Node, NodeFields, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::pagination::{PageParams, encode_cursor};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// OpenAPI tag of the hierarchy.
pub const TAG: &str = "hierarchy";

const PATH: &str = "/api/v1/nodes";

/// Kind of node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = NodeType)]
pub enum NodeTypeView {
    Portfolio,
    Program,
    /// Holds tickets; has a key.
    Project,
}

impl From<NodeTypeView> for NodeType {
    fn from(view: NodeTypeView) -> Self {
        match view {
            NodeTypeView::Portfolio => Self::Portfolio,
            NodeTypeView::Program => Self::Program,
            NodeTypeView::Project => Self::Project,
        }
    }
}

impl From<NodeType> for NodeTypeView {
    fn from(node_type: NodeType) -> Self {
        match node_type {
            NodeType::Portfolio => Self::Portfolio,
            NodeType::Program => Self::Program,
            NodeType::Project => Self::Project,
        }
    }
}

/// How a node stands, as its owners see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = NodeHealth)]
pub enum HealthView {
    #[default]
    OnTrack,
    AtRisk,
    OffTrack,
}

impl From<HealthView> for Health {
    fn from(view: HealthView) -> Self {
        match view {
            HealthView::OnTrack => Self::OnTrack,
            HealthView::AtRisk => Self::AtRisk,
            HealthView::OffTrack => Self::OffTrack,
        }
    }
}

impl From<Health> for HealthView {
    fn from(health: Health) -> Self {
        match health {
            Health::OnTrack => Self::OnTrack,
            Health::AtRisk => Self::AtRisk,
            Health::OffTrack => Self::OffTrack,
        }
    }
}

/// A member owning a node.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeOwner)]
pub struct OwnerView {
    account_id: Uuid,
    #[schema(example = "Marion Garnier")]
    display_name: String,
}

/// A parent of a node.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeParent)]
pub struct ParentView {
    id: Uuid,
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    #[schema(example = "IT Modernization")]
    name: String,
}

/// A portfolio, program or project.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Node)]
pub struct NodeView {
    id: Uuid,
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    #[schema(example = "Identity platform")]
    name: String,
    /// Projects only, unique in the tenant.
    #[schema(example = "PLAT")]
    key: Option<String>,
    description: String,
    objectives: String,
    /// Local planning date.
    start_date: Option<NaiveDate>,
    /// Local planning date, not before `startDate`.
    end_date: Option<NaiveDate>,
    health: HealthView,
    /// Read-only, out of the active lists (L03-US06).
    archived: bool,
    /// Blocks inherited access (L03-US04).
    confidential: bool,
    owners: Vec<OwnerView>,
    /// Empty at the root.
    parents: Vec<ParentView>,
    #[schema(example = 1)]
    version: i64,
}

impl From<Node> for NodeView {
    fn from(node: Node) -> Self {
        Self {
            id: node.id,
            node_type: node.node_type.into(),
            name: node.name,
            key: node.key,
            description: node.description,
            objectives: node.objectives,
            start_date: node.start_date,
            end_date: node.end_date,
            health: node.health.into(),
            archived: node.archived,
            confidential: node.confidential,
            owners: node
                .owners
                .into_iter()
                .map(|o| OwnerView {
                    account_id: o.account_id,
                    display_name: o.display_name,
                })
                .collect(),
            parents: node
                .parents
                .into_iter()
                .map(|p| ParentView {
                    id: p.id,
                    node_type: p.node_type.into(),
                    name: p.name,
                })
                .collect(),
            version: node.version,
        }
    }
}

/// A page of nodes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodePage {
    items: Vec<NodeView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?type=&limit=&cursor=` of the nodes list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NodeListParams {
    /// Only nodes of this type.
    #[serde(rename = "type")]
    #[param(rename = "type", value_type = Option<NodeTypeView>)]
    pub node_type: Option<NodeTypeView>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// What can be changed on a node.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NodeChanges)]
pub struct ChangesBody {
    /// 1 to 200 characters.
    name: String,
    /// 10,000 characters at most.
    #[serde(default)]
    description: String,
    /// 10,000 characters at most.
    #[serde(default)]
    objectives: String,
    #[serde(default)]
    start_date: Option<NaiveDate>,
    #[serde(default)]
    end_date: Option<NaiveDate>,
    #[serde(default)]
    health: HealthView,
    /// Members of the tenant, 20 at most.
    #[serde(default)]
    owner_ids: Vec<Uuid>,
}

/// A node to create.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewNode,
    example = json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": null})
)]
pub struct CreateBody {
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    /// 1 to 200 characters.
    name: String,
    /// Projects only (required for them): 2 to 10 capital letters and
    /// digits, a letter first; unique in the tenant.
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    objectives: String,
    #[serde(default)]
    start_date: Option<NaiveDate>,
    #[serde(default)]
    end_date: Option<NaiveDate>,
    #[serde(default)]
    health: HealthView,
    #[serde(default)]
    owner_ids: Vec<Uuid>,
    /// The parent to create it under; at the root when absent.
    #[serde(default)]
    parent_id: Option<Uuid>,
}

fn ymd(date: Option<NaiveDate>) -> Option<(i32, u32, u32)> {
    date.map(|d| (d.year(), d.month(), d.day()))
}

fn input(name: &str, key: Option<&str>, changes: &ChangesBody) -> NodeInput {
    NodeInput {
        name: name.to_owned(),
        key: key.map(str::to_owned),
        description: changes.description.clone(),
        objectives: changes.objectives.clone(),
        start_date: ymd(changes.start_date),
        end_date: ymd(changes.end_date),
        owner_count: changes.owner_ids.len(),
    }
}

fn fields(changes: &ChangesBody) -> NodeFields<'_> {
    NodeFields {
        name: &changes.name,
        description: &changes.description,
        objectives: &changes.objectives,
        start_date: changes.start_date,
        end_date: changes.end_date,
        health: changes.health.into(),
        owners: &changes.owner_ids,
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    let field =
        |name: &'static str, code: &'static str| Problem::validation(vec![FieldError::new(name, code)]);
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::KeyTaken => {
            Problem::new(ErrorCode::NodeKeyTaken).with_errors(vec![FieldError::new("key", "taken")])
        }
        WriteConflict::ParentNotFound => field("parentId", "not_found"),
        WriteConflict::ParentNotAllowed => field("parentId", "not_allowed"),
        WriteConflict::Cycle => Problem::new(ErrorCode::NodeCycle),
        WriteConflict::UnknownOwner => field("ownerIds", "unknown_member"),
    }
}

fn is_tenant_admin(tx: &TenantTx) -> bool {
    tx.member().tenant_role == TenantRole::TenantAdmin
}

/// The signed-in member's rights on `node`.
async fn rights(state: &AppState, tx: &mut TenantTx, node: Uuid) -> Result<Rights, Problem> {
    let tenant_admin = is_tenant_admin(tx);
    let administers_node = !tenant_admin
        && permissions::effective(state, tx, Some(node))
            .await?
            .contains(Permission::AdministerNode);
    Ok(Rights {
        tenant_admin,
        administers_node,
    })
}

/// Whether the signed-in member sees `node`.
async fn sees(state: &AppState, tx: &mut TenantTx, node: Uuid) -> Result<bool, Problem> {
    Ok(is_tenant_admin(tx)
        || permissions::effective(state, tx, Some(node))
            .await?
            .contains(Permission::View))
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response.headers_mut().insert(header::ETAG, etag(version));
    response
}

/// `GET /api/v1/nodes`.
#[utoipa::path(
    get,
    path = "/nodes",
    tag = TAG,
    operation_id = "listNodes",
    summary = "List the portfolios, programs and projects",
    description = "In creation order; follow `nextCursor` until it is `null`. Only the nodes the \
                   member sees: a page may hold fewer items than `limit` without being the last.",
    params(NodeListParams),
    responses(
        (status = 200, description = "A page of nodes", body = NodePage),
        (status = 400, description = "Invalid `type`, `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<NodeListParams>,
) -> Result<ApiJson<NodePage>, Problem> {
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let page = repo::list(&mut tx, params.node_type.map(NodeType::from), after, limit).await?;
    let mut items = Vec::with_capacity(page.items.len());
    for node in page.items {
        if sees(&state, &mut tx, node.id).await? {
            items.push(NodeView::from(node));
        }
    }
    tx.commit().await?;
    Ok(ApiJson(NodePage {
        items,
        next_cursor: page.next.map(encode_cursor),
    }))
}

/// `POST /api/v1/nodes`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/nodes",
    tag = TAG,
    operation_id = "createNode",
    summary = "Create a portfolio, program or project",
    description = "At the root: tenant admins only. Under `parentId`: tenant admins, and the \
                   members who administer that node. The type must be allowed under the parent: \
                   a portfolio under a portfolio, a program under a portfolio or a program, a \
                   project under any node.",
    params(("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters")),
    request_body = CreateBody,
    responses(
        (status = 201, description = "Created", body = NodeView, headers(
            ("ETag" = String, description = "Version of the node"),
            ("Location" = String, description = "URL of the node"),
        )),
        (status = 400, description = "`validation_failed`: `name`, `key` (`required`, `format`, `not_allowed`), \
                                      `endDate` `before_start`, `ownerIds` (`too_many`, `unknown_member`), \
                                      `parentId` (`not_found`, `not_allowed`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: may not create there, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.key_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<Response, Problem> {
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let node_type = NodeType::from(body.node_type);
    // Who creates here: the parent decides.
    let parent = match body.parent_id {
        Some(id) => Some(repo::find(&mut tx, id).await?),
        None => None,
    };
    let allowed = match &parent {
        Some(Some(parent)) => may_create(rights(&state, &mut tx, parent.id).await?, true),
        // At the root, or under an unknown parent (only those who may create
        // anywhere learn that it does not exist).
        Some(None) | None => may_create(
            Rights {
                tenant_admin: is_tenant_admin(&tx),
                administers_node: false,
            },
            false,
        ),
    };
    if !allowed {
        return Err(Problem::new(ErrorCode::Forbidden));
    }

    let changes = ChangesBody {
        name: body.name.clone(),
        description: body.description.clone(),
        objectives: body.objectives.clone(),
        start_date: body.start_date,
        end_date: body.end_date,
        health: body.health,
        owner_ids: body.owner_ids.clone(),
    };
    let node_key = body.key.as_deref().map(str::trim);
    let mut errors = validate_node(node_type, &input(&body.name, node_key, &changes))
        .err()
        .unwrap_or_default();
    match &parent {
        Some(None) => errors.push(FieldError::new("parentId", "not_found")),
        Some(Some(parent)) if !may_attach(parent.node_type, node_type) => {
            errors.push(FieldError::new("parentId", "not_allowed"));
        }
        _ => {}
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }

    let new = NewNode {
        id: infra::id::new_id(),
        node_type,
        key: node_key,
        fields: fields(&changes),
        parent: body.parent_id,
        created_by: tx.account_id(),
    };
    let node = repo::insert(&mut tx, &new).await?.map_err(conflict_problem)?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        node = %node.id,
        node_type = node_type.as_str(),
        parent = ?body.parent_id,
        "node created"
    );
    let (id, version) = (node.id, node.version);
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &NodeView::from(node),
        format!("{PATH}/{id}"),
        version,
    )
    .await?;
    let tenant = tx.tenant_id();
    tx.commit().await?;
    // A new node under a parent inherits its grants (L03-US03).
    permissions::invalidate(&state, tenant).await;
    Ok(response)
}

/// `GET /api/v1/nodes/{id}`.
#[utoipa::path(
    get,
    path = "/nodes/{id}",
    tag = TAG,
    operation_id = "getNode",
    summary = "Read a portfolio, program or project",
    params(
        ("id" = Uuid, Path, description = "Id of the node"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The node", body = NodeView, headers(
            ("ETag" = String, description = "Version of the node"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let node = repo::find(&mut tx, id).await?;
    let visible = match &node {
        Some(_) => sees(&state, &mut tx, id).await?,
        None => false,
    };
    let node = node
        .filter(|_| visible)
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tx.commit().await?;
    let version = node.version;
    let response = if if_none_match(&headers, version) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ApiJson(NodeView::from(node)).into_response()
    };
    Ok(with_etag(response, version))
}

/// `PUT /api/v1/nodes/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/nodes/{id}",
    tag = TAG,
    operation_id = "updateNode",
    summary = "Change a portfolio, program or project",
    description = "Tenant admins, and the members who administer the node. The type and the key \
                   do not change; the owners are replaced by `ownerIds`.",
    params(
        ("id" = Uuid, Path, description = "Id of the node"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = ChangesBody,
    responses(
        (status = 200, description = "The node", body = NodeView, headers(
            ("ETag" = String, description = "New version of the node"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: may not change it, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The node changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ChangesBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let current = repo::find(&mut tx, id).await?;
    let Some(current) = current else {
        return Err(Problem::new(ErrorCode::NotFound));
    };
    if !sees(&state, &mut tx, id).await? {
        return Err(Problem::new(ErrorCode::NotFound));
    }
    if !may_change(rights(&state, &mut tx, id).await?) {
        return Err(Problem::new(ErrorCode::Forbidden));
    }
    validate_node(
        current.node_type,
        &input(&body.name, current.key.as_deref(), &body),
    )
    .map_err(Problem::validation)?;
    let node = repo::update(&mut tx, id, if_match.versions(), &fields(&body))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), node = %id, "node changed");
    tx.commit().await?;
    let version = node.version;
    Ok(with_etag(ApiJson(NodeView::from(node)).into_response(), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_follow_the_domain() {
        for node_type in NodeType::ALL {
            assert_eq!(NodeType::from(NodeTypeView::from(node_type)), node_type);
            let json = serde_json::to_value(NodeTypeView::from(node_type)).unwrap_or_default();
            assert_eq!(json, node_type.as_str());
        }
        for health in Health::ALL {
            assert_eq!(Health::from(HealthView::from(health)), health);
            let json = serde_json::to_value(HealthView::from(health)).unwrap_or_default();
            assert_eq!(json, health.as_str());
        }
    }

    #[test]
    fn conflicts_become_stable_problems() {
        let cases = [
            (WriteConflict::NotFound, 404, "not_found"),
            (WriteConflict::VersionMismatch, 412, "precondition_failed"),
            (WriteConflict::KeyTaken, 409, "node.key_taken"),
            (WriteConflict::ParentNotFound, 400, "validation_failed"),
            (WriteConflict::ParentNotAllowed, 400, "validation_failed"),
            (WriteConflict::Cycle, 409, "node.cycle"),
            (WriteConflict::UnknownOwner, 400, "validation_failed"),
        ];
        for (conflict, status, code) in cases {
            let problem = conflict_problem(conflict);
            assert_eq!(
                (problem.code.status().as_u16(), problem.code.as_str()),
                (status, code),
                "{conflict:?}"
            );
        }
    }
}
