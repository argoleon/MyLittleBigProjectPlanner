//! The hierarchy's nodes (specification §4, L03-US01): portfolios, programs
//! and projects.
//!
//! - Tenant admins create nodes anywhere; the administrators of a node
//!   ("administer node", directly or inherited) create children under it and
//!   change it. Roles are granted on nodes in `crate::grants`.
//! - A node is visible to tenant admins and to the members with "view" on
//!   it; to anyone else it does not exist (404).
//! - The attachment matrix is checked here and by the database.
//! - A node has any number of parents (L03-US02), each link with a
//!   weighting; tenant admins, and the members who administer both nodes,
//!   link them.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, NaiveDate};
use domain::account::TenantRole;
use domain::hierarchy::{
    DEFAULT_WEIGHT_PERCENT, Health, NodeInput, NodeType, ParentCandidate, ParentRefusal, Rights,
    is_weight_percent, may_attach, may_change, may_create, may_link, parent_refusal, validate_node,
};
use domain::permissions::{Permission, PermissionChange};
use domain::validation::FieldError;
use infra::nodes::{
    self as repo, Candidate, Link, LinkChange, NewNode, Node, NodeFields, NodeFilter, NodeRef, WriteConflict,
};
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

impl From<NodeRef> for ParentView {
    fn from(node: NodeRef) -> Self {
        Self {
            id: node.id,
            node_type: node.node_type.into(),
            name: node.name,
        }
    }
}

/// A parent or a child of a node, with the weighting of their link.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeLink)]
pub struct LinkView {
    /// Id of the parent or child.
    id: Uuid,
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    #[schema(example = "Compliance")]
    name: String,
    /// Projects only.
    key: Option<String>,
    /// Share of the child in the parent's aggregates, 0 to 100.
    #[schema(example = 40)]
    weight_percent: i16,
    /// The parents of that parent or child, for context.
    parents: Vec<ParentView>,
}

impl From<Link> for LinkView {
    fn from(link: Link) -> Self {
        Self {
            id: link.id,
            node_type: link.node_type.into(),
            name: link.name,
            key: link.key,
            weight_percent: link.weight_percent,
            parents: link.parents.into_iter().map(ParentView::from).collect(),
        }
    }
}

/// Why a node cannot become the parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ParentRefusal)]
pub enum RefusalView {
    /// It already is a parent.
    AlreadyParent,
    /// The attachment matrix does not allow it.
    NotAllowed,
    /// It is a descendant of the node: the link would make a cycle.
    Cycle,
    /// The member does not administer both nodes.
    Forbidden,
}

impl From<ParentRefusal> for RefusalView {
    fn from(refusal: ParentRefusal) -> Self {
        match refusal {
            ParentRefusal::AlreadyParent => Self::AlreadyParent,
            ParentRefusal::NotAllowed => Self::NotAllowed,
            ParentRefusal::Cycle => Self::Cycle,
            ParentRefusal::Forbidden => Self::Forbidden,
        }
    }
}

/// A node that might become a parent.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeCandidate)]
pub struct CandidateView {
    id: Uuid,
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    #[schema(example = "Cyber resilience")]
    name: String,
    key: Option<String>,
    /// Its own parents, for context.
    parents: Vec<ParentView>,
    /// Why it cannot become the parent; `null` when it can.
    #[schema(required = true)]
    refusal: Option<RefusalView>,
}

/// The weighting of a parent link.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NodeLinkChanges, example = json!({"weightPercent": 40}))]
pub struct LinkBody {
    /// Share of the child in the parent's aggregates, 0 to 100 (default 100).
    #[serde(default = "default_weight")]
    #[schema(default = 100, minimum = 0, maximum = 100)]
    weight_percent: i16,
}

const fn default_weight() -> i16 {
    DEFAULT_WEIGHT_PERCENT
}

/// `?search=&limit=` of the candidate parents.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CandidateParams {
    /// Part of the name or key, any case.
    pub search: Option<String>,
    /// At most this many, from 1 to 50 (default 20).
    #[param(minimum = 1, maximum = 50)]
    pub limit: Option<usize>,
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
            parents: node.parents.into_iter().map(ParentView::from).collect(),
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

