//! Boards (specification §6, §12): Kanban (L06-US05) and Scrum (L09-US01).
//!
//! A board belongs to a node: the members who see the node see its board,
//! those with `manage_boards` there (tenant admins always) create, change
//! and delete it. Its cards are the tickets of its filter (and of the quick
//! filters toggled on) that the member may see, in the column of their
//! status, grouped in swimlanes by assignee or epic.
//!
//! Dropping a card in a column runs the transition of its workflow leading
//! there, with its conditions, validators and post-functions
//! (`issue_transitions::perform`). A transition with fields to fill first
//! answers `screen`, several leading there answer `choose`: the client then
//! runs the one picked through `POST /issues/{idOrKey}/transitions`, or
//! moves again with its `transitionId`.
//!
//! Cards follow the board's manual rank, the unranked ones last in the
//! filter's order; a Scrum board plans its tickets in sprints
//! (`crate::sprints`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::boards::{
    BoardFields, Candidate, ColumnFields, Drop, Estimation, Kind, MAX_CARDS, QuickFilterFields, StatusRef,
    Swimlanes, Wip, default_columns, default_quick_filters, effective_query, lanes, pick_transition,
    validate, wip,
};
use domain::issues::Priority;
use domain::permissions::Permission;
use domain::rank::sort_by_rank;
use domain::transitions::allowed;
use domain::validation::FieldError;
use domain::workflows::Category;
use infra::boards::{self as repo, Board, BoardInput, ColumnInput, QuickFilter, WriteConflict};
use infra::full_text::Audience;
use infra::search::Context;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::issue_changes::{locked, require_edit};
use crate::issue_transitions::{Run, TransitionView, actor, leaving, perform, transition_view};
use crate::issues::{
    Content, IssueRefView, IssueView, PersonView, PriorityView, StatusView, TypeView, issue_view, person,
    visible_issue,
};
use crate::nodes::{Sight, ensure_changeable, is_tenant_admin};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::search::{query_problem, search_problem};
use crate::tenant::{AppState, TenantTx};

pub const TAG: &str = "boards";

const PATH: &str = "/api/v1/boards";

/// How the cards of a board are grouped in rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BoardSwimlanes)]
pub enum SwimlanesView {
    #[default]
    None,
    Assignee,
    Epic,
}

impl From<SwimlanesView> for Swimlanes {
    fn from(view: SwimlanesView) -> Self {
        match view {
            SwimlanesView::None => Self::None,
            SwimlanesView::Assignee => Self::Assignee,
            SwimlanesView::Epic => Self::Epic,
        }
    }
}

impl From<Swimlanes> for SwimlanesView {
    fn from(swimlanes: Swimlanes) -> Self {
        match swimlanes {
            Swimlanes::None => Self::None,
            Swimlanes::Assignee => Self::Assignee,
            Swimlanes::Epic => Self::Epic,
        }
    }
}

/// What a board is for: the flow of its tickets, or planning them in
/// sprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BoardKind)]
pub enum KindView {
    #[default]
    Kanban,
    Scrum,
}

impl From<KindView> for Kind {
    fn from(view: KindView) -> Self {
        match view {
            KindView::Kanban => Self::Kanban,
            KindView::Scrum => Self::Scrum,
        }
    }
}

impl From<Kind> for KindView {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Kanban => Self::Kanban,
            Kind::Scrum => Self::Scrum,
        }
    }
}

/// What cards are estimated in: story points, or remaining work in hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BoardEstimation)]
pub enum EstimationView {
    #[default]
    Points,
    Hours,
}

impl From<EstimationView> for Estimation {
    fn from(view: EstimationView) -> Self {
        match view {
            EstimationView::Points => Self::Points,
            EstimationView::Hours => Self::Hours,
        }
    }
}

impl From<Estimation> for EstimationView {
    fn from(estimation: Estimation) -> Self {
        match estimation {
            Estimation::Points => Self::Points,
            Estimation::Hours => Self::Hours,
        }
    }
}

/// The node a board belongs to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardNode)]
pub struct NodeView {
    id: Uuid,
    #[schema(example = "PLAT")]
    key: String,
    #[schema(example = "Identity platform")]
    name: String,
    /// `portfolio`, `program` or `project`.
    #[schema(example = "project")]
    node_type: String,
    /// Archived: the board shows, nothing changes.
    archived: bool,
}

/// A column of a board.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardColumn)]
pub struct ColumnView {
    id: Uuid,
    /// 1 to 40 characters.
    #[schema(example = "In progress")]
    name: String,
    /// Keys of the statuses it shows; a status in one column at most.
    #[schema(example = json!(["in_progress"]))]
    statuses: Vec<String>,
    /// Most cards before the column shows over its limit (1 to 999);
    /// `null`: no limit.
    #[schema(example = 4)]
    wip_limit: Option<u32>,
}

/// A quick filter of a board.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BoardQuickFilter)]
pub struct QuickFilterView {
    /// 1 to 40 characters.
    #[schema(example = "My tickets")]
    label: String,
    /// A query of the language without `ORDER BY`, joined to the board's
    /// filter with `AND` when toggled on.
    #[schema(example = "assignee = currentUser()")]
    query: String,
}

