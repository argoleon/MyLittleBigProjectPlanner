//! Integration tests of sprints and the Scrum backlog (L09-US01) on real
//! PostgreSQL and Valkey: the backlog lists the open sprints and the ranked
//! tickets left (finished ones and sub-tasks out), moves change a sprint's
//! scope and the rank, the query language finds tickets by sprint; a ticket
//! planned by another board shows there read-only; Kanban cards follow the
//! rank; who plans; validation, versions, deletion, closed sprints,
//! archived nodes and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

type Session = (String, String);

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

fn field_codes(body: &Value) -> Vec<(String, String)> {
    let mut codes: Vec<(String, String)> = body["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no errors in {body}"))
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    codes.sort();
    codes
}

fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("accept-language", "en")
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account_id, tenant.id)
    .await
    .unwrap();
    (account_id, (opened.token, opened.session.csrf_token))
}

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: Uuid, label: &str, account: Uuid) {
    let role: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
            .bind(tenant.id)
            .bind(label)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let path = format!("/api/v1/nodes/{node}/grants");
    let (status, granted) =
        send(admin(api, tenant, Method::POST, &path).json(&json!({"roleId": role, "accountId": account})))
            .await;
    assert_eq!(status, 201, "{granted}");
}

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, key: &str) -> Uuid {
    let body = json!({"type": "project", "name": name, "key": key});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().parse().unwrap()
}

/// A ticket `KEY-n` of `project`, written directly (the tickets' own API is
/// tested elsewhere).
#[derive(Clone, Copy)]
struct Ticket<'a> {
    project: Uuid,
    key: &'a str,
    number: i32,
    type_key: &'a str,
    status: &'a str,
    parent: Option<Uuid>,
    points: Option<f64>,
}

async fn ticket(db: &TestDb, tenant: &SeedTenant, t: Ticket<'_>) -> Uuid {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let id: Uuid = infra::sqlx::query_scalar(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority,
                           parent_id, fields)
        SELECT uuidv7(), app_current_tenant(), n.id, $2, $3 || '-' || $2,
               (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE t.key = $4),
               (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE s.key = $5),
               'Ticket ' || $3 || '-' || $2, 'medium', $6,
               CASE WHEN $7::float8 IS NULL THEN '{}'::jsonb ELSE jsonb_build_object(
                   (SELECT f.item_id::text FROM field_def f JOIN project_config_version v ON v.id = f.version_id
                    AND v.state = 'published' AND v.config_id = n.config_id WHERE f.key = 'story_points'), $7) END
        FROM node n WHERE n.id = $1
        RETURNING id
        "#,
    )
    .bind(t.project)
    .bind(t.number)
    .bind(t.key)
    .bind(t.type_key)
    .bind(t.status)
    .bind(t.parent)
    .bind(t.points)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

fn task(project: Uuid, number: i32, status: &str) -> Ticket<'_> {
    Ticket {
        project,
        key: "PLAT",
        number,
        type_key: "task",
        status,
        parent: None,
        points: None,
    }
}

async fn create_board(api: &TestApi, tenant: &SeedTenant, node: Uuid, name: &str, kind: &str) -> String {
    let body = json!({
        "nodeId": node,
        "name": name,
        "kind": kind,
        "filter": "project = PLAT ORDER BY key",
        "columns": [
            {"name": "To do", "statuses": ["todo"]},
            {"name": "In progress", "statuses": ["in_progress", "in_review"]},
            {"name": "Done", "statuses": ["done"]}
        ],
        "quickFilters": []
    });
    let (status, board) = send(admin(api, tenant, Method::POST, "/api/v1/boards").json(&body)).await;
    assert_eq!(status, 201, "{board}");
    assert_eq!(board["kind"], kind);
    board["id"].as_str().unwrap().to_owned()
}

async fn create_sprint(api: &TestApi, tenant: &SeedTenant, board: &str, body: Value) -> Value {
    let path = format!("/api/v1/boards/{board}/sprints");
    let (status, sprint) = send(admin(api, tenant, Method::POST, &path).json(&body)).await;
    assert_eq!(status, 201, "{sprint}");
    sprint
}

async fn plan(api: &TestApi, tenant: &SeedTenant, board: &str, body: Value) -> (u16, Value) {
    let path = format!("/api/v1/boards/{board}/backlog/moves");
    send(admin(api, tenant, Method::POST, &path).json(&body)).await
}

async fn planned(api: &TestApi, tenant: &SeedTenant, board: &str, body: Value) {
    let (status, moved) = plan(api, tenant, board, body).await;
    assert_eq!(status, 204, "{moved}");
}

async fn rank(api: &TestApi, tenant: &SeedTenant, board: &str, body: Value) -> (u16, Value) {
    let path = format!("/api/v1/boards/{board}/rank");
    send(admin(api, tenant, Method::POST, &path).json(&body)).await
}

async fn backlog(api: &TestApi, tenant: &SeedTenant, board: &str) -> Value {
    get(api, tenant, &format!("/api/v1/boards/{board}/backlog")).await
}

fn keys(items: &Value) -> Vec<&str> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["card"]["key"].as_str().unwrap())
        .collect()
}