/// `?type=&ancestorOf=&descendantOf=&limit=&cursor=` of the nodes list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct NodeListParams {
    /// Only nodes of this type.
    #[serde(rename = "type")]
    #[param(rename = "type", value_type = Option<NodeTypeView>)]
    pub node_type: Option<NodeTypeView>,
    /// Only the ancestors of this node, at any depth.
    pub ancestor_of: Option<Uuid>,
    /// Only the descendants of this node, at any depth; a node reached by
    /// several paths comes once.
    pub descendant_of: Option<Uuid>,
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

pub(crate) fn is_tenant_admin(tx: &TenantTx) -> bool {
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
                   member sees: all of them for tenant admins; for the others, those where a role \
                   with `view` reaches them, directly, through a group, or inherited.",
    params(NodeListParams),
    responses(
        (status = 200, description = "A page of nodes", body = NodePage),
        (status = 400, description = "Invalid `type`, `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<NodeListParams>,
) -> Result<ApiJson<NodePage>, Problem> {
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let filter = NodeFilter {
        node_type: params.node_type.map(NodeType::from),
        ancestor_of: params.ancestor_of,
        descendant_of: params.descendant_of,
        // Tenant admins see every node; the others, those where a role with
        // "view" reaches them.
        visible_to: (!is_tenant_admin(&tx)).then(|| tx.account_id()),
    };
    let page = repo::list(&mut tx, &filter, after, limit).await?;
    tx.commit().await?;
    Ok(ApiJson(NodePage {
        items: page.items.into_iter().map(NodeView::from).collect(),
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
    // Nothing is cached for a new node: what it inherits is computed when
    // it is first read.
    tx.commit().await?;
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

/// The node `id` if the signed-in member sees it, else 404 (`what` names it
/// in the problem's detail).
pub(crate) async fn visible_node(
    state: &AppState,
    tx: &mut TenantTx,
    id: Uuid,
    what: &'static str,
) -> Result<Node, Problem> {
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail(what);
    let node = repo::find(tx, id).await?.ok_or_else(not_found)?;
    if sees(state, tx, id).await? {
        Ok(node)
    } else {
        Err(not_found())
    }
}

/// Only the links to nodes the signed-in member sees.
async fn visible_links(
    state: &AppState,
    tx: &mut TenantTx,
    links: Vec<Link>,
) -> Result<Vec<LinkView>, Problem> {
    let mut views = Vec::with_capacity(links.len());
    for link in links {
        if sees(state, tx, link.id).await? {
            views.push(LinkView::from(link));
        }
    }
    Ok(views)
}

fn link_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::ParentNotAllowed => Problem::new(ErrorCode::NodeParentNotAllowed),
        WriteConflict::Cycle => Problem::new(ErrorCode::NodeCycle),
        WriteConflict::ParentNotFound => Problem::new(ErrorCode::NotFound).with_detail("no such parent"),
        other => conflict_problem(other),
    }
}

/// `GET /api/v1/nodes/{id}/parents`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/parents",
    tag = TAG,
    operation_id = "listNodeParents",
    summary = "List the parents of a node",
    description = "By name, each with the weighting of its link; only the parents the member sees.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    responses(
        (status = 200, description = "The parents", body = Vec<LinkView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn parents(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<ApiJson<Vec<LinkView>>, Problem> {
    visible_node(&state, &mut tx, id, "no such node").await?;
    let links = repo::parent_links(&mut tx, id).await?;
    let views = visible_links(&state, &mut tx, links).await?;
    tx.commit().await?;
    Ok(ApiJson(views))
}

/// `GET /api/v1/nodes/{id}/children`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/children",
    tag = TAG,
    operation_id = "listNodeChildren",
    summary = "List the children of a node",
    description = "By name, each with the weighting of its link; only the children the member sees. \
                   Deeper descendants: `GET /nodes?descendantOf=`.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    responses(
        (status = 200, description = "The children", body = Vec<LinkView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn children(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<ApiJson<Vec<LinkView>>, Problem> {
    visible_node(&state, &mut tx, id, "no such node").await?;
    let links = repo::child_links(&mut tx, id).await?;
    let views = visible_links(&state, &mut tx, links).await?;
    tx.commit().await?;
    Ok(ApiJson(views))
}

/// `PUT /api/v1/nodes/{id}/parents/{parentId}`.
#[utoipa::path(
    put,
    path = "/nodes/{id}/parents/{parentId}",
    tag = TAG,
    operation_id = "linkNodeParent",
    summary = "Attach a node under a parent, or change the link's weighting",
    description = "Tenant admins, and the members who administer both nodes. The node's type must \
                   be allowed under the parent, and the parent must not be one of its descendants. \
                   Putting an existing link again sets its weighting.",
    params(
        ("id" = Uuid, Path, description = "Id of the child node"),
        ("parentId" = Uuid, Path, description = "Id of the parent node"),
    ),
    request_body = LinkBody,
    responses(
        (status = 200, description = "The link existed: its weighting is set", body = LinkView),
        (status = 201, description = "Linked", body = LinkView),
        (status = 400, description = "`validation_failed`: `weightPercent` `out_of_range`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: may not link these nodes, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node or parent in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.parent_not_allowed` (attachment matrix) or `node.cycle`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn link_parent(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id, parent_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<LinkBody>,
) -> Result<Response, Problem> {
    let child = visible_node(&state, &mut tx, id, "no such node").await?;
    let parent = visible_node(&state, &mut tx, parent_id, "no such parent").await?;
    let child_rights = rights(&state, &mut tx, id).await?;
    let parent_rights = rights(&state, &mut tx, parent_id).await?;
    if !may_link(child_rights, parent_rights) {
        return Err(Problem::new(ErrorCode::Forbidden));
    }
    if !is_weight_percent(body.weight_percent) {
        return Err(Problem::validation(vec![FieldError::new(
            "weightPercent",
            "out_of_range",
        )]));
    }
    if !may_attach(parent.node_type, child.node_type) {
        return Err(Problem::new(ErrorCode::NodeParentNotAllowed));
    }
    let change = repo::link(&mut tx, parent_id, id, body.weight_percent)
        .await?
        .map_err(link_problem)?;
    if change != LinkChange::Unchanged {
        tracing::info!(
            target: "audit",
            actor = %tx.account_id(),
            node = %id,
            parent = %parent_id,
            weight_percent = body.weight_percent,
            "node parent linked"
        );
    }
    let view = LinkView {
        id: parent.id,
        node_type: parent.node_type.into(),
        name: parent.name,
        key: parent.key,
        weight_percent: body.weight_percent,
        parents: parent.parents.into_iter().map(ParentView::from).collect(),
    };
    let tenant = tx.tenant_id();
    tx.commit().await?;
    // A new link: the node now inherits the parent's grants.
    let (status, changed) = match change {
        LinkChange::Created => (StatusCode::CREATED, Some(PermissionChange::ParentLinked)),
        LinkChange::Updated => (StatusCode::OK, Some(PermissionChange::LinkWeightChanged)),
        LinkChange::Unchanged => (StatusCode::OK, None),
    };
    if let Some(changed) = changed {
        permissions::changed(&state, tenant, changed).await;
    }
    Ok((status, ApiJson(view)).into_response())
}

/// `DELETE /api/v1/nodes/{id}/parents/{parentId}`.
#[utoipa::path(
    delete,
    path = "/nodes/{id}/parents/{parentId}",
    tag = TAG,
    operation_id = "unlinkNodeParent",
    summary = "Detach a node from one of its parents",
    description = "Tenant admins, and the members who administer both nodes. Without parents left, \
                   the node is at the root.",
    params(
        ("id" = Uuid, Path, description = "Id of the child node"),
        ("parentId" = Uuid, Path, description = "Id of the parent node"),
    ),
    responses(
        (status = 204, description = "Detached"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: may not unlink these nodes, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node, parent or link in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn unlink_parent(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id, parent_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    visible_node(&state, &mut tx, id, "no such node").await?;
    visible_node(&state, &mut tx, parent_id, "no such parent").await?;
    let child_rights = rights(&state, &mut tx, id).await?;
    let parent_rights = rights(&state, &mut tx, parent_id).await?;
    if !may_link(child_rights, parent_rights) {
        return Err(Problem::new(ErrorCode::Forbidden));
    }
    if !repo::unlink(&mut tx, parent_id, id).await? {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("not a parent of this node"));
    }
    tracing::info!(target: "audit", actor = %tx.account_id(), node = %id, parent = %parent_id, "node parent unlinked");
    let tenant = tx.tenant_id();
    tx.commit().await?;
    // The node no longer inherits the parent's grants.
    permissions::changed(&state, tenant, PermissionChange::ParentUnlinked).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/nodes/{id}/parent-candidates`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/parent-candidates",
    tag = TAG,
    operation_id = "listNodeParentCandidates",
    summary = "Search the nodes that might become a parent",
    description = "Active nodes the member sees, other than the node, whose name or key holds \
                   `search`, by name. Each says why it cannot become the parent (`refusal`), or \
                   `null` when it can.",
    params(("id" = Uuid, Path, description = "Id of the child node"), CandidateParams),
    responses(
        (status = 200, description = "The candidates", body = Vec<CandidateView>),
        (status = 400, description = "Invalid `limit`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn parent_candidates(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<CandidateParams>,
) -> Result<ApiJson<Vec<CandidateView>>, Problem> {
    let limit = params.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("limit: 1 to 50"));
    }
    let child = visible_node(&state, &mut tx, id, "no such node").await?;
    let child_rights = rights(&state, &mut tx, id).await?;
    let search = params.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let candidates = repo::parent_candidates(&mut tx, id, search, limit).await?;
    let mut views = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !sees(&state, &mut tx, candidate.id).await? {
            continue;
        }
        let parent_rights = rights(&state, &mut tx, candidate.id).await?;
        views.push(candidate_view(
            candidate,
            child.node_type,
            may_link(child_rights, parent_rights),
        ));
    }
    tx.commit().await?;
    Ok(ApiJson(views))
}

fn candidate_view(candidate: Candidate, child_type: NodeType, may_link: bool) -> CandidateView {
    let refusal = parent_refusal(ParentCandidate {
        candidate_type: candidate.node_type,
        child_type,
        descends_from_child: candidate.descends_from_child,
        already_parent: candidate.already_parent,
        may_link,
    });
    CandidateView {
        id: candidate.id,
        node_type: candidate.node_type.into(),
        name: candidate.name,
        key: candidate.key,
        parents: candidate.parents.into_iter().map(ParentView::from).collect(),
        refusal: refusal.map(RefusalView::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_conflicts_have_their_own_codes() {
        let cases = [
            (WriteConflict::ParentNotAllowed, 409, "node.parent_not_allowed"),
            (WriteConflict::Cycle, 409, "node.cycle"),
            (WriteConflict::ParentNotFound, 404, "not_found"),
            (WriteConflict::NotFound, 404, "not_found"),
        ];
        for (conflict, status, code) in cases {
            let problem = link_problem(conflict);
            assert_eq!(
                (problem.code.status().as_u16(), problem.code.as_str()),
                (status, code),
                "{conflict:?}"
            );
        }
    }

    #[test]
    fn candidates_carry_their_refusal() {
        let candidate = Candidate {
            id: Uuid::nil(),
            node_type: NodeType::Project,
            name: "Cyber permissions".to_owned(),
            key: Some("CYBP".to_owned()),
            parents: vec![],
            descends_from_child: true,
            already_parent: false,
        };
        let view = candidate_view(candidate.clone(), NodeType::Project, true);
        assert_eq!(view.refusal, Some(RefusalView::Cycle));
        let json = serde_json::to_value(&view).unwrap_or_default();
        assert_eq!(json["refusal"], "cycle");
        let free = Candidate {
            descends_from_child: false,
            ..candidate
        };
        let view = candidate_view(free, NodeType::Project, true);
        let json = serde_json::to_value(&view).unwrap_or_default();
        assert_eq!(json["refusal"], serde_json::Value::Null);
    }

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
