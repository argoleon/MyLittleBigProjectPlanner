//! Sprints and the backlog of Scrum boards (specification §6, §12, L09-US01).
//!
//! A sprint belongs to one Scrum board. The members who see the board's node
//! see its backlog: the open sprints holding tickets of its filter (its own,
//! and those of other boards, read-only here) and the ranked backlog of the
//! tickets in no open sprint, finished ones and sub-tasks left out
//! (`domain::sprints::plan`). Those with `manage_sprints` on the node
//! (tenant admins always) create, change and delete its future sprints,
//! move tickets between the backlog and its sprints, and rank them.
//!
//! A ticket is in one open sprint at most. Ranks are kept per board and
//! order Kanban boards too (`POST /boards/{id}/rank`). Every move records an
//! outbox event on the ticket, so that the boards showing it follow.
//!
//! A board has one active sprint at most (L09-US02). Starting a sprint takes
//! its dates and snapshots the committed scope; closing it snapshots the
//! completed scope and sends the unfinished tickets to the backlog, a future
//! sprint of the board or a new one. The stays of a closed sprint are kept:
//! its scope can be read afterwards (`GET /sprints/{id}/scope`).

use std::collections::{BTreeMap, HashMap};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::boards::{Estimation, Kind, MAX_CARDS, effective_query};
use domain::permissions::Permission;
use domain::rank::sort_by_rank;
use domain::sprints::{
    self as rules, CloseRefusal, CloseTarget, Item, MoveRefusal, SprintFields, SprintRef, StartRefusal,
    State as SprintState, Stay, Target, Totals,
};
use domain::validation::FieldError;
use infra::boards::Board;
use infra::full_text::Audience;
use infra::outbox::Event;
use infra::search::Context;
use infra::sprints::{self as repo, Estimated, Place, Sprint, SprintInput, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};

use crate::boards::{CardView, EstimationView, build_cards, visible};
use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::issues::{Content, visible_issue};
use crate::nodes::{Sight, is_tenant_admin};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::search::{query_problem, search_problem};
use crate::tenant::{AppState, TenantTx};

pub const TAG: &str = "sprints";

/// Outbox event: a ticket joined or left a sprint.
pub const PLANNED: &str = "issue.planned";
/// Outbox event: a ticket moved in a board's order.
pub const RANKED: &str = "issue.ranked";

/// Where a sprint stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SprintState)]
pub enum StateView {
    /// Not started yet.
    Future,
    Active,
    Closed,
}

impl From<SprintState> for StateView {
    fn from(state: SprintState) -> Self {
        match state {
            SprintState::Future => Self::Future,
            SprintState::Active => Self::Active,
            SprintState::Closed => Self::Closed,
        }
    }
}

/// A sprint.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Sprint)]
pub struct SprintView {
    id: Uuid,
    /// The Scrum board it belongs to.
    board_id: Uuid,
    #[schema(example = "Platform board")]
    board_name: String,
    #[schema(example = "PLAT Sprint 25")]
    name: String,
    #[schema(example = "Audit and security of sign-ins")]
    goal: Option<String>,
    /// First day, a local date.
    #[schema(value_type = Option<String>, format = Date, example = "2026-11-16")]
    start_date: Option<Date>,
    /// Last day, included.
    #[schema(value_type = Option<String>, format = Date, example = "2026-11-27")]
    end_date: Option<Date>,
    state: StateView,
    /// When it closed.
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    closed_at: Option<OffsetDateTime>,
    /// The scope it committed to when it started: tickets, and their
    /// estimate in the board's estimation.
    #[schema(example = 4)]
    committed_count: Option<i32>,
    #[schema(example = 34.0)]
    committed_estimate: Option<f64>,
    /// The scope done when it closed.
    #[schema(example = 3)]
    completed_count: Option<i32>,
    #[schema(example = 21.0)]
    completed_estimate: Option<f64>,
    #[schema(example = 1)]
    version: i64,
}

pub(crate) fn sprint_view(sprint: Sprint) -> SprintView {
    SprintView {
        state: SprintState::parse(&sprint.state).unwrap_or_default().into(),
        closed_at: sprint.closed_at,
        committed_count: sprint.committed_count,
        committed_estimate: sprint.committed_estimate,
        completed_count: sprint.completed_count,
        completed_estimate: sprint.completed_estimate,
        id: sprint.id,
        board_id: sprint.board_id,
        board_name: sprint.board_name,
        name: sprint.name,
        goal: sprint.goal,
        start_date: sprint.start_date,
        end_date: sprint.end_date,
        version: sprint.version,
    }
}

/// The count and estimate of a list, in the board's estimation.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BacklogTotals)]
pub struct TotalsView {
    #[schema(example = 4)]
    count: usize,
    /// The sum of the estimates given.
    #[schema(example = 12.0)]
    estimate: f64,
    /// The part of `estimate` whose tickets are done.
    #[schema(example = 3.0)]
    done: f64,
    /// Tickets without an estimate.
    #[schema(example = 1)]
    unestimated: usize,
}

impl From<Totals> for TotalsView {
    fn from(totals: Totals) -> Self {
        Self {
            count: totals.count,
            estimate: totals.estimate,
            done: totals.done,
            unestimated: totals.unestimated,
        }
    }
}