/// A board.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Board)]
pub struct BoardView {
    id: Uuid,
    node: NodeView,
    #[schema(example = "Platform board")]
    name: String,
    /// Set at creation.
    kind: KindView,
    /// Its query: the tickets it shows, and their order.
    #[schema(example = "project = PLAT AND status != cancelled")]
    filter: String,
    estimation: EstimationView,
    /// How cards are grouped when the board opens.
    swimlanes: SwimlanesView,
    quick_filters: Vec<QuickFilterView>,
    columns: Vec<ColumnView>,
    /// The member may change or delete it: `manage_boards` on its node.
    can_manage: bool,
    /// The member may plan its sprints and rank its tickets:
    /// `manage_sprints` on its node, which is not archived.
    can_plan: bool,
    #[schema(example = 1)]
    version: i64,
}

/// The boards a member sees.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardList {
    /// By node name, then board name.
    items: Vec<BoardView>,
}

/// A board to write.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BoardFields, example = json!({
    "name": "Platform board",
    "filter": "project = PLAT",
    "estimation": "points",
    "swimlanes": "none",
    "columns": [
        {"name": "To do", "statuses": ["todo"], "wipLimit": null},
        {"name": "In progress", "statuses": ["in_progress"], "wipLimit": 4},
        {"name": "Done", "statuses": ["done"], "wipLimit": null}
    ],
    "quickFilters": [{"label": "My tickets", "query": "assignee = currentUser()"}]
}))]
pub struct BoardBody {
    /// 1 to 100 characters, unique on its node whatever the case.
    name: String,
    /// A query of the language, checked as `POST /search/parse` does.
    filter: String,
    #[serde(default)]
    estimation: EstimationView,
    #[serde(default)]
    swimlanes: SwimlanesView,
    /// 1 to 12. Left out at creation: one column per status category, each
    /// with the statuses of the projects of its node.
    #[serde(default)]
    columns: Option<Vec<ColumnBody>>,
    /// Up to 10. Left out at creation: "My tickets" and "Bugs".
    #[serde(default)]
    quick_filters: Option<Vec<QuickFilterView>>,
}

/// A column to write.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BoardColumnFields)]
pub struct ColumnBody {
    name: String,
    #[serde(default)]
    statuses: Vec<String>,
    #[serde(default)]
    wip_limit: Option<u32>,
}

/// A board to create.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NewBoardFields, example = json!({"nodeId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a", "name": "Platform board", "filter": "project = PLAT"}))]
pub struct NewBoardBody {
    /// The node it belongs to.
    node_id: Uuid,
    name: String,
    /// Kanban when left out; it cannot change afterwards.
    #[serde(default)]
    kind: KindView,
    filter: String,
    #[serde(default)]
    estimation: EstimationView,
    #[serde(default)]
    swimlanes: SwimlanesView,
    #[serde(default)]
    columns: Option<Vec<ColumnBody>>,
    #[serde(default)]
    quick_filters: Option<Vec<QuickFilterView>>,
}

impl NewBoardBody {
    fn fields(&self) -> BoardBody {
        BoardBody {
            name: self.name.clone(),
            filter: self.filter.clone(),
            estimation: self.estimation,
            swimlanes: self.swimlanes,
            columns: self.columns.clone(),
            quick_filters: self.quick_filters.clone(),
        }
    }
}

/// `?nodeId=`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    /// Only the boards of this node.
    pub node_id: Option<Uuid>,
}

/// `?nodeId=` of the statuses columns may show.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct StatusParams {
    /// Only those of the projects of this node and below; all the projects
    /// the member sees when left out.
    pub node_id: Option<Uuid>,
}

/// A status columns may show.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardStatus)]
pub struct StatusOptionView {
    #[schema(example = "in_progress")]
    key: String,
    #[schema(example = "In progress")]
    label: Option<String>,
    category: crate::workflows::CategoryView,
}

/// `?quick=&swimlanes=` of a board's cards.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CardParams {
    /// The quick filters toggled on, by position from 0, comma-separated
    /// (`0,2`).
    pub quick: Option<String>,
    /// How to group the cards; the board's setting when left out.
    pub swimlanes: Option<SwimlanesView>,
}

/// Where a column stands against its work in progress limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BoardWip)]
pub enum WipView {
    /// No limit.
    Unlimited,
    Under,
    /// As many cards as the limit.
    Reached,
    /// More cards than the limit.
    Exceeded,
}

impl From<Wip> for WipView {
    fn from(wip: Wip) -> Self {
        match wip {
            Wip::Unlimited => Self::Unlimited,
            Wip::Under => Self::Under,
            Wip::Reached => Self::Reached,
            Wip::Exceeded => Self::Exceeded,
        }
    }
}

/// A column with its cards' count.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardColumnCards)]
pub struct ColumnCardsView {
    id: Uuid,
    name: String,
    statuses: Vec<String>,
    wip_limit: Option<u32>,
    /// Its cards, every swimlane included.
    count: usize,
    wip: WipView,
}

/// A card: a ticket on the board.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardCard)]
pub struct CardView {
    id: Uuid,
    #[schema(example = "PLAT-142")]
    key: String,
    summary: String,
    project_id: Uuid,
    #[serde(rename = "type")]
    issue_type: TypeView,
    status: StatusView,
    priority: PriorityView,
    assignee: Option<PersonView>,
    /// In the board's estimation: story points, or remaining hours.
    estimate: Option<f64>,
    /// The nearest epic above it, if the member sees it.
    epic: Option<IssueRefView>,
    /// Keys of the unfinished tickets blocking it.
    blocked_by: Vec<String>,
    /// For `If-Match` when it moves.
    version: i64,
}