/// Each sprint's name, whether it is the board's own, and its keys.
fn sprints_of(backlog: &Value) -> Vec<(&str, bool, Vec<&str>)> {
    backlog["sprints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["sprint"]["name"].as_str().unwrap(),
                s["own"].as_bool().unwrap(),
                keys(&s["items"]),
            )
        })
        .collect()
}

async fn events(db: &TestDb, tenant: &SeedTenant, kind: &str) -> Vec<Value> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let events: Vec<Value> =
        infra::sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
            .bind(kind)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    events
}

async fn search_keys(api: &TestApi, tenant: &SeedTenant, query: &str) -> Vec<String> {
    let (status, page) =
        send(admin(api, tenant, Method::POST, "/api/v1/search").json(&json!({"query": query}))).await;
    assert_eq!(status, 200, "{query}: {page}");
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect()
}

/// Starts the sprint `sprint` through the API, over two weeks.
async fn start_sprint(api: &TestApi, tenant: &SeedTenant, sprint: &str) -> Value {
    let path = format!("/api/v1/sprints/{sprint}/start");
    let body = json!({"startDate": "2026-11-30", "endDate": "2026-12-11"});
    let (status, started) = send(
        admin(api, tenant, Method::POST, &path)
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{started}");
    started
}

/// Closes the sprint `sprint` through the API, its unfinished tickets going as `body` says.
async fn close_sprint(api: &TestApi, tenant: &SeedTenant, sprint: &str, body: Value) -> Value {
    let path = format!("/api/v1/sprints/{sprint}/close");
    let (status, closed) = send(
        admin(api, tenant, Method::POST, &path)
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{closed}");
    closed
}

#[tokio::test]
async fn the_backlog_lists_open_sprints_and_ranked_tickets_and_moves_change_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let story = |number, points| Ticket {
        type_key: "story",
        points,
        ..task(plat, number, "todo")
    };
    let p1 = ticket(&db, t, story(1, Some(8.0))).await;
    let p2 = ticket(&db, t, story(2, Some(5.0))).await;
    ticket(
        &db,
        t,
        Ticket {
            points: Some(3.0),
            ..task(plat, 3, "done")
        },
    )
    .await;
    ticket(
        &db,
        t,
        Ticket {
            type_key: "subtask",
            parent: Some(p2),
            ..task(plat, 4, "todo")
        },
    )
    .await;
    let p5 = ticket(&db, t, task(plat, 5, "todo")).await;
    let p6 = ticket(
        &db,
        t,
        Ticket {
            points: Some(1.0),
            ..task(plat, 6, "in_progress")
        },
    )
    .await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;

    // A sprint without a name takes one from the node's key.
    let path = format!("/api/v1/boards/{board}/sprints");
    let response = admin(&api, t, Method::POST, &path)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert!(
        response.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with("/api/v1/sprints/")
    );
    assert_eq!(response.headers()["etag"], "\"1\"");
    let first: Value = response.json().await.unwrap();
    assert_eq!(
        (
            &first["name"],
            &first["state"],
            &first["goal"],
            &first["boardName"]
        ),
        (
            &json!("PLAT Sprint 1"),
            &json!("future"),
            &Value::Null,
            &json!("Platform Scrum")
        )
    );
    let s1 = first["id"].as_str().unwrap().to_owned();
    let second = create_sprint(
        &api,
        t,
        &board,
        json!({"name": "Sprint 25", "goal": " Audit of sign-ins ", "startDate": "2026-11-16", "endDate": "2026-11-27"}),
    )
    .await;
    assert_eq!(
        (&second["goal"], &second["startDate"], &second["endDate"]),
        (
            &json!("Audit of sign-ins"),
            &json!("2026-11-16"),
            &json!("2026-11-27")
        )
    );

    // Finished tickets and sub-tasks are not in the backlog.
    let read = backlog(&api, t, &board).await;
    assert_eq!(
        (&read["canPlan"], &read["estimation"], &read["truncated"]),
        (&json!(true), &json!("points"), &json!(false))
    );
    assert_eq!(
        sprints_of(&read),
        [("PLAT Sprint 1", true, vec![]), ("Sprint 25", true, vec![])]
    );
    assert_eq!(
        keys(&read["backlog"]["items"]),
        ["PLAT-1", "PLAT-2", "PLAT-5", "PLAT-6"]
    );
    assert_eq!(read["backlog"]["items"][1]["subtasks"], 1);
    assert_eq!(
        read["backlog"]["totals"],
        json!({"count": 4, "estimate": 14.0, "done": 0.0, "unestimated": 1})
    );
    assert_eq!(read["sprints"][0]["totals"]["count"], 0);

    // Into Sprint 1: its total follows.
    planned(&api, t, &board, json!({"issueId": p2, "sprintId": s1})).await;
    planned(
        &api,
        t,
        &board,
        json!({"issueId": p6, "sprintId": s1, "afterId": p2}),
    )
    .await;
    planned(
        &api,
        t,
        &board,
        json!({"issueId": p1, "sprintId": s1, "beforeId": p2}),
    )
    .await;
    let read = backlog(&api, t, &board).await;
    assert_eq!(
        sprints_of(&read)[0],
        ("PLAT Sprint 1", true, vec!["PLAT-1", "PLAT-2", "PLAT-6"])
    );
    assert_eq!(
        read["sprints"][0]["totals"],
        json!({"count": 3, "estimate": 14.0, "done": 0.0, "unestimated": 0})
    );
    assert_eq!(keys(&read["backlog"]["items"]), ["PLAT-5"]);

    // Back to the backlog, on top; then reranked there.
    planned(
        &api,
        t,
        &board,
        json!({"issueId": p2, "sprintId": null, "beforeId": p5}),
    )
    .await;
    let read = backlog(&api, t, &board).await;
    assert_eq!(sprints_of(&read)[0].2, ["PLAT-1", "PLAT-6"]);
    assert_eq!(keys(&read["backlog"]["items"]), ["PLAT-2", "PLAT-5"]);
    let (status, ranked) = rank(&api, t, &board, json!({"issueId": p5, "beforeId": p2})).await;
    assert_eq!(status, 204, "{ranked}");
    // Moving within the same sprint only reranks.
    planned(
        &api,
        t,
        &board,
        json!({"issueId": p6, "sprintId": s1, "beforeId": p1}),
    )
    .await;
    let read = backlog(&api, t, &board).await;
    assert_eq!(keys(&read["backlog"]["items"]), ["PLAT-5", "PLAT-2"]);
    assert_eq!(sprints_of(&read)[0].2, ["PLAT-6", "PLAT-1"]);

    // Every scope change is told; a rank alone too.
    let moved = events(&db, t, "issue.planned").await;
    assert_eq!(moved.len(), 4, "{moved:?}");
    assert_eq!(moved[3], json!({"boardId": board, "sprintId": null}));
    assert_eq!(events(&db, t, "issue.ranked").await.len(), 2);

    // The query language finds tickets by sprint; a sub-task follows its parent.
    assert_eq!(
        search_keys(&api, t, "sprint IN openSprints() ORDER BY key").await,
        ["PLAT-1", "PLAT-6"]
    );
    planned(&api, t, &board, json!({"issueId": p2, "sprintId": s1})).await;
    assert_eq!(
        search_keys(&api, t, "sprint = \"plat sprint 1\" ORDER BY key").await,
        ["PLAT-1", "PLAT-2", "PLAT-4", "PLAT-6"]
    );
    assert_eq!(
        search_keys(&api, t, "sprint IS EMPTY ORDER BY key").await,
        ["PLAT-3", "PLAT-5"]
    );
    assert_eq!(
        search_keys(&api, t, "sprint != \"Sprint 25\" ORDER BY key").await,
        ["PLAT-1", "PLAT-2", "PLAT-4", "PLAT-6"]
    );
    assert_eq!(
        search_keys(&api, t, "sprint = \"Sprint 25\"").await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn a_ticket_planned_by_another_board_shows_there_read_only() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let p1 = ticket(&db, t, task(plat, 1, "todo")).await;
    let p2 = ticket(&db, t, task(plat, 2, "todo")).await;
    let sub = ticket(
        &db,
        t,
        Ticket {
            type_key: "subtask",
            parent: Some(p2),
            ..task(plat, 3, "todo")
        },
    )
    .await;
    let team_a = create_board(&api, t, plat, "Team A", "scrum").await;
    let team_b = create_board(&api, t, plat, "Team B", "scrum").await;
    let a1 = create_sprint(&api, t, &team_a, json!({"name": "A1"})).await["id"].clone();
    let b1 = create_sprint(&api, t, &team_b, json!({"name": "B1"})).await["id"].clone();
    planned(&api, t, &team_a, json!({"issueId": p1, "sprintId": a1})).await;

    let read = backlog(&api, t, &team_b).await;
    assert_eq!(
        sprints_of(&read),
        [("B1", true, vec![]), ("A1", false, vec!["PLAT-1"])]
    );
    assert_eq!(read["sprints"][1]["sprint"]["boardName"], "Team A");
    assert_eq!(keys(&read["backlog"]["items"]), ["PLAT-2"]);

    // Team B cannot take it, nor send it back to the backlog.
    for target in [b1.clone(), Value::Null] {
        let (status, refused) = plan(&api, t, &team_b, json!({"issueId": p1, "sprintId": target})).await;
        assert_problem(status, &refused, 409, "sprint.foreign");
    }
    // Only this board's sprints take its tickets.
    let (status, refused) = plan(&api, t, &team_b, json!({"issueId": p2, "sprintId": a1})).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("sprintId".to_owned(), "not_found".to_owned())]
    );
    // A sub-task follows its parent.
    let (status, refused) = plan(&api, t, &team_a, json!({"issueId": sub, "sprintId": a1})).await;
    assert_problem(status, &refused, 409, "sprint.subtask");
    // Team A still sees it as its own.
    assert_eq!(
        sprints_of(&backlog(&api, t, &team_a).await),
        [("A1", true, vec!["PLAT-1"])]
    );

    // A Kanban board has no backlog nor sprints.
    let kanban = create_board(&api, t, plat, "Flow", "kanban").await;
    let (status, refused) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/boards/{kanban}/backlog"),
    ))
    .await;
    assert_problem(status, &refused, 409, "board.not_scrum");
    let (status, refused) =
        send(admin(&api, t, Method::POST, &format!("/api/v1/boards/{kanban}/sprints")).json(&json!({})))
            .await;
    assert_problem(status, &refused, 409, "board.not_scrum");
    let (status, refused) = plan(&api, t, &kanban, json!({"issueId": p2, "sprintId": null})).await;
    assert_problem(status, &refused, 409, "board.not_scrum");
}