/// A ticket of the backlog or of a sprint.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BacklogItem)]
pub struct ItemView {
    card: CardView,
    /// Its sub-tasks that the filter holds: they follow it.
    subtasks: usize,
}

/// An open sprint, with its tickets.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BacklogSprint)]
pub struct BacklogSprintView {
    sprint: SprintView,
    /// It belongs to this board: its planners change it. The sprints of
    /// other boards holding tickets of this one are shown read-only.
    own: bool,
    items: Vec<ItemView>,
    totals: TotalsView,
}

/// The tickets in no open sprint.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BacklogList)]
pub struct BacklogListView {
    items: Vec<ItemView>,
    totals: TotalsView,
}

/// The backlog of a Scrum board.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Backlog)]
pub struct BacklogView {
    estimation: EstimationView,
    /// The member may create sprints and move or rank tickets:
    /// `manage_sprints` on the board's node, which is not archived.
    can_plan: bool,
    /// This board's open sprints (the active one first, then by number),
    /// then those of other boards holding its tickets.
    sprints: Vec<BacklogSprintView>,
    backlog: BacklogListView,
    /// The filter holds more than 1,000 tickets: only the first are shown.
    truncated: bool,
}

/// A sprint to write.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SprintFields, example = json!({"name": "PLAT Sprint 25", "goal": "Audit and security of sign-ins", "startDate": "2026-11-16", "endDate": "2026-11-27"}))]
pub struct SprintBody {
    /// 1 to 100 characters. Left out at creation: "<node key> Sprint <n>".
    #[serde(default)]
    name: Option<String>,
    /// Up to 1,000 characters.
    #[serde(default)]
    goal: Option<String>,
    /// Both dates or none.
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Date)]
    start_date: Option<Date>,
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Date)]
    end_date: Option<Date>,
}

/// A ticket moved in the backlog view.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BacklogMove, example = json!({"issueId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a", "sprintId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9b", "afterId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9c"}))]
pub struct MoveBody {
    issue_id: Uuid,
    /// The sprint of this board it goes to; `null` for the backlog.
    #[serde(default)]
    sprint_id: Option<Uuid>,
    /// The ticket it now follows in the list.
    #[serde(default)]
    after_id: Option<Uuid>,
    /// The ticket it now precedes, when it follows none (top of the list).
    /// Neither: the end of the board's order.
    #[serde(default)]
    before_id: Option<Uuid>,
}

/// A ticket moved in a board's order.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BoardRank, example = json!({"issueId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a", "beforeId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9c"}))]
pub struct RankBody {
    issue_id: Uuid,
    /// The ticket it now follows.
    #[serde(default)]
    after_id: Option<Uuid>,
    /// The ticket it now precedes, when it follows none.
    #[serde(default)]
    before_id: Option<Uuid>,
}

/// Whether the member may plan on the boards of `node`.
pub(crate) async fn plans(state: &AppState, tx: &mut TenantTx, node: Uuid) -> Result<bool, Problem> {
    if is_tenant_admin(tx) {
        return Ok(true);
    }
    Ok(crate::permissions::effective(state, tx, Some(node))
        .await?
        .contains(Permission::ManageSprints))
}

/// `manage_sprints` on the board's node, which takes changes.
async fn require_plan(state: &AppState, tx: &mut TenantTx, board: &Board) -> Result<(), Problem> {
    if !plans(state, tx, board.node_id).await? {
        return Err(
            Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: manage_sprints")
        );
    }
    if board.node_archived {
        return Err(Problem::new(ErrorCode::NodeArchived));
    }
    Ok(())
}

fn require_scrum(board: &Board) -> Result<(), Problem> {
    if Kind::parse(&board.kind) == Some(Kind::Scrum) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::BoardNotScrum))
    }
}

/// The sprint `id` and its board, if the member sees the board's node.
async fn visible_sprint(tx: &mut TenantTx, sight: &Sight, id: Uuid) -> Result<(Sprint, Board), Problem> {
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail("no such sprint");
    let sprint = repo::find(tx, id).await?.ok_or_else(not_found)?;
    let board = visible(tx, sight, sprint.board_id)
        .await
        .map_err(|_| not_found())?;
    Ok((sprint, board))
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response.headers_mut().insert(header::ETAG, etag(version));
    response
}

/// Checks `body`; a name left out takes `default_name`.
fn checked(body: &SprintBody, default_name: Option<String>) -> Result<(String, Option<String>), Problem> {
    let name = body
        .name
        .clone()
        .or(default_name)
        .ok_or_else(|| Problem::validation(vec![FieldError::new("name", "required")]))?;
    let goal = body
        .goal
        .as_ref()
        .map(|g| g.trim().to_owned())
        .filter(|g| !g.is_empty());
    let local = |date: Option<Date>| date.and_then(infra::capacity::local_date);
    rules::validate(&SprintFields {
        name: name.clone(),
        goal: goal.clone(),
        start: local(body.start_date),
        end: local(body.end_date),
    })
    .map_err(Problem::validation)?;
    Ok((name.trim().to_owned(), goal))
}