impl CardView {
    pub(crate) const fn id(&self) -> Uuid {
        self.id
    }

    pub(crate) const fn estimate(&self) -> Option<f64> {
        self.estimate
    }
}

/// A swimlane: a row of the board, one cell per column.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardLane)]
pub struct LaneView {
    /// The assignee or the epic; `null` for the row without one (and the
    /// single row without swimlanes).
    id: Option<Uuid>,
    /// The epic's key.
    key: Option<String>,
    /// The assignee's name or the epic's summary.
    label: Option<String>,
    /// The cards of each column, in the board's order.
    cells: Vec<Vec<CardView>>,
}

/// The cards of a board.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardCards)]
pub struct BoardCardsView {
    columns: Vec<ColumnCardsView>,
    swimlanes: SwimlanesView,
    lanes: Vec<LaneView>,
    /// Cards on the board.
    total: usize,
    /// Tickets of the filter whose status no column shows.
    unmapped: usize,
    /// The filter holds more than 1,000 tickets: only the first are shown.
    truncated: bool,
    /// A Scrum board shows the tickets of its active sprint only (and their
    /// sub-tasks): that sprint, `null` when none is active.
    sprint: Option<crate::sprints::SprintView>,
}

/// A card dropped in a column.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BoardMove, example = json!({"issueId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a", "columnId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9b"}))]
pub struct MoveBody {
    issue_id: Uuid,
    column_id: Uuid,
    /// The transition picked after `choose`.
    #[serde(default)]
    transition_id: Option<Uuid>,
}

/// What a drop did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BoardMoveOutcome)]
pub enum MoveOutcome {
    /// The transition ran: the ticket is in its new status.
    Moved,
    /// Its status is already one of the column's.
    Unchanged,
    /// The transition has fields to fill first (`transitions` holds it):
    /// run it through `POST /issues/{idOrKey}/transitions`.
    Screen,
    /// Several transitions lead to the column (`transitions`): move again
    /// with the `transitionId` picked.
    Choose,
}

/// The result of a drop.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BoardMoveResult)]
pub struct MoveView {
    outcome: MoveOutcome,
    /// The ticket, moved or not.
    issue: IssueView,
    /// For `screen` and `choose`.
    transitions: Vec<TransitionView>,
}

fn node_view(board: &Board) -> NodeView {
    NodeView {
        id: board.node_id,
        key: board.node_key.clone(),
        name: board.node_name.clone(),
        node_type: board.node_type.clone(),
        archived: board.node_archived,
    }
}

fn board_view(board: Board, can_manage: bool, can_plan: bool) -> BoardView {
    BoardView {
        can_plan: can_plan && !board.node_archived,
        id: board.id,
        node: node_view(&board),
        kind: Kind::parse(&board.kind).unwrap_or_default().into(),
        filter: board.filter,
        estimation: Estimation::parse(&board.estimation).unwrap_or_default().into(),
        swimlanes: Swimlanes::parse(&board.swimlanes).unwrap_or_default().into(),
        quick_filters: board
            .quick_filters
            .into_iter()
            .map(|q| QuickFilterView {
                label: q.label,
                query: q.query,
            })
            .collect(),
        columns: board
            .columns
            .into_iter()
            .map(|c| ColumnView {
                id: c.id,
                name: c.name,
                statuses: c.statuses,
                wip_limit: c.wip_limit,
            })
            .collect(),
        name: board.name,
        can_manage,
        version: board.version,
    }
}

/// Whether the member may manage the boards of `node`.
async fn manages(state: &AppState, tx: &mut TenantTx, node: Uuid) -> Result<bool, Problem> {
    if is_tenant_admin(tx) {
        return Ok(true);
    }
    Ok(crate::permissions::effective(state, tx, Some(node))
        .await?
        .contains(Permission::ManageBoards))
}

async fn require_manage(state: &AppState, tx: &mut TenantTx, node: Uuid) -> Result<(), Problem> {
    if manages(state, tx, node).await? {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: manage_boards"))
    }
}