/// The keys of each column of a Kanban board's single lane, in order.
fn column_keys(cards: &Value) -> Vec<Vec<&str>> {
    cards["lanes"][0]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cell| {
            cell.as_array()
                .unwrap()
                .iter()
                .map(|card| card["key"].as_str().unwrap())
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn kanban_cards_follow_the_board_rank() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let p1 = ticket(&db, t, task(plat, 1, "todo")).await;
    let p2 = ticket(&db, t, task(plat, 2, "todo")).await;
    let p3 = ticket(&db, t, task(plat, 3, "todo")).await;
    ticket(&db, t, task(plat, 4, "in_progress")).await;
    let board = create_board(&api, t, plat, "Flow", "kanban").await;
    let cards = |api: &TestApi| {
        let path = format!("/api/v1/boards/{board}/cards");
        let request = admin(api, t, Method::GET, &path);
        async move {
            let (status, body) = send(request).await;
            assert_eq!(status, 200, "{body}");
            body
        }
    };
    // Unranked: the filter's order.
    assert_eq!(column_keys(&cards(&api).await)[0], ["PLAT-1", "PLAT-2", "PLAT-3"]);
    let (status, ranked) = rank(&api, t, &board, json!({"issueId": p3, "beforeId": p1})).await;
    assert_eq!(status, 204, "{ranked}");
    assert_eq!(
        column_keys(&cards(&api).await)[..2],
        [vec!["PLAT-3", "PLAT-1", "PLAT-2"], vec!["PLAT-4"]]
    );
    // Neither neighbour: last of the board.
    let (status, ranked) = rank(&api, t, &board, json!({"issueId": p1})).await;
    assert_eq!(status, 204, "{ranked}");
    assert_eq!(column_keys(&cards(&api).await)[0], ["PLAT-3", "PLAT-2", "PLAT-1"]);
    // A neighbour that is the ticket itself, or not on the board.
    let ops = project(&api, t, "Operations", "OPS").await;
    let outside = ticket(
        &db,
        t,
        Ticket {
            key: "OPS",
            ..task(ops, 1, "todo")
        },
    )
    .await;
    let (status, refused) = rank(&api, t, &board, json!({"issueId": p2, "afterId": p2})).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("afterId".to_owned(), "same_issue".to_owned())]
    );
    let (status, refused) = rank(&api, t, &board, json!({"issueId": p2, "afterId": outside})).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("afterId".to_owned(), "not_found".to_owned())]
    );
}