/// The tickets of `board`'s filter that the member sees, at most 1,000, in
/// the board's order; whether more were left out.
async fn board_items(
    tx: &mut TenantTx,
    sight: &Sight,
    board: &Board,
) -> Result<(Vec<infra::issues::Issue>, bool), Problem> {
    let filter = domain::query::parse(&board.filter).map_err(|e| query_problem(&e))?;
    let query = effective_query(&filter, &[]);
    let account = tx.account_id();
    let visible_projects = sight.visible();
    let time_zone = infra::search::time_zone_of(tx, account).await?;
    let context = Context {
        account_id: account,
        time_zone: &time_zone,
        visible_projects: visible_projects.as_deref(),
        audience: Audience::Members,
    };
    let page = infra::search::search(tx, &query, &context, None, MAX_CARDS, false)
        .await
        .map_err(search_problem)?;
    let truncated = page.next_cursor.is_some();
    let mut items = page.items;
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let ranks = repo::ranks_of(tx, board.id, &ids).await?;
    sort_by_rank(&mut items, |i| ranks.get(&i.id).map(String::as_str));
    Ok((items, truncated))
}

/// `GET /api/v1/boards/{id}/backlog`.
#[utoipa::path(
    get,
    path = "/boards/{id}/backlog",
    tag = TAG,
    operation_id = "getBacklog",
    summary = "The backlog of a Scrum board",
    description = "Members who see its node. The tickets of its filter that the member may see (at \
                   most 1,000), in the board's rank: those in an open sprint under it (this board's \
                   open sprints always show, empty or not; those of other boards only when they hold \
                   its tickets), the others in the backlog, finished ones left out. Sub-tasks follow \
                   their parent and only count on its row. Totals are in the board's estimation.",
    params(
        ("id" = Uuid, Path, description = "Id of the board"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The backlog", body = BacklogView),
        (status = 400, description = "`query.invalid` when the filter no longer runs", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.not_scrum`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn backlog(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_scrum(&board)?;
    let can_plan = plans(&state, &mut tx, board.node_id).await? && !board.node_archived;
    let estimation = Estimation::parse(&board.estimation).unwrap_or_default();
    let (items, truncated) = board_items(&mut tx, &sight, &board).await?;
    let language = Language::of(&tx, &accept);
    let visible_projects = sight.visible();
    let built = build_cards(&mut tx, items, estimation, visible_projects.as_deref(), &language).await?;
    let ids: Vec<Uuid> = built.cards.iter().map(|c| c.card.id()).collect();
    let in_sprint = repo::open_sprint_of(&mut tx, &ids).await?;
    let own = repo::open_of_board(&mut tx, board.id).await?;
    let mut foreign_ids: Vec<Uuid> = in_sprint
        .values()
        .filter(|s| !own.iter().any(|o| o.id == **s))
        .copied()
        .collect();
    foreign_ids.sort_unstable();
    foreign_ids.dedup();
    let foreign = repo::find_many(&mut tx, &foreign_ids).await?;
    tx.commit().await?;

    let plan = rules::plan(
        &built
            .cards
            .iter()
            .map(|c| Item {
                id: c.card.id(),
                parent: c.parent_id,
                subtask: c.subtask,
                done: c.done,
                estimate: c.card.estimate(),
                sprint: in_sprint.get(&c.card.id()).copied(),
            })
            .collect::<Vec<_>>(),
    );
    let mut cards: HashMap<Uuid, CardView> = built.cards.into_iter().map(|c| (c.card.id(), c.card)).collect();
    let mut take = |ids: &[Uuid]| -> Vec<ItemView> {
        ids.iter()
            .filter_map(|id| {
                cards.remove(id).map(|card| ItemView {
                    card,
                    subtasks: plan.subtasks.get(id).copied().unwrap_or(0),
                })
            })
            .collect()
    };
    let mut sprints = Vec::with_capacity(own.len() + foreign.len());
    for (sprint, is_own) in own
        .into_iter()
        .map(|s| (s, true))
        .chain(foreign.into_iter().map(|s| (s, false)))
    {
        let listed = plan.sprints.get(&sprint.id).map_or(&[][..], Vec::as_slice);
        let totals = plan.sprint_totals.get(&sprint.id).copied().unwrap_or_default();
        sprints.push(BacklogSprintView {
            items: take(listed),
            totals: totals.into(),
            own: is_own,
            sprint: sprint_view(sprint),
        });
    }
    let backlog = BacklogListView {
        items: take(&plan.backlog),
        totals: plan.backlog_totals.into(),
    };
    let mut response = ApiJson(BacklogView {
        estimation: estimation.into(),
        can_plan,
        sprints,
        backlog,
        truncated,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/boards/{id}/sprints`.
#[utoipa::path(
    post,
    path = "/boards/{id}/sprints",
    tag = TAG,
    operation_id = "createSprint",
    summary = "Create a sprint",
    description = "Members with `manage_sprints` on the board's node (tenant admins always). The \
                   sprint is future, after the board's other sprints; a name left out is \
                   \"<node key> Sprint <n>\".",
    params(("id" = Uuid, Path, description = "Id of the Scrum board")),
    request_body = SprintBody,
    responses(
        (status = 201, description = "Created", body = SprintView, headers(
            ("ETag" = String, description = "Version of the sprint"),
            ("Location" = String, description = "URL of the sprint"),
        )),
        (status = 400, description = "`validation_failed` (`name`, `goal`, `startDate` `required`, \
                                      `endDate` `required` or `before_start`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.not_scrum`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SprintBody>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_scrum(&board)?;
    require_plan(&state, &mut tx, &board).await?;
    repo::lock_board(&mut tx, board.id).await?;
    let sequence = repo::next_sequence(&mut tx, board.id).await?;
    let (name, goal) = checked(&body, Some(rules::default_name(&board.node_key, sequence)))?;
    let author = tx.account_id();
    let sprint = repo::insert(
        &mut tx,
        infra::id::new_id(),
        board.id,
        sequence,
        author,
        &SprintInput {
            name: &name,
            goal: goal.as_deref(),
            start_date: body.start_date,
            end_date: body.end_date,
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = %author, sprint = %sprint.id, board = %board.id, "sprint created");
    tx.commit().await?;
    let (sprint_id, version) = (sprint.id, sprint.version);
    let mut response = (StatusCode::CREATED, ApiJson(sprint_view(sprint))).into_response();
    if let Ok(location) = format!("/api/v1/sprints/{sprint_id}").parse() {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(with_etag(response, version))
}

/// `GET /api/v1/sprints/{id}`.
#[utoipa::path(
    get,
    path = "/sprints/{id}",
    tag = TAG,
    operation_id = "getSprint",
    summary = "Read a sprint",
    description = "Members who see its board's node.",
    params(
        ("id" = Uuid, Path, description = "Id of the sprint"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The sprint", body = SprintView, headers(
            ("ETag" = String, description = "Version of the sprint"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such sprint visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let (sprint, _) = visible_sprint(&mut tx, &sight, id).await?;
    tx.commit().await?;
    let version = sprint.version;
    if if_none_match(&headers, version) {
        return Ok(with_etag(StatusCode::NOT_MODIFIED.into_response(), version));
    }
    Ok(with_etag(ApiJson(sprint_view(sprint)).into_response(), version))
}

/// `PUT /api/v1/sprints/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/sprints/{id}",
    tag = TAG,
    operation_id = "updateSprint",
    summary = "Change a sprint",
    description = "Members with `manage_sprints` on its board's node: its name, goal and dates, all \
                   replaced. Not once closed.",
    params(
        ("id" = Uuid, Path, description = "Id of the sprint"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = SprintBody,
    responses(
        (status = 200, description = "The sprint", body = SprintView, headers(
            ("ETag" = String, description = "New version of the sprint"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such sprint visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`sprint.closed`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the sprint changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SprintBody>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let (sprint, board) = visible_sprint(&mut tx, &sight, id).await?;
    require_plan(&state, &mut tx, &board).await?;
    if !SprintState::parse(&sprint.state).unwrap_or_default().is_open() {
        return Err(Problem::new(ErrorCode::SprintClosed));
    }
    let if_match = require_if_match(&headers)?;
    let (name, goal) = checked(&body, None)?;
    let sprint = repo::update(
        &mut tx,
        id,
        if_match.versions(),
        &SprintInput {
            name: &name,
            goal: goal.as_deref(),
            start_date: body.start_date,
            end_date: body.end_date,
        },
    )
    .await?
    .map_err(conflict_problem)?;
    let actor = tx.account_id();
    tracing::info!(target: "audit", actor = %actor, sprint = %id, "sprint changed");
    tx.commit().await?;
    let version = sprint.version;
    Ok(with_etag(ApiJson(sprint_view(sprint)).into_response(), version))
}

/// `DELETE /api/v1/sprints/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/sprints/{id}",
    tag = TAG,
    operation_id = "deleteSprint",
    summary = "Delete a sprint",
    description = "Members with `manage_sprints` on its board's node, while it is future: its \
                   tickets go back to the backlog.",
    params(
        ("id" = Uuid, Path, description = "Id of the sprint"),
        ("If-Match" = String, Header, description = "ETag of the sprint, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such sprint visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`sprint.not_future`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the sprint changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
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
    let (sprint, board) = visible_sprint(&mut tx, &sight, id).await?;
    require_plan(&state, &mut tx, &board).await?;
    if SprintState::parse(&sprint.state) != Some(SprintState::Future) {
        return Err(Problem::new(ErrorCode::SprintNotFuture));
    }
    let if_match = require_if_match(&headers)?;
    let issues: Vec<Uuid> = repo::history(&mut tx, id)
        .await?
        .into_iter()
        .filter(|s| s.removed_at.is_none())
        .map(|s| s.issue_id)
        .collect();
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    let actor = tx.account_id();
    for issue in issues {
        record(
            &mut tx,
            PLANNED,
            issue,
            json!({"boardId": board.id, "sprintId": null}),
            actor,
        )
        .await?;
    }
    tracing::info!(target: "audit", actor = %actor, sprint = %id, "sprint deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
    }
}

async fn record(
    tx: &mut TenantTx,
    kind: &str,
    issue: Uuid,
    payload: serde_json::Value,
    actor: Uuid,
) -> Result<(), Problem> {
    infra::outbox::record(
        tx,
        &Event {
            kind,
            aggregate_type: "issue",
            aggregate_id: issue,
            payload,
            actor_id: Some(actor),
        },
    )
    .await?;
    Ok(())
}

/// Ranks `issue` on `board` after `after` (or before `before`): the
/// neighbours without a rank yet first get the place they have in the
/// board's order. A neighbour that is not a ticket of the board is refused.
async fn rank(
    tx: &mut TenantTx,
    sight: &Sight,
    board: &Board,
    issue: Uuid,
    after: Option<Uuid>,
    before: Option<Uuid>,
) -> Result<(), Problem> {
    for (field, neighbour) in [("afterId", after), ("beforeId", before)] {
        if neighbour == Some(issue) {
            return Err(Problem::validation(vec![FieldError::new(field, "same_issue")]));
        }
    }
    let neighbours: Vec<Uuid> = after.into_iter().chain(before).collect();
    let ranked = repo::ranks_of(tx, board.id, &neighbours).await?;
    if neighbours.iter().any(|n| !ranked.contains_key(n)) {
        let (items, _) = board_items(tx, sight, board).await?;
        let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
        let known = repo::ranks_of(tx, board.id, &ids).await?;
        let unranked: Vec<Uuid> = ids
            .into_iter()
            .filter(|id| !known.contains_key(id) && *id != issue)
            .collect();
        repo::rank_last(tx, board.id, &unranked).await?;
    }
    let place = Place {
        after,
        before: if after.is_some() { None } else { before },
    };
    if repo::rank_at(tx, board.id, issue, place).await? {
        Ok(())
    } else {
        let field = if after.is_some() { "afterId" } else { "beforeId" };
        Err(Problem::validation(vec![FieldError::new(field, "not_found")]))
    }
}

/// `POST /api/v1/boards/{id}/backlog/moves`.
#[utoipa::path(
    post,
    path = "/boards/{id}/backlog/moves",
    tag = TAG,
    operation_id = "moveBacklogIssue",
    summary = "Move a ticket between the backlog and the sprints",
    description = "Members with `manage_sprints` on the board's node. Puts the ticket in a sprint of \
                   this board, open, or back in the backlog (out of the open sprint it was in), and \
                   ranks it after `afterId` (or before `beforeId`). A sub-task follows its parent; \
                   a ticket planned by another board stays there.",
    params(("id" = Uuid, Path, description = "Id of the Scrum board")),
    request_body = MoveBody,
    responses(
        (status = 204, description = "Moved"),
        (status = 400, description = "`validation_failed`: `sprintId` `not_found`, `afterId` or \
                                      `beforeId` `not_found` (not a ticket of the board) or \
                                      `same_issue`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board or ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.not_scrum`, `sprint.subtask`, `sprint.foreign`, \
                                      `sprint.closed`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn move_issue(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<MoveBody>,
) -> Result<StatusCode, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_scrum(&board)?;
    require_plan(&state, &mut tx, &board).await?;
    let (issue, project) = visible_issue(&state, &mut tx, &body.issue_id.to_string()).await?;
    repo::lock_board(&mut tx, board.id).await?;

    let content = Content::of(&mut tx, project.id).await?;
    let subtask = content
        .types
        .iter()
        .find(|t| t.item_id == issue.type_id)
        .is_some_and(|t| t.is_subtask);
    let current = repo::open_sprint_of(&mut tx, &[issue.id])
        .await?
        .remove(&issue.id);
    let mut refs: BTreeMap<Uuid, Sprint> = BTreeMap::new();
    for sprint in repo::find_many(
        &mut tx,
        &current.into_iter().chain(body.sprint_id).collect::<Vec<_>>(),
    )
    .await?
    {
        refs.insert(sprint.id, sprint);
    }
    let sprint_ref = |sprint: &Sprint| SprintRef {
        id: sprint.id,
        board: sprint.board_id,
        state: SprintState::parse(&sprint.state).unwrap_or_default(),
    };
    let from = current.and_then(|c| refs.get(&c)).map(sprint_ref);
    let target = match body.sprint_id {
        None => Target::Backlog,
        Some(sprint) => Target::Sprint(
            // Only this board's sprints take tickets from its backlog.
            refs.get(&sprint)
                .filter(|s| s.board_id == board.id)
                .map(sprint_ref)
                .ok_or_else(|| Problem::validation(vec![FieldError::new("sprintId", "not_found")]))?,
        ),
    };
    rules::check_move(board.id, subtask, from, target).map_err(|refusal| {
        Problem::new(match refusal {
            MoveRefusal::Subtask => ErrorCode::SprintSubtask,
            MoveRefusal::Foreign => ErrorCode::SprintForeign,
            MoveRefusal::Closed => ErrorCode::SprintClosed,
        })
    })?;

    rank(&mut tx, &sight, &board, issue.id, body.after_id, body.before_id).await?;
    let actor = tx.account_id();
    let moved = match target {
        Target::Backlog => repo::take_out(&mut tx, issue.id, actor).await?,
        Target::Sprint(sprint) => repo::put(&mut tx, issue.id, sprint.id, actor).await?,
    };
    let (kind, sprint) = if moved {
        (PLANNED, body.sprint_id)
    } else {
        (RANKED, current)
    };
    record(
        &mut tx,
        kind,
        issue.id,
        json!({"boardId": board.id, "sprintId": sprint}),
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/boards/{id}/rank`.
#[utoipa::path(
    post,
    path = "/boards/{id}/rank",
    tag = TAG,
    operation_id = "rankBoardIssue",
    summary = "Move a ticket in a board's order",
    description = "Members with `manage_sprints` on the board's node, on a Kanban or Scrum board. \
                   Ranks the ticket after `afterId` (or before `beforeId`; neither: last). The \
                   board's other tickets keep their order.",
    params(("id" = Uuid, Path, description = "Id of the board")),
    request_body = RankBody,
    responses(
        (status = 204, description = "Ranked"),
        (status = 400, description = "`validation_failed`: `afterId` or `beforeId` `not_found` (not a \
                                      ticket of the board) or `same_issue`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board or ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn rank_issue(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<RankBody>,
) -> Result<StatusCode, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_plan(&state, &mut tx, &board).await?;
    let (issue, _) = visible_issue(&state, &mut tx, &body.issue_id.to_string()).await?;
    repo::lock_board(&mut tx, board.id).await?;
    rank(&mut tx, &sight, &board, issue.id, body.after_id, body.before_id).await?;
    let actor = tx.account_id();
    record(&mut tx, RANKED, issue.id, json!({"boardId": board.id}), actor).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// What a sprint starts with.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SprintStart, example = json!({"name": "PLAT Sprint 25", "goal": "Audit of sign-ins", "startDate": "2026-11-30", "endDate": "2026-12-11"}))]
pub struct StartBody {
    /// 1 to 100 characters; its current name when left out.
    #[serde(default)]
    name: Option<String>,
    /// Up to 1,000 characters; its current goal when left out.
    #[serde(default)]
    goal: Option<String>,
    #[schema(value_type = String, format = Date)]
    start_date: Option<Date>,
    #[schema(value_type = String, format = Date)]
    end_date: Option<Date>,
}

/// Where the unfinished tickets of a closing sprint go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SprintCloseTarget)]
pub enum MoveToView {
    Backlog,
    /// The future sprint `sprintId` of the board.
    Sprint,
    /// A sprint created for them, after the board's others.
    NewSprint,
}

/// How a sprint closes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SprintClose, example = json!({"moveTo": "sprint", "sprintId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9b"}))]
pub struct CloseBody {
    move_to: MoveToView,
    /// With `moveTo: sprint`.
    #[serde(default)]
    sprint_id: Option<Uuid>,
}

/// What closing a sprint did.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SprintClosed)]
pub struct ClosedView {
    sprint: SprintView,
    /// The tickets moved out, unfinished.
    #[schema(example = 3)]
    moved: usize,
    /// Where they went: a sprint of the board, or `null` for the backlog.
    target: Option<SprintView>,
}

/// `?state=` of a board's sprints.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct SprintListParams {
    /// `open` (future and active, the active one first) or `closed` (the
    /// last closed first); all of them when left out.
    pub state: Option<String>,
}

/// The sprints of a board.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SprintList)]
pub struct SprintListView {
    items: Vec<SprintView>,
}

/// A stay of a ticket in a sprint.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SprintScopeItem)]
pub struct ScopeItemView {
    issue_id: Uuid,
    #[schema(example = "PLAT-142")]
    key: String,
    summary: String,
    /// In the sprint when it started.
    committed: bool,
    /// Added once the sprint had started.
    added_later: bool,
    /// Taken out before the sprint closed.
    removed: bool,
    /// Done when the sprint closed; `null` while it runs, or when it left before.
    completed: Option<bool>,
    estimate_at_start: Option<f64>,
    estimate_at_close: Option<f64>,
}

/// The scope of a sprint over its life.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SprintScope)]
pub struct ScopeView {
    sprint: SprintView,
    /// Every stay of the tickets the member sees, by arrival.
    items: Vec<ScopeItemView>,
    /// Stays of tickets the member does not see, left out of `items`.
    hidden: usize,
}

/// The estimate in `estimation`, and whether its status is done, of each of
/// `issues` (those of a project without a configuration are left out).
async fn estimated(
    tx: &mut TenantTx,
    issues: &[Uuid],
    estimation: Estimation,
) -> Result<Vec<Estimated>, Problem> {
    let found = infra::issues::find_many(tx, issues).await?;
    let projects: Vec<Uuid> = found.iter().map(|i| i.project_id).collect();
    let configs: HashMap<Uuid, Option<Uuid>> = infra::search::projects_of(tx, &projects)
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
    let mut out = Vec::with_capacity(found.len());
    for issue in found {
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
        let done = content
            .workflows
            .statuses
            .iter()
            .find(|s| s.item_id == issue.status_id)
            .is_some_and(|s| {
                domain::workflows::Category::parse(&s.category) == Some(domain::workflows::Category::Done)
            });
        out.push(Estimated {
            issue: issue.id,
            estimate,
            done,
        });
    }
    Ok(out)
}

fn stays_of(estimated: &[Estimated]) -> Vec<Stay<Uuid>> {
    estimated
        .iter()
        .map(|e| Stay {
            issue: e.issue,
            done: e.done,
            estimate: e.estimate,
        })
        .collect()
}

fn count_of(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// `POST /api/v1/sprints/{id}/start`: requires `If-Match`.
#[utoipa::path(
    post,
    path = "/sprints/{id}/start",
    tag = TAG,
    operation_id = "startSprint",
    summary = "Start a sprint",
    description = "Members with `manage_sprints` on its board's node. The sprint must be future and \
                   the board without an active sprint; it takes its dates (both required), and its \
                   current tickets become its committed scope, with their estimate.",
    params(
        ("id" = Uuid, Path, description = "Id of the sprint"),
        ("If-Match" = String, Header, description = "ETag of the sprint, or `*`"),
    ),
    request_body = StartBody,
    responses(
        (status = 200, description = "The sprint, active", body = SprintView, headers(
            ("ETag" = String, description = "New version of the sprint"),
        )),
        (status = 400, description = "`validation_failed` (`name`, `goal`, `startDate` and `endDate` \
                                      `required`, `endDate` `before_start`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such sprint visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`sprint.not_future`, `sprint.active_exists`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the sprint changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn start(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<StartBody>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let (sprint, board) = visible_sprint(&mut tx, &sight, id).await?;
    require_plan(&state, &mut tx, &board).await?;
    let if_match = require_if_match(&headers)?;
    repo::lock_board(&mut tx, board.id).await?;
    let active = repo::active_of_board(&mut tx, board.id).await?;
    rules::check_start(
        SprintState::parse(&sprint.state).unwrap_or_default(),
        active.is_some(),
    )
    .map_err(|refusal| {
        Problem::new(match refusal {
            StartRefusal::NotFuture => ErrorCode::SprintNotFuture,
            StartRefusal::ActiveExists => ErrorCode::SprintActiveExists,
        })
    })?;
    let name = body.name.clone().unwrap_or_else(|| sprint.name.clone());
    let goal = body
        .goal
        .clone()
        .or_else(|| sprint.goal.clone())
        .map(|g| g.trim().to_owned())
        .filter(|g| !g.is_empty());
    let local = |date: Option<Date>| date.and_then(infra::capacity::local_date);
    rules::validate_start(&SprintFields {
        name: name.clone(),
        goal: goal.clone(),
        start: local(body.start_date),
        end: local(body.end_date),
    })
    .map_err(Problem::validation)?;
    let estimation = Estimation::parse(&board.estimation).unwrap_or_default();
    let issues = repo::issues_in(&mut tx, id).await?;
    let committed = estimated(&mut tx, &issues, estimation).await?;
    let snapshot = rules::snapshot(&stays_of(&committed));
    let sprint = repo::start(
        &mut tx,
        id,
        if_match.versions(),
        &SprintInput {
            name: name.trim(),
            goal: goal.as_deref(),
            start_date: body.start_date,
            end_date: body.end_date,
        },
        &committed,
        (count_of(snapshot.count), snapshot.estimate),
    )
    .await?
    .map_err(conflict_problem)?;
    let actor = tx.account_id();
    tracing::info!(target: "audit", actor = %actor, sprint = %id, board = %board.id, "sprint started");
    tx.commit().await?;
    let version = sprint.version;
    Ok(with_etag(ApiJson(sprint_view(sprint)).into_response(), version))
}

/// `POST /api/v1/sprints/{id}/close`: requires `If-Match`.
#[utoipa::path(
    post,
    path = "/sprints/{id}/close",
    tag = TAG,
    operation_id = "closeSprint",
    summary = "Close a sprint",
    description = "Members with `manage_sprints` on its board's node, while it is active. Its \
                   tickets whose status is of the `done` category are its completed scope, kept \
                   with their estimate; the others go to the backlog, to a future sprint of the \
                   board, or to a new sprint after the board's others (\"<node key> Sprint <n>\"). \
                   Sub-tasks follow their parent.",
    params(
        ("id" = Uuid, Path, description = "Id of the sprint"),
        ("If-Match" = String, Header, description = "ETag of the sprint, or `*`"),
    ),
    request_body = CloseBody,
    responses(
        (status = 200, description = "The sprint, closed", body = ClosedView),
        (status = 400, description = "`validation_failed`: `sprintId` `required` or `not_found` (not a \
                                      future sprint of the board)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such sprint visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`sprint.not_active`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the sprint changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn close(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CloseBody>,
) -> Result<Response, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let (sprint, board) = visible_sprint(&mut tx, &sight, id).await?;
    require_plan(&state, &mut tx, &board).await?;
    let if_match = require_if_match(&headers)?;
    repo::lock_board(&mut tx, board.id).await?;
    let chosen = match (body.move_to, body.sprint_id) {
        (MoveToView::Sprint, None) => {
            return Err(Problem::validation(vec![FieldError::new("sprintId", "required")]));
        }
        (MoveToView::Sprint, Some(target)) => Some(
            repo::find(&mut tx, target)
                .await?
                .ok_or_else(|| Problem::validation(vec![FieldError::new("sprintId", "not_found")]))?,
        ),
        _ => None,
    };
    let target = match &chosen {
        Some(chosen) => CloseTarget::Sprint(SprintRef {
            id: chosen.id,
            board: chosen.board_id,
            state: SprintState::parse(&chosen.state).unwrap_or_default(),
        }),
        None if body.move_to == MoveToView::NewSprint => CloseTarget::NewSprint,
        None => CloseTarget::Backlog,
    };
    rules::check_close(
        board.id,
        SprintState::parse(&sprint.state).unwrap_or_default(),
        target,
    )
    .map_err(|refusal| match refusal {
        CloseRefusal::NotActive => Problem::new(ErrorCode::SprintNotActive),
        CloseRefusal::BadTarget => Problem::validation(vec![FieldError::new("sprintId", "not_found")]),
    })?;

    let estimation = Estimation::parse(&board.estimation).unwrap_or_default();
    let issues = repo::issues_in(&mut tx, id).await?;
    let stays = estimated(&mut tx, &issues, estimation).await?;
    let closing = rules::close(&stays_of(&stays));
    let closed = repo::close(
        &mut tx,
        id,
        if_match.versions(),
        &stays,
        (count_of(closing.snapshot.count), closing.snapshot.estimate),
    )
    .await?
    .map_err(conflict_problem)?;
    let actor = tx.account_id();
    let target = match target {
        CloseTarget::Backlog => None,
        CloseTarget::Sprint(_) => chosen,
        CloseTarget::NewSprint => {
            let sequence = repo::next_sequence(&mut tx, board.id).await?;
            let name = rules::default_name(&board.node_key, sequence);
            let input = SprintInput {
                name: &name,
                goal: None,
                start_date: None,
                end_date: None,
            };
            Some(repo::insert(&mut tx, infra::id::new_id(), board.id, sequence, actor, &input).await?)
        }
    };
    for issue in &closing.unfinished {
        if let Some(target) = &target {
            repo::put(&mut tx, *issue, target.id, actor).await?;
        }
        record(
            &mut tx,
            PLANNED,
            *issue,
            json!({"boardId": board.id, "sprintId": target.as_ref().map(|t| t.id)}),
            actor,
        )
        .await?;
    }
    tracing::info!(target: "audit", actor = %actor, sprint = %id, board = %board.id, moved = closing.unfinished.len(), "sprint closed");
    tx.commit().await?;
    Ok(ApiJson(ClosedView {
        sprint: sprint_view(closed),
        moved: closing.unfinished.len(),
        target: target.map(sprint_view),
    })
    .into_response())
}

/// `GET /api/v1/boards/{id}/sprints`.
#[utoipa::path(
    get,
    path = "/boards/{id}/sprints",
    tag = TAG,
    operation_id = "listSprints",
    summary = "The sprints of a Scrum board",
    description = "Members who see its node: its open sprints (the active one first, then by number), \
                   then its closed ones (the last closed first), with their committed and completed \
                   scope.",
    params(("id" = Uuid, Path, description = "Id of the board"), SprintListParams),
    responses(
        (status = 200, description = "The sprints", body = SprintListView),
        (status = 400, description = "`validation_failed`: `state` `invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such board visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`board.not_scrum`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<SprintListParams>,
) -> Result<ApiJson<SprintListView>, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let board = visible(&mut tx, &sight, id).await?;
    require_scrum(&board)?;
    let (open, closed) = match params.state.as_deref() {
        None => (true, true),
        Some("open") => (true, false),
        Some("closed") => (false, true),
        Some(_) => return Err(Problem::validation(vec![FieldError::new("state", "invalid")])),
    };
    let mut items = Vec::new();
    if open {
        items.extend(repo::open_of_board(&mut tx, board.id).await?);
    }
    if closed {
        items.extend(repo::closed_of_board(&mut tx, board.id).await?);
    }
    tx.commit().await?;
    Ok(ApiJson(SprintListView {
        items: items.into_iter().map(sprint_view).collect(),
    }))
}

/// `GET /api/v1/sprints/{id}/scope`.
#[utoipa::path(
    get,
    path = "/sprints/{id}/scope",
    tag = TAG,
    operation_id = "getSprintScope",
    summary = "The scope of a sprint over its life",
    description = "Members who see its board's node: every stay of a ticket in the sprint, whether it \
                   was committed at the start, added later or taken out, and whether it was \
                   completed at the close, with its estimates then. Tickets the member does not see \
                   are only counted.",
    params(("id" = Uuid, Path, description = "Id of the sprint")),
    responses(
        (status = 200, description = "The scope", body = ScopeView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such sprint visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn scope(mut tx: TenantTx, ApiPath(id): ApiPath<Uuid>) -> Result<ApiJson<ScopeView>, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let (sprint, _) = visible_sprint(&mut tx, &sight, id).await?;
    let rows = repo::scope(&mut tx, id).await?;
    tx.commit().await?;
    let started = sprint.started_at;
    let (seen, hidden): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| sight.sees(r.project_id));
    Ok(ApiJson(ScopeView {
        items: seen
            .into_iter()
            .map(|r| ScopeItemView {
                added_later: started.is_some_and(|s| r.added_at > s) && !r.committed,
                removed: r.removed_at.is_some(),
                issue_id: r.issue_id,
                key: r.issue_key,
                summary: r.issue_summary,
                committed: r.committed,
                completed: r.completed,
                estimate_at_start: r.estimate_at_start,
                estimate_at_close: r.estimate_at_close,
            })
            .collect(),
        hidden: hidden.len(),
        sprint: sprint_view(sprint),
    }))
}