/// The board `id`, if the member sees its node.
pub(crate) async fn visible(tx: &mut TenantTx, sight: &Sight, id: Uuid) -> Result<Board, Problem> {
    repo::find(tx, id)
        .await?
        .filter(|b| sight.sees(b.node_id))
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such board"))
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::NameTaken => {
            Problem::new(ErrorCode::BoardNameTaken).with_errors(vec![FieldError::new("name", "taken")])
        }
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

/// A board as checked: what is stored.
struct Checked {
    name: String,
    kind: Kind,
    filter: String,
    estimation: Estimation,
    swimlanes: Swimlanes,
    columns: Vec<ColumnFields>,
    quick_filters: Vec<QuickFilter>,
}

impl Checked {
    fn input(&self) -> (Vec<ColumnInput<'_>>, BoardInput<'_>) {
        let columns = self
            .columns
            .iter()
            .map(|c| ColumnInput {
                name: &c.name,
                statuses: &c.statuses,
                wip_limit: c.wip_limit,
            })
            .collect();
        (
            columns,
            BoardInput {
                name: &self.name,
                kind: self.kind.as_str(),
                filter: &self.filter,
                estimation: self.estimation.as_str(),
                swimlanes: self.swimlanes.as_str(),
                quick_filters: &self.quick_filters,
                columns: &[],
            },
        )
    }
}

/// Checks `body`, its filter included, for a board of `kind`; columns and
/// quick filters left out take `defaults`.
fn checked(
    body: &BoardBody,
    kind: Kind,
    defaults: impl FnOnce() -> (Vec<ColumnFields>, Vec<QuickFilterFields>),
) -> Result<Checked, Problem> {
    let (columns, quick) = match (&body.columns, &body.quick_filters) {
        (Some(columns), Some(quick)) => (columns_of(columns), quick_of(quick)),
        (columns, quick) => {
            let (default_columns, default_quick) = defaults();
            (
                columns.as_ref().map_or(default_columns, |c| columns_of(c)),
                quick.as_ref().map_or(default_quick, |q| quick_of(q)),
            )
        }
    };
    let fields = BoardFields {
        name: body.name.clone(),
        columns,
        quick_filters: quick,
    };
    validate(&fields).map_err(Problem::validation)?;
    domain::query::parse(&body.filter).map_err(|e| query_problem(&e))?;
    Ok(Checked {
        name: fields.name.trim().to_owned(),
        kind,
        filter: body.filter.trim().to_owned(),
        estimation: body.estimation.into(),
        swimlanes: body.swimlanes.into(),
        columns: fields
            .columns
            .into_iter()
            .map(|c| ColumnFields {
                name: c.name.trim().to_owned(),
                ..c
            })
            .collect(),
        quick_filters: fields
            .quick_filters
            .into_iter()
            .map(|q| QuickFilter {
                label: q.label.trim().to_owned(),
                query: q.query.trim().to_owned(),
            })
            .collect(),
    })
}

fn columns_of(columns: &[ColumnBody]) -> Vec<ColumnFields> {
    columns
        .iter()
        .map(|c| ColumnFields {
            name: c.name.clone(),
            statuses: c.statuses.clone(),
            wip_limit: c.wip_limit,
        })
        .collect()
}

fn quick_of(quick: &[QuickFilterView]) -> Vec<QuickFilterFields> {
    quick
        .iter()
        .map(|q| QuickFilterFields {
            label: q.label.clone(),
            query: q.query.clone(),
        })
        .collect()
}

/// The projects of `node` and below that the member sees (`None`: every
/// project, for a tenant admin on no node).
async fn projects_below(
    tx: &mut TenantTx,
    sight: &Sight,
    node: Option<Uuid>,
) -> Result<Option<Vec<Uuid>>, Problem> {
    let Some(node) = node else {
        return Ok(sight.visible());
    };
    let below = infra::boards::projects_below(tx, node).await?;
    Ok(Some(below.into_iter().filter(|p| sight.sees(*p)).collect()))
}

/// `GET /api/v1/boards`.
#[utoipa::path(
    get,
    path = "/boards",
    tag = TAG,
    operation_id = "listBoards",
    summary = "List the boards the member sees",
    description = "The boards of the nodes the member sees, or of one node, by node name then board \
                   name. A tenant holds a few hundred at most: no pages.",
    params(ListParams),
    responses(
        (status = 200, description = "The boards", body = BoardList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<ApiJson<BoardList>, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let visible = sight.visible();
    let boards = repo::list(&mut tx, visible.as_deref(), params.node_id).await?;
    let mut rights: HashMap<Uuid, (bool, bool)> = HashMap::new();
    let mut items = Vec::with_capacity(boards.len());
    for board in boards {
        let (can_manage, can_plan) = match rights.get(&board.node_id) {
            Some(can) => *can,
            None => {
                let can = (
                    manages(&state, &mut tx, board.node_id).await?,
                    crate::sprints::plans(&state, &mut tx, board.node_id).await?,
                );
                rights.insert(board.node_id, can);
                can
            }
        };
        items.push(board_view(board, can_manage, can_plan));
    }
    tx.commit().await?;
    Ok(ApiJson(BoardList { items }))
}

/// `GET /api/v1/boards/statuses`.
#[utoipa::path(
    get,
    path = "/boards/statuses",
    tag = TAG,
    operation_id = "listBoardStatuses",
    summary = "The statuses board columns may show",
    description = "One per key, from the published configurations of the projects the member sees \
                   (of a node and below with `nodeId`), with the label and category of the first \
                   workflow using the key.",
    params(StatusParams, ("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    responses(
        (status = 200, description = "The statuses, by key", body = Vec<StatusOptionView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn statuses(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<StatusParams>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let projects = projects_below(&mut tx, &sight, params.node_id).await?;
    let statuses = repo::statuses(&mut tx, projects.as_deref()).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let views: Vec<StatusOptionView> = statuses
        .into_iter()
        .map(|s| StatusOptionView {
            label: language.label(&serde_json::from_value(s.label).unwrap_or_default()),
            category: crate::workflows::CategoryView::of(&s.category),
            key: s.key,
        })
        .collect();
    let mut response = ApiJson(views).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/boards`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/boards",
    tag = TAG,
    operation_id = "createBoard",
    summary = "Create a board",
    description = "Members with `manage_boards` on the node (tenant admins always). Columns left \
                   out: one per status category, with the statuses of the node's projects; quick \
                   filters left out: \"My tickets\" and \"Bugs\".",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the default column names"),
    ),
    request_body = NewBoardBody,
    responses(
        (status = 201, description = "Created", body = BoardView, headers(
            ("ETag" = String, description = "Version of the board"),
            ("Location" = String, description = "URL of the board"),
        )),
        (status = 400, description = "`validation_failed` (`name`, `columns`, `columns.{i}.name`, \
                                      `columns.{i}.statuses` `duplicate`, `columns.{i}.wipLimit`, \
                                      `quickFilters.{i}.label`, `quickFilters.{i}.query` `invalid` or \
                                      `order_by_not_allowed`), or `query.invalid` for the filter", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.name_taken`, `node.archived`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiJson(body): ApiJson<NewBoardBody>,
) -> Result<Response, Problem> {
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let sight = Sight::of(&mut tx).await?;
    let node = infra::nodes::find(&mut tx, body.node_id)
        .await?
        .filter(|n| sight.sees(n.id))
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such node"))?;
    require_manage(&state, &mut tx, node.id).await?;
    ensure_changeable(&node)?;
    let language = Language::of(&tx, &accept);
    let fields = body.fields();
    let defaults = if fields.columns.is_none() {
        let projects = projects_below(&mut tx, &sight, Some(node.id)).await?;
        repo::statuses(&mut tx, projects.as_deref()).await?
    } else {
        Vec::new()
    };
    let locale = language.locale();
    let checked = checked(&fields, body.kind.into(), || {
        let refs: Vec<StatusRef> = defaults
            .iter()
            .filter_map(|s| {
                Some(StatusRef {
                    key: s.key.clone(),
                    category: Category::parse(&s.category)?,
                })
            })
            .collect();
        (default_columns(&refs, locale), default_quick_filters(locale))
    })?;
    let (columns, mut input) = checked.input();
    input.columns = &columns;
    let author = tx.account_id();
    let board = repo::insert(&mut tx, infra::id::new_id(), node.id, author, &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %author, board = %board.id, node = %node.id, "board created");
    let (id, version) = (board.id, board.version);
    let can_plan = crate::sprints::plans(&state, &mut tx, node.id).await?;
    let view = board_view(board, true, can_plan);
    let response =
        idempotency::created(&mut tx, key.as_deref(), &view, format!("{PATH}/{id}"), version).await?;
    tx.commit().await?;
    Ok(response)
}

/// `GET /api/v1/boards/{id}`.
#[utoipa::path(
    get,
    path = "/boards/{id}",
    tag = TAG,
    operation_id = "getBoard",
    summary = "Read a board",
    description = "Members who see its node.",
    params(
        ("id" = Uuid, Path, description = "Id of the board"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The board", body = BoardView, headers(
            ("ETag" = String, description = "Version of the board"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    let can_manage = manages(&state, &mut tx, board.node_id).await?;
    let can_plan = crate::sprints::plans(&state, &mut tx, board.node_id).await?;
    tx.commit().await?;
    let version = board.version;
    if if_none_match(&headers, version) {
        return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), version));
    }
    Ok(with_etag(
        ApiJson(board_view(board, can_manage, can_plan)).into_response(),
        version,
    ))
}

/// `PUT /api/v1/boards/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/boards/{id}",
    tag = TAG,
    operation_id = "updateBoard",
    summary = "Change a board",
    description = "Members with `manage_boards` on its node: its name, filter, estimation, \
                   swimlanes, columns and quick filters, all replaced (columns and quick filters \
                   left out stay as they are).",
    params(
        ("id" = Uuid, Path, description = "Id of the board"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = BoardBody,
    responses(
        (status = 200, description = "The board", body = BoardView, headers(
            ("ETag" = String, description = "New version of the board"),
        )),
        (status = 400, description = "`validation_failed`, or `query.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.name_taken`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the board changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<BoardBody>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_manage(&state, &mut tx, board.node_id).await?;
    if board.node_archived {
        return Err(Problem::new(ErrorCode::NodeArchived));
    }
    let if_match = require_if_match(&headers)?;
    let current = board.clone();
    let kind = Kind::parse(&board.kind).unwrap_or_default();
    let checked = checked(&body, kind, move || {
        (
            current
                .columns
                .into_iter()
                .map(|c| ColumnFields {
                    name: c.name,
                    statuses: c.statuses,
                    wip_limit: c.wip_limit,
                })
                .collect(),
            current
                .quick_filters
                .into_iter()
                .map(|q| QuickFilterFields {
                    label: q.label,
                    query: q.query,
                })
                .collect(),
        )
    })?;
    let (columns, mut input) = checked.input();
    input.columns = &columns;
    let board = repo::update(&mut tx, id, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    let actor = tx.account_id();
    tracing::info!(target: "audit", actor = %actor, board = %id, "board changed");
    let can_plan = crate::sprints::plans(&state, &mut tx, board.node_id).await?;
    let version = board.version;
    tx.commit().await?;
    Ok(with_etag(
        ApiJson(board_view(board, true, can_plan)).into_response(),
        version,
    ))
}

/// `DELETE /api/v1/boards/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/boards/{id}",
    tag = TAG,
    operation_id = "deleteBoard",
    summary = "Delete a board",
    description = "Members with `manage_boards` on its node. Its tickets stay as they are.",
    params(
        ("id" = Uuid, Path, description = "Id of the board"),
        ("If-Match" = String, Header, description = "ETag of the board, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the board changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_manage(&state, &mut tx, board.node_id).await?;
    let if_match = require_if_match(&headers)?;
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    let actor = tx.account_id();
    tracing::info!(target: "audit", actor = %actor, board = %id, "board deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The quick filters toggled on, from `0,2`.
fn quick_toggled(raw: Option<&str>, count: usize) -> Result<Vec<usize>, Problem> {
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let mut toggled = BTreeSet::new();
    for part in raw.split(',') {
        match part.trim().parse::<usize>() {
            Ok(i) if i < count => {
                toggled.insert(i);
            }
            _ => {
                return Err(Problem::validation(vec![FieldError::new(
                    "quick",
                    "out_of_range",
                )]));
            }
        }
    }
    Ok(toggled.into_iter().collect())
}

/// `GET /api/v1/boards/{id}/cards`.
#[utoipa::path(
    get,
    path = "/boards/{id}/cards",
    tag = TAG,
    operation_id = "getBoardCards",
    summary = "The cards of a board",
    description = "Members who see its node. The tickets of its filter (`AND` the quick filters \
                   toggled on) that the member may see, at most 1,000, in the board's rank, the \
                   unranked ones last in the order of the filter (priority then last change \
                   without `ORDER BY`), in the column of their status (on a Scrum board, those of \
                   its active sprint only, and their sub-tasks) \
                   and grouped in swimlanes. Each column tells its count against its work in \
                   progress limit.",
    params(
        ("id" = Uuid, Path, description = "Id of the board"),
        CardParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The cards", body = BoardCardsView),
        (status = 400, description = "`validation_failed`: `quick` (`out_of_range`); or `query.invalid` \
                                      when the filter no longer runs (a field deleted since)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn cards(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<CardParams>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    let toggled = quick_toggled(params.quick.as_deref(), board.quick_filters.len())?;
    let filter = domain::query::parse(&board.filter).map_err(|e| query_problem(&e))?;
    let mut quick = Vec::with_capacity(toggled.len());
    for i in toggled {
        quick.push(domain::query::parse(&board.quick_filters[i].query).map_err(|e| query_problem(&e))?);
    }
    let query = effective_query(&filter, &quick);
    let swimlanes: Swimlanes = params.swimlanes.map_or_else(
        || Swimlanes::parse(&board.swimlanes).unwrap_or_default(),
        Into::into,
    );
    let estimation = Estimation::parse(&board.estimation).unwrap_or_default();

    let account = tx.account_id();
    let visible_projects = sight.visible();
    let time_zone = infra::search::time_zone_of(&mut tx, account).await?;
    let context = Context {
        account_id: account,
        time_zone: &time_zone,
        visible_projects: visible_projects.as_deref(),
        audience: Audience::Members,
    };
    let page = infra::search::search(&mut tx, &query, &context, None, MAX_CARDS, false)
        .await
        .map_err(search_problem)?;
    let truncated = page.next_cursor.is_some();
    let mut items = page.items;
    // A Scrum board: its active sprint's tickets only, sub-tasks following their parent.
    let sprint = if Kind::parse(&board.kind) == Some(Kind::Scrum) {
        let active = infra::sprints::active_of_board(&mut tx, board.id).await?;
        let in_sprint: BTreeSet<Uuid> = match &active {
            Some(sprint) => infra::sprints::issues_in(&mut tx, sprint.id)
                .await?
                .into_iter()
                .collect(),
            None => BTreeSet::new(),
        };
        items.retain(|i| in_sprint.contains(&i.id) || i.parent_id.is_some_and(|p| in_sprint.contains(&p)));
        active
    } else {
        None
    };
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let ranks = infra::sprints::ranks_of(&mut tx, board.id, &ids).await?;
    sort_by_rank(&mut items, |i| ranks.get(&i.id).map(String::as_str));
    let language = Language::of(&tx, &accept);
    let built = build_cards(&mut tx, items, estimation, visible_projects.as_deref(), &language).await?;
    let epics = built.epics;
    tx.commit().await?;

    let columns: Vec<ColumnFields> = board
        .columns
        .iter()
        .map(|c| ColumnFields {
            name: c.name.clone(),
            statuses: c.statuses.clone(),
            wip_limit: c.wip_limit,
        })
        .collect();
    // Each card with its column, in the board's order.
    let mut placed: Vec<(usize, CardView)> = Vec::with_capacity(built.cards.len());
    let mut unmapped = 0;
    for built in built.cards {
        match domain::boards::column_of(&columns, built.card.status.key()) {
            Some(column) => placed.push((column, built.card)),
            None => unmapped += 1,
        }
    }
    let mut counts = vec![0_usize; columns.len()];
    for (column, _) in &placed {
        counts[*column] += 1;
    }
    let total = placed.len();
    let lanes = lanes_of(swimlanes, placed, &epics, columns.len());
    let views = board
        .columns
        .into_iter()
        .zip(counts)
        .map(|(c, count)| ColumnCardsView {
            wip: wip(count, c.wip_limit).into(),
            id: c.id,
            name: c.name,
            statuses: c.statuses,
            wip_limit: c.wip_limit,
            count,
        })
        .collect();
    let mut response = ApiJson(BoardCardsView {
        columns: views,
        swimlanes: swimlanes.into(),
        lanes,
        total,
        unmapped,
        truncated,
        sprint: sprint.map(crate::sprints::sprint_view),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// A card, with what the backlog needs besides.
#[derive(Debug)]
pub(crate) struct BuiltCard {
    pub(crate) card: CardView,
    /// Its status is of the `done` category.
    pub(crate) done: bool,
    /// Its type is a sub-task.
    pub(crate) subtask: bool,
    pub(crate) parent_id: Option<Uuid>,
}

/// The cards of a list of tickets, and the epic of each.
#[derive(Debug)]
pub(crate) struct BuiltCards {
    pub(crate) cards: Vec<BuiltCard>,
    pub(crate) epics: HashMap<Uuid, infra::boards::EpicOf>,
}

/// The cards of `items`, in their order, estimated in `estimation`; those of
/// a project without a configuration are left out.
pub(crate) async fn build_cards(
    tx: &mut TenantTx,
    items: Vec<infra::issues::Issue>,
    estimation: Estimation,
    visible_projects: Option<&[Uuid]>,
    language: &Language,
) -> Result<BuiltCards, Problem> {
    let project_ids: Vec<Uuid> = items
        .iter()
        .map(|i| i.project_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let configs: HashMap<Uuid, Option<Uuid>> = infra::search::projects_of(tx, &project_ids)
        .await?
        .into_iter()
        .map(|p| (p.id, p.config_id))
        .collect();
    let mut contents: BTreeMap<Uuid, Content> = BTreeMap::new();
    for config in configs.values().flatten() {
        if let std::collections::btree_map::Entry::Vacant(entry) = contents.entry(*config) {
            entry.insert(Content::of_config(tx, *config).await?);
        }
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let epics = repo::epics_of(tx, &ids, visible_projects).await?;
    let mut blockers = repo::blockers_of(tx, &ids, visible_projects).await?;
    let mut cards = Vec::with_capacity(items.len());
    for issue in items {
        let Some(content) = configs
            .get(&issue.project_id)
            .copied()
            .flatten()
            .and_then(|c| contents.get(&c))
        else {
            continue;
        };
        let estimate = content
            .fields
            .iter()
            .find(|f| f.key == estimation.field_key())
            .and_then(|f| issue.fields.get(f.item_id.to_string()))
            .and_then(serde_json::Value::as_f64);
        let epic = epics
            .get(&issue.id)
            .map(|e| IssueRefView::new(e.id, e.key.clone(), e.summary.clone()));
        let done = content
            .workflows
            .statuses
            .iter()
            .find(|s| s.item_id == issue.status_id)
            .is_some_and(|s| Category::parse(&s.category) == Some(Category::Done));
        let subtask = content
            .types
            .iter()
            .find(|t| t.item_id == issue.type_id)
            .is_some_and(|t| t.is_subtask);
        cards.push(BuiltCard {
            done,
            subtask,
            parent_id: issue.parent_id,
            card: CardView {
                issue_type: language.type_of(content, issue.type_id),
                status: language.status_view(content, issue.status_id),
                priority: Priority::parse(&issue.priority).unwrap_or_default().into(),
                assignee: person(issue.assignee_id, issue.assignee_name.as_ref()),
                estimate,
                epic,
                blocked_by: blockers.remove(&issue.id).unwrap_or_default(),
                version: issue.version,
                id: issue.id,
                key: issue.key,
                summary: issue.summary,
                project_id: issue.project_id,
            },
        });
    }
    Ok(BuiltCards { cards, epics })
}

/// What a lane is keyed by: the assignee's name, or the epic's project key
/// and number; the id last, to keep namesakes apart.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LaneKey {
    Assignee(String, Uuid),
    Epic(String, i32, Uuid),
}

fn lanes_of(
    swimlanes: Swimlanes,
    placed: Vec<(usize, CardView)>,
    epics: &HashMap<Uuid, infra::boards::EpicOf>,
    columns: usize,
) -> Vec<LaneView> {
    let keys: Vec<Option<LaneKey>> = placed
        .iter()
        .map(|(_, card)| match swimlanes {
            Swimlanes::None => None,
            Swimlanes::Assignee => card
                .assignee
                .as_ref()
                .map(|a| LaneKey::Assignee(a.name().to_lowercase(), a.id())),
            Swimlanes::Epic => epics.get(&card.id).map(|e| {
                let project = e.key.split('-').next().unwrap_or_default().to_owned();
                LaneKey::Epic(project, e.key_number, e.id)
            }),
        })
        .collect();
    let labels: HashMap<Uuid, (Option<String>, String)> = placed
        .iter()
        .filter_map(|(_, card)| match swimlanes {
            Swimlanes::None => None,
            Swimlanes::Assignee => card
                .assignee
                .as_ref()
                .map(|a| (a.id(), (None, a.name().to_owned()))),
            Swimlanes::Epic => epics
                .get(&card.id)
                .map(|e| (e.id, (Some(e.key.clone()), e.summary.clone()))),
        })
        .collect();
    let grouped = lanes(&keys);
    let mut cards: Vec<Option<(usize, CardView)>> = placed.into_iter().map(Some).collect();
    grouped
        .into_iter()
        .map(|(key, members)| {
            let mut cells: Vec<Vec<CardView>> = (0..columns).map(|_| Vec::new()).collect();
            for i in members {
                if let Some((column, card)) = cards[i].take() {
                    cells[column].push(card);
                }
            }
            let id = key.map(|k| match k {
                LaneKey::Assignee(_, id) | LaneKey::Epic(_, _, id) => id,
            });
            let (key, label) = id
                .and_then(|id| labels.get(&id).cloned())
                .map_or((None, None), |(key, label)| (key, Some(label)));
            LaneView {
                id,
                key,
                label,
                cells,
            }
        })
        .collect()
}

/// `POST /api/v1/boards/{id}/moves`: requires `If-Match` (the ticket's).
#[utoipa::path(
    post,
    path = "/boards/{id}/moves",
    tag = TAG,
    operation_id = "moveBoardCard",
    summary = "Drop a card in a column",
    description = "Members with `edit` on the ticket's project. Runs the transition of its workflow \
                   that leads from its status to a status of the column and whose condition the \
                   member meets, with its validators and post-functions, as \
                   `POST /issues/{idOrKey}/transitions` does. Answers `screen` when it has fields \
                   to fill first, `choose` when several lead there (move again with the \
                   `transitionId` picked).",
    params(
        ("id" = Uuid, Path, description = "Id of the board"),
        ("If-Match" = String, Header, description = "ETag of the ticket's version, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = MoveBody,
    responses(
        (status = 200, description = "What the drop did", body = MoveView),
        (status = 400, description = "`validation_failed`: `columnId` (`not_found`: not a column of \
                                      the board), `transitionId` (`not_found`: none leading from \
                                      its status to the column)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: no `edit`, or the transitions leading \
                                      there are restricted; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board or ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.no_transition`: no transition leads from its status to \
                                      the column; or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the ticket changed since this version", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn move_card(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<MoveBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    let column = board
        .columns
        .iter()
        .find(|c| c.id == body.column_id)
        .map(|c| ColumnFields {
            name: c.name.clone(),
            statuses: c.statuses.clone(),
            wip_limit: c.wip_limit,
        })
        .ok_or_else(|| Problem::validation(vec![FieldError::new("columnId", "not_found")]))?;
    let (issue, project) = visible_issue(&state, &mut tx, &body.issue_id.to_string()).await?;
    require_edit(&state, &mut tx, project.id).await?;
    ensure_changeable(&project)?;
    let issue = locked(&mut tx, issue.id, if_match.versions()).await?;
    let content = Content::of(&mut tx, project.id).await?;
    let (actor, admin) = actor(&state, &mut tx, project.id).await?;
    let language = Language::of(&tx, &accept);
    let leaving: Vec<_> = leaving(&content, &issue)
        .into_iter()
        .filter(|t| body.transition_id.is_none_or(|picked| picked == t.item_id))
        .collect();
    let candidates: Vec<Candidate> = leaving
        .iter()
        .map(|t| Candidate {
            to_status: language.status_view(&content, t.to_status_id).key().to_owned(),
            allowed: admin || allowed(&t.rules.condition, &actor),
        })
        .collect();
    let current = language.status_view(&content, issue.status_id);
    let (outcome, transitions, issue) = match pick_transition(current.key(), &column, &candidates) {
        Drop::AlreadyThere => (
            MoveOutcome::Unchanged,
            Vec::new(),
            issue_now(&state, &mut tx, &language, issue, &project).await?,
        ),
        Drop::NoTransition if body.transition_id.is_some() => {
            return Err(Problem::validation(vec![FieldError::new(
                "transitionId",
                "not_found",
            )]));
        }
        Drop::NoTransition => {
            return Err(Problem::new(ErrorCode::BoardNoTransition).with_detail(format!(
                "no transition leads from {} to the column {}",
                current.key(),
                column.name
            )));
        }
        Drop::Restricted => {
            return Err(Problem::new(ErrorCode::PermissionDenied).with_detail("the transition is restricted"));
        }
        Drop::Choose(picks) => {
            let views = picks
                .iter()
                .map(|i| transition_view(&language, &content, leaving[*i], &issue))
                .collect();
            (
                MoveOutcome::Choose,
                views,
                issue_now(&state, &mut tx, &language, issue, &project).await?,
            )
        }
        Drop::Run(i) => {
            let transition = leaving[i];
            let view = transition_view(&language, &content, transition, &issue);
            if view.immediate() {
                let run = Run {
                    project: project.id,
                    content: &content,
                    transition,
                    actor: &actor,
                    admin,
                };
                let moved = perform(
                    &state,
                    &mut tx,
                    &language,
                    &run,
                    &project,
                    issue,
                    &BTreeMap::new(),
                )
                .await?;
                (MoveOutcome::Moved, Vec::new(), moved)
            } else {
                (
                    MoveOutcome::Screen,
                    vec![view],
                    issue_now(&state, &mut tx, &language, issue, &project).await?,
                )
            }
        }
    };
    tx.commit().await?;
    let mut response = ApiJson(MoveView {
        outcome,
        issue,
        transitions,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

async fn issue_now(
    state: &AppState,
    tx: &mut TenantTx,
    language: &Language,
    issue: infra::issues::Issue,
    project: &infra::nodes::Node,
) -> Result<IssueView, Problem> {
    issue_view(state, tx, language, issue, project, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_filters_toggle_by_position() {
        assert_eq!(quick_toggled(None, 2).unwrap(), Vec::<usize>::new());
        assert_eq!(quick_toggled(Some(" "), 2).unwrap(), Vec::<usize>::new());
        assert_eq!(quick_toggled(Some("1,0,1"), 2).unwrap(), vec![0, 1]);
        for wrong in ["2", "a", "0,", "-1"] {
            let problem = quick_toggled(Some(wrong), 2).unwrap_err();
            assert_eq!(problem.code, ErrorCode::ValidationFailed, "{wrong}");
        }
    }
}