#[tokio::test]
async fn planning_needs_manage_sprints_on_the_node() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let p1 = ticket(&db, t, task(plat, 1, "todo")).await;
    let p2 = ticket(&db, t, task(plat, 2, "todo")).await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;
    let sprint = create_sprint(&api, t, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Someone who does not see the node does not see the board.
    let (_, stranger) = member(&db, &api, t).await;
    let backlog_path = format!("/api/v1/boards/{board}/backlog");
    let (status, hidden) = send(as_member(&api, t, &stranger, Method::GET, &backlog_path)).await;
    assert_problem(status, &hidden, 404, "not_found");

    for label in ["Reader", "Contributor"] {
        let (account, session) = member(&db, &api, t).await;
        grant(&db, &api, t, plat, label, account).await;
        let (status, read) = send(as_member(&api, t, &session, Method::GET, &backlog_path)).await;
        assert_eq!(status, 200, "{read}");
        assert_eq!(read["canPlan"], false, "{label}");
        assert_eq!(keys(&read["backlog"]["items"]), ["PLAT-1", "PLAT-2"]);
        let board_path = format!("/api/v1/boards/{board}");
        let (status, shown) = send(as_member(&api, t, &session, Method::GET, &board_path)).await;
        assert_eq!((status, &shown["canPlan"]), (200, &json!(false)), "{shown}");
        let sprint_path = format!("/api/v1/sprints/{sprint}");
        for request in [
            as_member(
                &api,
                t,
                &session,
                Method::POST,
                &format!("/api/v1/boards/{board}/sprints"),
            )
            .json(&json!({})),
            as_member(&api, t, &session, Method::PUT, &sprint_path)
                .header("if-match", "*")
                .json(&json!({"name": "Mine"})),
            as_member(&api, t, &session, Method::DELETE, &sprint_path).header("if-match", "*"),
            as_member(
                &api,
                t,
                &session,
                Method::POST,
                &format!("/api/v1/boards/{board}/backlog/moves"),
            )
            .json(&json!({"issueId": p1, "sprintId": sprint})),
            as_member(
                &api,
                t,
                &session,
                Method::POST,
                &format!("/api/v1/boards/{board}/rank"),
            )
            .json(&json!({"issueId": p2, "beforeId": p1})),
        ] {
            let (status, refused) = send(request).await;
            assert_problem(status, &refused, 403, "permission.denied");
        }
        let (status, read) = send(as_member(&api, t, &session, Method::GET, &sprint_path)).await;
        assert_eq!((status, &read["name"]), (200, &json!("PLAT Sprint 1")), "{read}");
    }

    let (account, manager) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Project manager", account).await;
    let (status, read) = send(as_member(&api, t, &manager, Method::GET, &backlog_path)).await;
    assert_eq!((status, &read["canPlan"]), (200, &json!(true)), "{read}");
    let (status, listed) = send(as_member(&api, t, &manager, Method::GET, "/api/v1/boards")).await;
    assert_eq!(
        (status, &listed["items"][0]["canPlan"]),
        (200, &json!(true)),
        "{listed}"
    );
    let (status, created) = send(
        as_member(
            &api,
            t,
            &manager,
            Method::POST,
            &format!("/api/v1/boards/{board}/sprints"),
        )
        .json(&json!({})),
    )
    .await;
    assert_eq!(
        (status, &created["name"]),
        (201, &json!("PLAT Sprint 2")),
        "{created}"
    );
    let (status, moved) = send(
        as_member(
            &api,
            t,
            &manager,
            Method::POST,
            &format!("/api/v1/boards/{board}/backlog/moves"),
        )
        .json(&json!({"issueId": p1, "sprintId": sprint})),
    )
    .await;
    assert_eq!(status, 204, "{moved}");
}

#[tokio::test]
async fn sprints_are_checked_versioned_deleted_and_closed_ones_kept() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let p1 = ticket(&db, t, task(plat, 1, "todo")).await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;
    let create = format!("/api/v1/boards/{board}/sprints");
    for (body, expected) in [
        (
            json!({"name": "S", "startDate": "2026-11-27", "endDate": "2026-11-16"}),
            vec![("endDate", "before_start")],
        ),
        (
            json!({"name": "S", "startDate": "2026-11-27"}),
            vec![("endDate", "required")],
        ),
        (
            json!({"name": "x".repeat(101), "goal": "g".repeat(1_001)}),
            vec![("goal", "too_long"), ("name", "too_long")],
        ),
        (json!({"name": "  "}), vec![("name", "required")]),
    ] {
        let (status, refused) = send(admin(&api, t, Method::POST, &create).json(&body)).await;
        assert_problem(status, &refused, 400, "validation_failed");
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(f, c)| (f.to_owned(), c.to_owned()))
            .collect();
        assert_eq!(field_codes(&refused), expected, "{body}");
    }
    let (status, refused) =
        send(admin(&api, t, Method::POST, &create).json(&json!({"state": "active"}))).await;
    assert_eq!(status, 400, "{refused}");

    let sprint = create_sprint(&api, t, &board, json!({"name": "Sprint 25"})).await;
    let id = sprint["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/sprints/{id}");
    let (status, missing) =
        send(admin(&api, t, Method::PUT, &path).json(&json!({"name": "Sprint 26"}))).await;
    assert_problem(status, &missing, 428, "precondition_required");
    let (status, nameless) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&json!({"goal": "g"})),
    )
    .await;
    assert_problem(status, &nameless, 400, "validation_failed");
    let (status, changed) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&json!({"name": "Sprint 26", "goal": "Ship it", "startDate": "2026-11-30", "endDate": "2026-12-11"})),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        (&changed["name"], &changed["version"], &changed["endDate"]),
        (&json!("Sprint 26"), &json!(2), &json!("2026-12-11"))
    );
    let (status, stale) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&json!({"name": "Sprint 27"})),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let response = admin(&api, t, Method::GET, &path)
        .header("if-none-match", "\"2\"")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 304);

    // Deleting a future sprint sends its tickets back to the backlog.
    planned(&api, t, &board, json!({"issueId": p1, "sprintId": id})).await;
    let (status, stale) = send(admin(&api, t, Method::DELETE, &path).header("if-match", "\"1\"")).await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let (status, deleted) = send(admin(&api, t, Method::DELETE, &path).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204, "{deleted}");
    let read = backlog(&api, t, &board).await;
    assert!(read["sprints"].as_array().unwrap().is_empty());
    assert_eq!(keys(&read["backlog"]["items"]), ["PLAT-1"]);
    assert_eq!(
        events(&db, t, "issue.planned").await.last().unwrap()["sprintId"],
        Value::Null
    );
    let (status, gone) = send(admin(&api, t, Method::GET, &path)).await;
    assert_problem(status, &gone, 404, "not_found");

    // Started, it cannot be deleted; closed, it no longer changes nor takes tickets.
    let started = create_sprint(&api, t, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    start_sprint(&api, t, &started).await;
    let started_path = format!("/api/v1/sprints/{started}");
    let (status, refused) = send(admin(&api, t, Method::DELETE, &started_path).header("if-match", "*")).await;
    assert_problem(status, &refused, 409, "sprint.not_future");
    planned(&api, t, &board, json!({"issueId": p1, "sprintId": started})).await;
    close_sprint(&api, t, &started, json!({"moveTo": "backlog"})).await;
    let (status, refused) = send(
        admin(&api, t, Method::PUT, &started_path)
            .header("if-match", "*")
            .json(&json!({"name": "Renamed"})),
    )
    .await;
    assert_problem(status, &refused, 409, "sprint.closed");
    let (status, refused) = plan(&api, t, &board, json!({"issueId": p1, "sprintId": started})).await;
    assert_problem(status, &refused, 409, "sprint.closed");
}

#[tokio::test]
async fn the_backlog_of_an_archived_node_shows_but_takes_no_plans() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let p1 = ticket(&db, t, task(plat, 1, "todo")).await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;
    let sprint = create_sprint(&api, t, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, archived) = send(
        admin(&api, t, Method::PUT, &format!("/api/v1/nodes/{plat}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200, "{archived}");
    let read = backlog(&api, t, &board).await;
    assert_eq!(
        (&read["canPlan"], keys(&read["backlog"]["items"])),
        (&json!(false), vec!["PLAT-1"])
    );
    assert_eq!(
        get(&api, t, &format!("/api/v1/boards/{board}")).await["canPlan"],
        false
    );
    for request in [
        admin(&api, t, Method::POST, &format!("/api/v1/boards/{board}/sprints")).json(&json!({})),
        admin(&api, t, Method::PUT, &format!("/api/v1/sprints/{sprint}"))
            .header("if-match", "*")
            .json(&json!({"name": "Renamed"})),
        admin(
            &api,
            t,
            Method::POST,
            &format!("/api/v1/boards/{board}/backlog/moves"),
        )
        .json(&json!({"issueId": p1, "sprintId": sprint})),
        admin(&api, t, Method::POST, &format!("/api/v1/boards/{board}/rank")).json(&json!({"issueId": p1})),
    ] {
        let (status, refused) = send(request).await;
        assert_problem(status, &refused, 409, "node.archived");
    }
}

#[tokio::test]
async fn sprints_and_ranks_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let p1 = ticket(&db, a, task(plat, 1, "todo")).await;
    let board = create_board(&api, a, plat, "Platform Scrum", "scrum").await;
    let sprint = create_sprint(&api, a, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for request in [
        admin(&api, b, Method::GET, &format!("/api/v1/boards/{board}/backlog")),
        admin(&api, b, Method::POST, &format!("/api/v1/boards/{board}/sprints")).json(&json!({})),
        admin(&api, b, Method::GET, &format!("/api/v1/sprints/{sprint}")),
        admin(&api, b, Method::PUT, &format!("/api/v1/sprints/{sprint}"))
            .header("if-match", "*")
            .json(&json!({"name": "Theirs"})),
        admin(&api, b, Method::DELETE, &format!("/api/v1/sprints/{sprint}")).header("if-match", "*"),
        admin(
            &api,
            b,
            Method::POST,
            &format!("/api/v1/boards/{board}/backlog/moves"),
        )
        .json(&json!({"issueId": p1, "sprintId": sprint})),
        admin(&api, b, Method::POST, &format!("/api/v1/boards/{board}/rank")).json(&json!({"issueId": p1})),
    ] {
        let (status, hidden) = send(request).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    // Still there for tenant A.
    assert_eq!(
        get(&api, a, &format!("/api/v1/sprints/{sprint}")).await["version"],
        1
    );
}

/// Puts the ticket `id` in the status `status` of its configuration, directly.
async fn set_status(db: &TestDb, tenant: &SeedTenant, id: Uuid, status: &str) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::sqlx::query(
        r#"
        UPDATE issue i SET status_id = (
            SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
            AND v.state = 'published' JOIN node n ON n.config_id = v.config_id AND n.id = i.project_id
            WHERE s.key = $2)
        WHERE i.id = $1
        "#,
    )
    .bind(id)
    .bind(status)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// The keys of every card of a board, sorted.
fn card_keys(cards: &Value) -> Vec<String> {
    let mut keys: Vec<String> = cards["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|lane| lane["cells"].as_array().unwrap().iter())
        .flat_map(|cell| cell.as_array().unwrap().iter())
        .map(|card| card["key"].as_str().unwrap().to_owned())
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn a_sprint_starts_alone_on_its_board_which_then_shows_its_tickets() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let story = |number, points| Ticket {
        type_key: "story",
        points: Some(points),
        ..task(plat, number, "todo")
    };
    let p1 = ticket(&db, t, story(1, 8.0)).await;
    let p2 = ticket(&db, t, story(2, 5.0)).await;
    ticket(
        &db,
        t,
        Ticket {
            type_key: "subtask",
            parent: Some(p1),
            ..task(plat, 3, "todo")
        },
    )
    .await;
    let p4 = ticket(&db, t, task(plat, 4, "todo")).await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;
    let s1 = create_sprint(&api, t, &board, json!({"goal": "OIDC"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let s2 = create_sprint(&api, t, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    planned(&api, t, &board, json!({"issueId": p1, "sprintId": s1})).await;
    planned(&api, t, &board, json!({"issueId": p2, "sprintId": s1})).await;
    let cards_path = format!("/api/v1/boards/{board}/cards");

    // No active sprint: an empty sprint board.
    let cards = get(&api, t, &cards_path).await;
    assert_eq!(
        (card_keys(&cards), &cards["sprint"]),
        (Vec::<String>::new(), &Value::Null)
    );

    // Dates are needed.
    let start = format!("/api/v1/sprints/{s1}/start");
    let (status, refused) = send(
        admin(&api, t, Method::POST, &start)
            .header("if-match", "*")
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [
            ("endDate".to_owned(), "required".to_owned()),
            ("startDate".to_owned(), "required".to_owned())
        ]
    );
    let (status, missing) = send(admin(&api, t, Method::POST, &start).json(&json!({}))).await;
    assert_problem(status, &missing, 428, "precondition_required");
    let (status, stale) = send(
        admin(&api, t, Method::POST, &start)
            .header("if-match", "\"9\"")
            .json(&json!({"startDate": "2026-11-30", "endDate": "2026-12-11"})),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let started = start_sprint(&api, t, &s1).await;
    assert_eq!(
        (
            &started["state"],
            &started["committedCount"],
            &started["committedEstimate"],
            &started["goal"]
        ),
        (&json!("active"), &json!(2), &json!(13.0), &json!("OIDC"))
    );
    assert_eq!(
        (&started["startDate"], &started["endDate"]),
        (&json!("2026-11-30"), &json!("2026-12-11"))
    );

    // One active sprint per board, started once.
    let (status, refused) = send(
        admin(&api, t, Method::POST, &format!("/api/v1/sprints/{s2}/start"))
            .header("if-match", "*")
            .json(&json!({"startDate": "2026-12-14", "endDate": "2026-12-25"})),
    )
    .await;
    assert_problem(status, &refused, 409, "sprint.active_exists");
    let (status, refused) = send(
        admin(&api, t, Method::POST, &start)
            .header("if-match", "*")
            .json(&json!({"startDate": "2026-11-30", "endDate": "2026-12-11"})),
    )
    .await;
    assert_problem(status, &refused, 409, "sprint.not_future");

    // The board shows the sprint's tickets and their sub-tasks only.
    let cards = get(&api, t, &cards_path).await;
    assert_eq!(card_keys(&cards), ["PLAT-1", "PLAT-2", "PLAT-3"]);
    assert_eq!(
        (&cards["sprint"]["id"], &cards["sprint"]["state"]),
        (&json!(s1), &json!("active"))
    );
    // Its scope may still change: a ticket added once started.
    planned(&api, t, &board, json!({"issueId": p4, "sprintId": s1})).await;
    assert_eq!(
        card_keys(&get(&api, t, &cards_path).await),
        ["PLAT-1", "PLAT-2", "PLAT-3", "PLAT-4"]
    );
    let read = backlog(&api, t, &board).await;
    assert_eq!(
        sprints_of(&read)[0],
        ("PLAT Sprint 1", true, vec!["PLAT-1", "PLAT-2", "PLAT-4"])
    );
    assert_eq!(read["sprints"][0]["sprint"]["state"], "active");
    // A Kanban board shows no sprint.
    let kanban = create_board(&api, t, plat, "Flow", "kanban").await;
    assert_eq!(
        get(&api, t, &format!("/api/v1/boards/{kanban}/cards")).await["sprint"],
        Value::Null
    );
}

#[tokio::test]
async fn closing_a_sprint_keeps_its_scope_and_moves_the_unfinished_tickets() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let story = |number, points| Ticket {
        type_key: "story",
        points: Some(points),
        ..task(plat, number, "todo")
    };
    let p1 = ticket(&db, t, story(1, 8.0)).await;
    let p2 = ticket(&db, t, story(2, 5.0)).await;
    let p3 = ticket(&db, t, story(3, 2.0)).await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;
    let other = create_board(&api, t, plat, "Other Scrum", "scrum").await;
    let elsewhere = create_sprint(&api, t, &other, json!({})).await["id"].clone();
    let s1 = create_sprint(&api, t, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let s2 = create_sprint(&api, t, &board, json!({"name": "Next"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    planned(&api, t, &board, json!({"issueId": p1, "sprintId": s1})).await;
    planned(&api, t, &board, json!({"issueId": p2, "sprintId": s1})).await;
    start_sprint(&api, t, &s1).await;
    // Added once started, then the first one done.
    planned(&api, t, &board, json!({"issueId": p3, "sprintId": s1})).await;
    set_status(&db, t, p1, "done").await;
    let close = format!("/api/v1/sprints/{s1}/close");

    // Where the unfinished tickets go must be a future sprint of the board.
    let (status, refused) = send(
        admin(&api, t, Method::POST, &close)
            .header("if-match", "*")
            .json(&json!({"moveTo": "sprint"})),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("sprintId".to_owned(), "required".to_owned())]
    );
    for target in [elsewhere, json!(s1)] {
        let (status, refused) = send(
            admin(&api, t, Method::POST, &close)
                .header("if-match", "*")
                .json(&json!({"moveTo": "sprint", "sprintId": target})),
        )
        .await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(
            field_codes(&refused),
            [("sprintId".to_owned(), "not_found".to_owned())]
        );
    }
    let (status, refused) = send(
        admin(&api, t, Method::POST, &format!("/api/v1/sprints/{s2}/close"))
            .header("if-match", "*")
            .json(&json!({"moveTo": "backlog"})),
    )
    .await;
    assert_problem(status, &refused, 409, "sprint.not_active");

    let closed = close_sprint(&api, t, &s1, json!({"moveTo": "sprint", "sprintId": s2})).await;
    assert_eq!(
        (&closed["moved"], &closed["target"]["name"]),
        (&json!(2), &json!("Next"))
    );
    let sprint = &closed["sprint"];
    assert_eq!(
        (
            &sprint["state"],
            &sprint["committedCount"],
            &sprint["committedEstimate"]
        ),
        (&json!("closed"), &json!(2), &json!(13.0))
    );
    assert_eq!(
        (&sprint["completedCount"], &sprint["completedEstimate"]),
        (&json!(1), &json!(8.0))
    );
    assert!(sprint["closedAt"].is_string());
    let (status, refused) = send(
        admin(&api, t, Method::POST, &close)
            .header("if-match", "*")
            .json(&json!({"moveTo": "backlog"})),
    )
    .await;
    assert_problem(status, &refused, 409, "sprint.not_active");

    // The unfinished ones are in the next sprint; the closed one is listed apart.
    let read = backlog(&api, t, &board).await;
    assert_eq!(sprints_of(&read), [("Next", true, vec!["PLAT-2", "PLAT-3"])]);
    let listed = get(&api, t, &format!("/api/v1/boards/{board}/sprints?state=closed")).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["completedEstimate"], 8.0);
    let all = get(&api, t, &format!("/api/v1/boards/{board}/sprints")).await;
    let names: Vec<&str> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Next", "PLAT Sprint 1"]);
    let (status, refused) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/boards/{board}/sprints?state=done"),
    ))
    .await;
    assert_problem(status, &refused, 400, "validation_failed");

    // Its scope, read later, is as it was.
    let scope = get(&api, t, &format!("/api/v1/sprints/{s1}/scope")).await;
    let rows: Vec<(&str, bool, bool, &Value, &Value, &Value)> = scope["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["key"].as_str().unwrap(),
                r["committed"].as_bool().unwrap(),
                r["addedLater"].as_bool().unwrap(),
                &r["completed"],
                &r["estimateAtStart"],
                &r["estimateAtClose"],
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("PLAT-1", true, false, &json!(true), &json!(8.0), &json!(8.0)),
            ("PLAT-2", true, false, &json!(false), &json!(5.0), &json!(5.0)),
            ("PLAT-3", false, true, &json!(false), &Value::Null, &json!(2.0)),
        ]
    );
    assert_eq!(scope["hidden"], 0);
    assert_eq!(
        search_keys(&api, t, "sprint = \"PLAT Sprint 1\" ORDER BY key").await,
        ["PLAT-1", "PLAT-2", "PLAT-3"]
    );
    assert_eq!(
        search_keys(&api, t, "sprint IN openSprints() ORDER BY key").await,
        ["PLAT-2", "PLAT-3"]
    );
    let moved = events(&db, t, "issue.planned").await;
    assert_eq!(moved.last().unwrap()["sprintId"], json!(s2));

    // Closed towards a new sprint, named after the project.
    start_sprint(&api, t, &s2).await;
    set_status(&db, t, p2, "done").await;
    let closed = close_sprint(&api, t, &s2, json!({"moveTo": "new_sprint"})).await;
    assert_eq!(
        (&closed["moved"], &closed["target"]["name"]),
        (&json!(1), &json!("PLAT Sprint 3"))
    );
    assert_eq!(
        sprints_of(&backlog(&api, t, &board).await),
        [("PLAT Sprint 3", true, vec!["PLAT-3"])]
    );
}

#[tokio::test]
async fn starting_and_closing_need_manage_sprints_and_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (t, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let board = create_board(&api, t, plat, "Platform Scrum", "scrum").await;
    let sprint = create_sprint(&api, t, &board, json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (account, reader) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Reader", account).await;
    let dates = json!({"startDate": "2026-11-30", "endDate": "2026-12-11"});
    let start = format!("/api/v1/sprints/{sprint}/start");
    let close = format!("/api/v1/sprints/{sprint}/close");
    let (status, refused) = send(
        as_member(&api, t, &reader, Method::POST, &start)
            .header("if-match", "*")
            .json(&dates),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    for request in [
        admin(&api, b, Method::POST, &start)
            .header("if-match", "*")
            .json(&dates),
        admin(&api, b, Method::POST, &close)
            .header("if-match", "*")
            .json(&json!({"moveTo": "backlog"})),
        admin(&api, b, Method::GET, &format!("/api/v1/sprints/{sprint}/scope")),
        admin(&api, b, Method::GET, &format!("/api/v1/boards/{board}/sprints")),
    ] {
        let (status, hidden) = send(request).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    start_sprint(&api, t, &sprint).await;
    let (status, refused) = send(
        as_member(&api, t, &reader, Method::POST, &close)
            .header("if-match", "*")
            .json(&json!({"moveTo": "backlog"})),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    // The reader still reads the sprints and their scope.
    let (status, listed) = send(as_member(
        &api,
        t,
        &reader,
        Method::GET,
        &format!("/api/v1/boards/{board}/sprints"),
    ))
    .await;
    assert_eq!((status, listed["items"].as_array().unwrap().len()), (200, 1));
    let (status, scope) = send(as_member(
        &api,
        t,
        &reader,
        Method::GET,
        &format!("/api/v1/sprints/{sprint}/scope"),
    ))
    .await;
    assert_eq!(status, 200, "{scope}");
}
