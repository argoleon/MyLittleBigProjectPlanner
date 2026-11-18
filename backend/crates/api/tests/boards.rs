//! Integration tests of Kanban boards (L06-US05) on real PostgreSQL and
//! Valkey: a board's columns mapped to statuses, work in progress limits,
//! swimlanes by assignee or epic, quick filters, blockers; dropping a card
//! runs the workflow's transition (refused when none leads there or its
//! condition is not met, a screen or a choice sent back); who sees, creates
//! and changes boards; validation, versions, archived nodes and tenant
//! isolation.

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

fn by<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant, name: &str) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, TenantRole::Member).await.account_id;
    infra::sqlx::query("UPDATE account SET display_name = $2 WHERE id = $1")
        .bind(account_id)
        .bind(name)
        .execute(&db.owner_pool)
        .await
        .unwrap();
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

async fn role(db: &TestDb, tenant: &SeedTenant, label: &str) -> Uuid {
    infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
        .bind(tenant.id)
        .bind(label)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: Uuid, label: &str, account: Uuid) {
    let role = role(db, tenant, label).await;
    let path = format!("/api/v1/nodes/{node}/grants");
    let (status, granted) =
        send(admin(api, tenant, Method::POST, &path).json(&json!({"roleId": role, "accountId": account})))
            .await;
    assert_eq!(status, 201, "{granted}");
}

async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> Uuid {
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().parse().unwrap()
}

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, key: &str, parent: Option<Uuid>) -> Uuid {
    node(
        api,
        tenant,
        json!({"type": "project", "name": name, "key": key, "parentId": parent}),
    )
    .await
}

/// A ticket `KEY-n` of `project`, of the type and status keys given, written
/// directly (the tickets' own API is tested elsewhere).
#[derive(Clone, Copy)]
struct Ticket<'a> {
    project: Uuid,
    key: &'a str,
    number: i32,
    type_key: &'a str,
    status: &'a str,
    assignee: Option<Uuid>,
    parent: Option<Uuid>,
    points: Option<f64>,
}

async fn ticket(db: &TestDb, tenant: &SeedTenant, t: Ticket<'_>) -> Uuid {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let id: Uuid = infra::sqlx::query_scalar(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority,
                           assignee_id, parent_id, fields)
        SELECT uuidv7(), app_current_tenant(), n.id, $2, $3 || '-' || $2,
               (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE t.key = $4),
               (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE s.key = $5),
               'Ticket ' || $3 || '-' || $2, 'medium', $6, $7,
               CASE WHEN $8::float8 IS NULL THEN '{}'::jsonb ELSE jsonb_build_object(
                   (SELECT f.item_id::text FROM field_def f JOIN project_config_version v ON v.id = f.version_id
                    AND v.state = 'published' AND v.config_id = n.config_id WHERE f.key = 'story_points'), $8) END
        FROM node n WHERE n.id = $1
        RETURNING id
        "#,
    )
    .bind(t.project)
    .bind(t.number)
    .bind(t.key)
    .bind(t.type_key)
    .bind(t.status)
    .bind(t.assignee)
    .bind(t.parent)
    .bind(t.points)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

fn task<'a>(project: Uuid, key: &'a str, number: i32, status: &'a str) -> Ticket<'a> {
    Ticket {
        project,
        key,
        number,
        type_key: "task",
        status,
        assignee: None,
        parent: None,
        points: None,
    }
}

async fn create_board(api: &TestApi, tenant: &SeedTenant, body: Value) -> Value {
    let (status, board) = send(admin(api, tenant, Method::POST, "/api/v1/boards").json(&body)).await;
    assert_eq!(status, 201, "{board}");
    board
}

/// The board of the acceptance criteria: To do, In progress (3 at most),
/// Review, Done.
fn plat_board(node: Uuid) -> Value {
    json!({
        "nodeId": node,
        "name": "Platform board",
        "filter": "project = PLAT",
        "columns": [
            {"name": "To do", "statuses": ["todo"]},
            {"name": "In progress", "statuses": ["in_progress"], "wipLimit": 3},
            {"name": "Review", "statuses": ["in_review"]},
            {"name": "Done", "statuses": ["done"]}
        ],
        "quickFilters": [
            {"label": "My tickets", "query": "assignee = currentUser()"},
            {"label": "Bugs", "query": "type = bug"}
        ]
    })
}

/// The keys of each column, every lane together.
fn columns_of(cards: &Value) -> Vec<Vec<String>> {
    let count = cards["columns"].as_array().unwrap().len();
    let mut columns = vec![Vec::new(); count];
    for lane in cards["lanes"].as_array().unwrap() {
        for (i, cell) in lane["cells"].as_array().unwrap().iter().enumerate() {
            for card in cell.as_array().unwrap() {
                columns[i].push(card["key"].as_str().unwrap().to_owned());
            }
        }
    }
    for column in &mut columns {
        column.sort();
    }
    columns
}

fn lanes_of(cards: &Value) -> Vec<(Value, Vec<String>)> {
    cards["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|lane| {
            let mut keys: Vec<String> = lane["cells"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|cell| cell.as_array().unwrap().iter())
                .map(|card| card["key"].as_str().unwrap().to_owned())
                .collect();
            keys.sort();
            (lane["label"].clone(), keys)
        })
        .collect()
}

#[tokio::test]
async fn each_ticket_shows_in_the_column_of_its_status_with_the_wip_limits() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    let ops = project(&api, t, "Operations", "OPS", None).await;
    for (n, status) in [
        (1, "todo"),
        (2, "in_progress"),
        (3, "in_progress"),
        (4, "in_progress"),
        (5, "in_progress"),
        (6, "in_review"),
        (7, "done"),
        (8, "cancelled"),
    ] {
        ticket(&db, t, task(plat, "PLAT", n, status)).await;
    }
    ticket(&db, t, task(ops, "OPS", 1, "todo")).await;
    let board = create_board(&api, t, plat_board(plat)).await;
    assert_eq!(
        (
            &board["kind"],
            &board["estimation"],
            &board["swimlanes"],
            &board["canManage"]
        ),
        (&json!("kanban"), &json!("points"), &json!("none"), &json!(true))
    );
    assert_eq!(board["node"]["key"], "PLAT");
    let id = board["id"].as_str().unwrap();

    let cards = get(&api, t, &format!("/api/v1/boards/{id}/cards")).await;
    assert_eq!(
        columns_of(&cards),
        [
            vec!["PLAT-1"],
            vec!["PLAT-2", "PLAT-3", "PLAT-4", "PLAT-5"],
            vec!["PLAT-6"],
            vec!["PLAT-7"],
        ]
    );
    // Four cards for a limit of three: over the limit.
    let wip: Vec<(&Value, &Value, &Value)> = cards["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (&c["name"], &c["count"], &c["wip"]))
        .collect();
    assert_eq!(
        wip,
        [
            (&json!("To do"), &json!(1), &json!("unlimited")),
            (&json!("In progress"), &json!(4), &json!("exceeded")),
            (&json!("Review"), &json!(1), &json!("unlimited")),
            (&json!("Done"), &json!(1), &json!("unlimited")),
        ]
    );
    // The cancelled ticket has no column; OPS is not in the filter.
    assert_eq!(
        (&cards["total"], &cards["unmapped"], &cards["truncated"]),
        (&json!(7), &json!(1), &json!(false))
    );
    let card = &cards["lanes"][0]["cells"][0][0];
    assert_eq!(
        (
            &card["type"]["key"],
            &card["status"]["label"],
            &card["priority"],
            &card["version"]
        ),
        (&json!("task"), &json!("To do"), &json!("medium"), &json!(1))
    );
}

#[tokio::test]
async fn a_new_board_takes_a_column_per_category_and_the_default_quick_filters() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    let board = create_board(
        &api,
        t,
        json!({"nodeId": plat, "name": "Platform", "filter": "project = PLAT"}),
    )
    .await;
    let columns: Vec<(&Value, &Value, &Value)> = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (&c["name"], &c["statuses"], &c["wipLimit"]))
        .collect();
    assert_eq!(
        columns,
        [
            (&json!("To do"), &json!(["todo"]), &Value::Null),
            (
                &json!("In progress"),
                &json!(["in_progress", "in_review"]),
                &Value::Null
            ),
            (&json!("Done"), &json!(["cancelled", "done"]), &Value::Null),
        ]
    );
    assert_eq!(
        board["quickFilters"],
        json!([
            {"label": "My tickets", "query": "assignee = currentUser()"},
            {"label": "Bugs", "query": "type = bug"}
        ])
    );
    // In French, the columns are named in French.
    let (status, french) = send(
        api.request(Method::POST, t, "/api/v1/boards")
            .header("accept-language", "fr")
            .json(&json!({"nodeId": plat, "name": "Plateforme", "filter": "project = PLAT"})),
    )
    .await;
    assert_eq!(status, 201, "{french}");
    assert_eq!(french["columns"][0]["name"], "À faire");
    assert_eq!(french["quickFilters"][0]["label"], "Mes tickets");

    // The statuses columns may show, by key.
    let statuses = get(&api, t, &format!("/api/v1/boards/statuses?nodeId={plat}")).await;
    let keys: Vec<(&Value, &Value)> = statuses
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (&s["key"], &s["category"]))
        .collect();
    assert_eq!(
        keys,
        [
            (&json!("cancelled"), &json!("done")),
            (&json!("done"), &json!("done")),
            (&json!("in_progress"), &json!("in_progress")),
            (&json!("in_review"), &json!("in_progress")),
            (&json!("todo"), &json!("todo")),
        ]
    );
    assert_eq!(by(&statuses, "key", "in_review")["label"], "In review");
}

#[tokio::test]
async fn swimlanes_group_cards_by_assignee_or_epic_and_quick_filters_narrow_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let me = api.session(t).member.account_id;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    let (camille, _) = member(&db, &api, t, "Camille Martin").await;
    let epic = ticket(
        &db,
        t,
        Ticket {
            type_key: "epic",
            ..task(plat, "PLAT", 1, "in_progress")
        },
    )
    .await;
    let story = ticket(
        &db,
        t,
        Ticket {
            type_key: "story",
            parent: Some(epic),
            assignee: Some(camille),
            points: Some(5.0),
            ..task(plat, "PLAT", 2, "todo")
        },
    )
    .await;
    ticket(
        &db,
        t,
        Ticket {
            type_key: "subtask",
            parent: Some(story),
            assignee: Some(me),
            ..task(plat, "PLAT", 3, "in_progress")
        },
    )
    .await;
    ticket(
        &db,
        t,
        Ticket {
            type_key: "bug",
            assignee: Some(me),
            ..task(plat, "PLAT", 4, "todo")
        },
    )
    .await;
    let board = create_board(&api, t, plat_board(plat)).await;
    let path = format!("/api/v1/boards/{}/cards", board["id"].as_str().unwrap());

    // Without swimlanes, one row.
    let cards = get(&api, t, &path).await;
    assert_eq!(cards["swimlanes"], "none");
    assert_eq!(
        lanes_of(&cards),
        [(
            Value::Null,
            vec![
                "PLAT-1".to_owned(),
                "PLAT-2".to_owned(),
                "PLAT-3".to_owned(),
                "PLAT-4".to_owned()
            ]
        )]
    );
    // The story's points, its epic; the sub-task's epic through its story.
    let story_card = &cards["lanes"][0]["cells"][0]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "PLAT-2")
        .unwrap()
        .clone();
    assert_eq!(
        (
            &story_card["estimate"],
            &story_card["epic"]["key"],
            &story_card["assignee"]["name"]
        ),
        (&json!(5.0), &json!("PLAT-1"), &json!("Camille Martin"))
    );

    // By assignee: by name, the unassigned last.
    let me_name: String = infra::sqlx::query_scalar("SELECT display_name FROM account WHERE id = $1")
        .bind(me)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    let cards = get(&api, t, &format!("{path}?swimlanes=assignee")).await;
    assert_eq!(cards["swimlanes"], "assignee");
    let mut expected = vec![
        (json!("Camille Martin"), vec!["PLAT-2".to_owned()]),
        (json!(me_name), vec!["PLAT-3".to_owned(), "PLAT-4".to_owned()]),
    ];
    expected.sort_by_key(|(name, _)| name.as_str().unwrap().to_lowercase());
    expected.push((Value::Null, vec!["PLAT-1".to_owned()]));
    assert_eq!(lanes_of(&cards), expected);

    // By epic: the epic's tickets, then those without one.
    let cards = get(&api, t, &format!("{path}?swimlanes=epic")).await;
    assert_eq!(
        lanes_of(&cards),
        [
            (
                json!("Ticket PLAT-1"),
                vec!["PLAT-2".to_owned(), "PLAT-3".to_owned()]
            ),
            (Value::Null, vec!["PLAT-1".to_owned(), "PLAT-4".to_owned()]),
        ]
    );
    assert_eq!(cards["lanes"][0]["key"], "PLAT-1");
    assert_eq!(cards["lanes"][0]["id"], json!(epic));

    // Quick filters: mine, then my bugs.
    let cards = get(&api, t, &format!("{path}?quick=0")).await;
    assert_eq!(cards["total"], 2);
    let cards = get(&api, t, &format!("{path}?quick=0,1")).await;
    assert_eq!(columns_of(&cards)[0], ["PLAT-4"]);
    assert_eq!(cards["total"], 1);
    let (status, refused) = send(admin(&api, t, Method::GET, &format!("{path}?quick=2"))).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("quick".to_owned(), "out_of_range".to_owned())]
    );
}

#[tokio::test]
async fn a_card_shows_the_unfinished_tickets_blocking_it() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    ticket(&db, t, task(plat, "PLAT", 1, "in_progress")).await;
    ticket(&db, t, task(plat, "PLAT", 2, "done")).await;
    ticket(&db, t, task(plat, "PLAT", 3, "todo")).await;
    for blocker in ["PLAT-1", "PLAT-2"] {
        let (status, linked) = send(
            admin(&api, t, Method::POST, "/api/v1/issues/PLAT-3/links")
                .json(&json!({"relation": "blocked_by", "issue": blocker})),
        )
        .await;
        assert_eq!(status, 201, "{linked}");
    }
    let board = create_board(&api, t, plat_board(plat)).await;
    let cards = get(
        &api,
        t,
        &format!("/api/v1/boards/{}/cards", board["id"].as_str().unwrap()),
    )
    .await;
    let blocked = &cards["lanes"][0]["cells"][0][0];
    assert_eq!(
        (&blocked["key"], &blocked["blockedBy"]),
        (&json!("PLAT-3"), &json!(["PLAT-1"]))
    );
}

/// Makes "Validate" (In review → Done) a project manager's transition and
/// gives "Send to review" (In progress → In review) a screen with the
/// points, in a configuration of its own for `project`.
async fn guarded_workflow(db: &TestDb, api: &TestApi, tenant: &SeedTenant, project: Uuid) {
    let (status, config) = send(
        admin(api, tenant, Method::POST, "/api/v1/project-configs")
            .json(&json!({"nameTranslations": {"en": "Guarded"}})),
    )
    .await;
    assert_eq!(status, 201, "{config}");
    let base = format!("/api/v1/project-configs/{}", config["id"].as_str().unwrap());
    let fields = get(api, tenant, &format!("{base}/fields")).await["items"].clone();
    let points = by(&fields, "key", "story_points")["id"].clone();
    let body = json!({"kind": "transition", "nameTranslations": {"en": "Estimate"}, "fieldIds": [points]});
    let (status, screen) =
        send(admin(api, tenant, Method::POST, &format!("{base}/screens")).json(&body)).await;
    assert_eq!(status, 201, "{screen}");
    let manager = role(db, tenant, "Project manager").await;
    let open = json!({"roleIds": [], "groupIds": []});
    for (name, rules) in [
        (
            "Send to review",
            json!({"conditions": open, "validators": [], "postFunctions": [], "screenId": screen["id"]}),
        ),
        (
            "Validate",
            json!({"conditions": {"roleIds": [manager], "groupIds": []}, "validators": [],
                   "postFunctions": [], "screenId": null}),
        ),
    ] {
        let workflow = get(api, tenant, &format!("{base}/workflows")).await["items"][0].clone();
        let transition = by(&workflow["transitions"], "name", name).clone();
        let body = json!({
            "nameTranslations": transition["nameTranslations"],
            "fromId": transition["fromId"],
            "toId": transition["toId"],
            "rules": rules,
        });
        let path = format!(
            "{base}/workflow-transitions/{}",
            transition["id"].as_str().unwrap()
        );
        let (status, saved) = send(
            admin(api, tenant, Method::PUT, &path)
                .header("if-match", "*")
                .json(&body),
        )
        .await;
        assert_eq!(status, 200, "{name}: {saved}");
    }
    let (status, published) =
        send(admin(api, tenant, Method::POST, &format!("{base}/publish")).header("if-match", "*")).await;
    assert_eq!(status, 200, "{published}");
    let path = format!("/api/v1/nodes/{project}/configuration");
    let (status, moved) =
        send(admin(api, tenant, Method::PUT, &path).json(&json!({"configId": config["id"]}))).await;
    assert_eq!(status, 200, "{moved}");
}

fn column_id(board: &Value, name: &str) -> Value {
    by(&board["columns"], "name", name)["id"].clone()
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

#[tokio::test]
async fn dropping_a_card_runs_the_transition_leading_to_its_column() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    guarded_workflow(&db, &api, t, plat).await;
    let todo = ticket(&db, t, task(plat, "PLAT", 1, "todo")).await;
    let review = ticket(&db, t, task(plat, "PLAT", 2, "in_review")).await;
    let progress = ticket(&db, t, task(plat, "PLAT", 3, "in_progress")).await;
    let mut body = plat_board(plat);
    body["columns"][3]["statuses"] = json!(["done", "cancelled"]);
    let board = create_board(&api, t, body).await;
    let moves = format!("/api/v1/boards/{}/moves", board["id"].as_str().unwrap());
    let (hugo, contributor) = member(&db, &api, t, "Hugo Petit").await;
    grant(&db, &api, t, plat, "Contributor", hugo).await;
    let drop = |session: &Session, version: Option<&str>, body: Value| {
        let request = as_member(&api, t, session, Method::POST, &moves).json(&body);
        match version {
            Some(version) => request.header("if-match", version.to_owned()),
            None => request,
        }
    };

    // To do → In progress: "Start" runs.
    let body = json!({"issueId": todo, "columnId": column_id(&board, "In progress")});
    let (status, refused) = send(drop(&contributor, None, body.clone())).await;
    assert_problem(status, &refused, 428, "precondition_required");
    let (status, moved) = send(drop(&contributor, Some("\"1\""), body.clone())).await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(
        (
            &moved["outcome"],
            &moved["issue"]["status"]["key"],
            &moved["issue"]["version"]
        ),
        (&json!("moved"), &json!("in_progress"), &json!(2))
    );
    assert_eq!(moved["transitions"], json!([]));
    let transitioned = events(&db, t, "issue.transitioned").await;
    assert_eq!(transitioned.len(), 1);
    assert_eq!(transitioned[0]["key"], "PLAT-1");
    // The same version again: the ticket changed since.
    let (status, stale) = send(drop(&contributor, Some("\"1\""), body.clone())).await;
    assert_problem(status, &stale, 412, "precondition_failed");
    // Dropped where it already is: nothing runs.
    let (status, unchanged) = send(drop(&contributor, Some("*"), body)).await;
    assert_eq!(status, 200, "{unchanged}");
    assert_eq!(
        (&unchanged["outcome"], &unchanged["issue"]["version"]),
        (&json!("unchanged"), &json!(2))
    );

    // To do → Done: no transition leads there, the card goes back.
    let (status, refused) = send(drop(
        &contributor,
        Some("*"),
        json!({"issueId": todo, "columnId": column_id(&board, "To do")}),
    ))
    .await;
    assert_problem(status, &refused, 409, "board.no_transition");
    let (status, refused) = send(drop(
        &contributor,
        Some("*"),
        json!({"issueId": progress, "columnId": column_id(&board, "To do")}),
    ))
    .await;
    assert_problem(status, &refused, 409, "board.no_transition");
    assert!(
        refused["detail"].as_str().unwrap().contains("in_progress"),
        "{refused}"
    );

    // In progress → Review: its screen first.
    let (status, screen) = send(drop(
        &contributor,
        Some("*"),
        json!({"issueId": progress, "columnId": column_id(&board, "Review")}),
    ))
    .await;
    assert_eq!(status, 200, "{screen}");
    assert_eq!(
        (&screen["outcome"], &screen["issue"]["status"]["key"]),
        (&json!("screen"), &json!("in_progress"))
    );
    assert_eq!(screen["transitions"][0]["name"], "Send to review");
    assert_eq!(screen["transitions"][0]["fields"].as_array().unwrap().len(), 1);

    // In review → Done: "Validate" is a project manager's, "Cancel" is open:
    // only "Cancel" leads there for a contributor.
    let (status, cancelled) = send(drop(
        &contributor,
        Some("*"),
        json!({"issueId": review, "columnId": column_id(&board, "Done")}),
    ))
    .await;
    assert_eq!(status, 200, "{cancelled}");
    assert_eq!(cancelled["issue"]["status"]["key"], "cancelled");

    // The admin meets both: a choice, then the one picked.
    let back = ticket(&db, t, task(plat, "PLAT", 4, "in_review")).await;
    let done = column_id(&board, "Done");
    let (status, choice) = send(
        admin(&api, t, Method::POST, &moves)
            .header("if-match", "*")
            .json(&json!({"issueId": back, "columnId": done})),
    )
    .await;
    assert_eq!(status, 200, "{choice}");
    assert_eq!(choice["outcome"], "choose");
    let names: Vec<&str> = choice["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Validate", "Cancel"]);
    let validate = choice["transitions"][0]["id"].clone();
    let (status, validated) = send(
        admin(&api, t, Method::POST, &moves)
            .header("if-match", "*")
            .json(&json!({"issueId": back, "columnId": done, "transitionId": validate})),
    )
    .await;
    assert_eq!(status, 200, "{validated}");
    assert_eq!(
        (&validated["outcome"], &validated["issue"]["status"]["key"]),
        (&json!("moved"), &json!("done"))
    );
    // A transition that does not lead to the column.
    let (status, refused) =
        send(admin(&api, t, Method::POST, &moves).header("if-match", "*").json(
            &json!({"issueId": todo, "columnId": column_id(&board, "Review"), "transitionId": validate}),
        ))
        .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("transitionId".to_owned(), "not_found".to_owned())]
    );
    let (status, refused) = send(
        admin(&api, t, Method::POST, &moves)
            .header("if-match", "*")
            .json(&json!({"issueId": todo, "columnId": Uuid::nil()})),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("columnId".to_owned(), "not_found".to_owned())]
    );
}

#[tokio::test]
async fn a_restricted_transition_sends_the_card_back_with_the_reason() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    guarded_workflow(&db, &api, t, plat).await;
    let review = ticket(&db, t, task(plat, "PLAT", 1, "in_review")).await;
    let board = create_board(&api, t, plat_board(plat)).await;
    let moves = format!("/api/v1/boards/{}/moves", board["id"].as_str().unwrap());
    let (hugo, contributor) = member(&db, &api, t, "Hugo Petit").await;
    grant(&db, &api, t, plat, "Contributor", hugo).await;
    // Done holds "done" only: "Validate" alone leads there, a project
    // manager's.
    let (status, refused) = send(
        as_member(&api, t, &contributor, Method::POST, &moves)
            .header("if-match", "*")
            .json(&json!({"issueId": review, "columnId": column_id(&board, "Done")})),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    assert_eq!(refused["detail"], "the transition is restricted");
    let status_now = get(&api, t, "/api/v1/issues/PLAT-1").await["status"]["key"].clone();
    assert_eq!(status_now, "in_review");

    // A reader sees the board but moves nothing.
    let (lea, reader) = member(&db, &api, t, "Léa Fontaine").await;
    grant(&db, &api, t, plat, "Reader", lea).await;
    let (status, cards) = send(as_member(
        &api,
        t,
        &reader,
        Method::GET,
        &format!("/api/v1/boards/{}/cards", board["id"].as_str().unwrap()),
    ))
    .await;
    assert_eq!(status, 200, "{cards}");
    let (status, refused) = send(
        as_member(&api, t, &reader, Method::POST, &moves)
            .header("if-match", "*")
            .json(&json!({"issueId": review, "columnId": column_id(&board, "To do")})),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
}

#[tokio::test]
async fn members_see_the_boards_of_their_nodes_and_only_the_tickets_they_may_see() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let digital = node(
        &api,
        t,
        json!({"type": "portfolio", "name": "Digital", "key": "DIGITAL"}),
    )
    .await;
    let plat = project(&api, t, "Identity platform", "PLAT", Some(digital)).await;
    let crm = project(&api, t, "Customer relations", "CRM", Some(digital)).await;
    ticket(&db, t, task(plat, "PLAT", 1, "todo")).await;
    ticket(&db, t, task(crm, "CRM", 1, "todo")).await;
    let board = create_board(
        &api,
        t,
        json!({"nodeId": digital, "name": "Digital", "filter": "project IN descendantsOf(\"DIGITAL\")"}),
    )
    .await;
    let id = board["id"].as_str().unwrap();
    assert_eq!(
        columns_of(&get(&api, t, &format!("/api/v1/boards/{id}/cards")).await)[0],
        ["CRM-1", "PLAT-1"]
    );

    // A contributor of the portfolio sees its board, and the tickets of
    // the projects open to them: CRM is confidential, closed to those above.
    let (status, closed) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("/api/v1/nodes/{crm}/confidentiality"),
        )
        .header("if-match", "*")
        .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200, "{closed}");
    let (hugo, contributor) = member(&db, &api, t, "Hugo Petit").await;
    grant(&db, &api, t, digital, "Contributor", hugo).await;
    let (status, list) = send(as_member(&api, t, &contributor, Method::GET, "/api/v1/boards")).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["items"][0]["name"], "Digital");
    let (status, read) = send(as_member(
        &api,
        t,
        &contributor,
        Method::GET,
        &format!("/api/v1/boards/{id}"),
    ))
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["canManage"], false);
    let (status, cards) = send(as_member(
        &api,
        t,
        &contributor,
        Method::GET,
        &format!("/api/v1/boards/{id}/cards"),
    ))
    .await;
    assert_eq!(status, 200, "{cards}");
    assert_eq!(columns_of(&cards)[0], ["PLAT-1"]);
    assert_eq!(cards["total"], 1);

    // On PLAT, a contributor may not create a board, a project manager may.
    let body = json!({"nodeId": plat, "name": "Team", "filter": "project = PLAT"});
    let (status, refused) =
        send(as_member(&api, t, &contributor, Method::POST, "/api/v1/boards").json(&body)).await;
    assert_problem(status, &refused, 403, "permission.denied");
    assert_eq!(refused["detail"], "missing permission: manage_boards");
    let (nina, manager) = member(&db, &api, t, "Nina Roux").await;
    grant(&db, &api, t, plat, "Project manager", nina).await;
    let (status, created) =
        send(as_member(&api, t, &manager, Method::POST, "/api/v1/boards").json(&body)).await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["canManage"], true);
    let team = created["id"].as_str().unwrap();
    let (status, read) = send(as_member(
        &api,
        t,
        &contributor,
        Method::GET,
        &format!("/api/v1/boards/{team}"),
    ))
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["canManage"], false);
    let (status, list) = send(as_member(
        &api,
        t,
        &contributor,
        Method::GET,
        &format!("/api/v1/boards?nodeId={plat}"),
    ))
    .await;
    assert_eq!(status, 200, "{list}");
    let names: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| &b["name"])
        .collect();
    assert_eq!(names, [&json!("Team")]);
    // The contributor changes nothing of it.
    let (status, refused) = send(
        as_member(
            &api,
            t,
            &contributor,
            Method::PUT,
            &format!("/api/v1/boards/{team}"),
        )
        .header("if-match", "*")
        .json(&json!({"name": "Mine", "filter": "project = PLAT"})),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    let (status, refused) = send(
        as_member(
            &api,
            t,
            &contributor,
            Method::DELETE,
            &format!("/api/v1/boards/{team}"),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");

    // A member granted nothing sees no board.
    let (_, stranger) = member(&db, &api, t, "Sam Blanc").await;
    let (status, list) = send(as_member(&api, t, &stranger, Method::GET, "/api/v1/boards")).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["items"], json!([]));
    let (status, hidden) = send(as_member(
        &api,
        t,
        &stranger,
        Method::GET,
        &format!("/api/v1/boards/{team}"),
    ))
    .await;
    assert_problem(status, &hidden, 404, "not_found");
    let (status, hidden) = send(as_member(
        &api,
        t,
        &stranger,
        Method::GET,
        &format!("/api/v1/boards/{team}/cards"),
    ))
    .await;
    assert_problem(status, &hidden, 404, "not_found");
}

#[tokio::test]
async fn a_board_is_checked_versioned_and_deleted() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;

    // Columns sharing a status, a wrong limit, a quick filter with ORDER BY.
    let body = json!({
        "nodeId": plat,
        "name": " ",
        "filter": "project = PLAT",
        "columns": [
            {"name": "To do", "statuses": ["todo"], "wipLimit": 0},
            {"name": "Also", "statuses": ["todo"]}
        ],
        "quickFilters": [{"label": "Sorted", "query": "type = bug ORDER BY due"}]
    });
    let (status, refused) = send(admin(&api, t, Method::POST, "/api/v1/boards").json(&body)).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [
            ("columns.0.wipLimit".to_owned(), "out_of_range".to_owned()),
            ("columns.1.statuses".to_owned(), "duplicate".to_owned()),
            ("name".to_owned(), "required".to_owned()),
            (
                "quickFilters.0.query".to_owned(),
                "order_by_not_allowed".to_owned()
            ),
        ]
    );
    // A filter that does not parse: where and why.
    let (status, refused) = send(
        admin(&api, t, Method::POST, "/api/v1/boards")
            .json(&json!({"nodeId": plat, "name": "Broken", "filter": "project = "})),
    )
    .await;
    assert_problem(status, &refused, 400, "query.invalid");
    assert_eq!(refused["query"]["position"], 10);
    // An unknown node.
    let (status, refused) = send(
        admin(&api, t, Method::POST, "/api/v1/boards")
            .json(&json!({"nodeId": Uuid::nil(), "name": "Nowhere", "filter": ""})),
    )
    .await;
    assert_problem(status, &refused, 404, "not_found");

    let board = create_board(&api, t, plat_board(plat)).await;
    let id = board["id"].as_str().unwrap();
    // The same name on the node, whatever its case.
    let (status, taken) = send(
        admin(&api, t, Method::POST, "/api/v1/boards")
            .json(&json!({"nodeId": plat, "name": "PLATFORM BOARD", "filter": ""})),
    )
    .await;
    assert_problem(status, &taken, 409, "board.name_taken");

    // Changed with the version read; columns left out stay.
    let path = format!("/api/v1/boards/{id}");
    let change = json!({"name": "Platform", "filter": "project = PLAT ORDER BY due", "swimlanes": "epic", "estimation": "hours"});
    let (status, refused) = send(admin(&api, t, Method::PUT, &path).json(&change)).await;
    assert_problem(status, &refused, 428, "precondition_required");
    let (status, changed) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&change),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        (
            &changed["name"],
            &changed["swimlanes"],
            &changed["estimation"],
            &changed["version"]
        ),
        (&json!("Platform"), &json!("epic"), &json!("hours"), &json!(2))
    );
    assert_eq!(changed["columns"].as_array().unwrap().len(), 4);
    assert_eq!(changed["quickFilters"].as_array().unwrap().len(), 2);
    let (status, stale) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&change),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    // Columns replaced.
    let (status, changed) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "*")
            .json(&json!({
                "name": "Platform",
                "filter": "project = PLAT",
                "columns": [{"name": "Open", "statuses": ["todo", "in_progress"], "wipLimit": 5}],
                "quickFilters": []
            })),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["columns"].as_array().unwrap().len(), 1);
    assert_eq!(changed["columns"][0]["wipLimit"], 5);
    assert_eq!(changed["quickFilters"], json!([]));
    // ETag and If-None-Match.
    let response = admin(&api, t, Method::GET, &path).send().await.unwrap();
    let etag = response.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(etag, "\"3\"");
    let response = admin(&api, t, Method::GET, &path)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 304);

    // Deleted with its columns.
    let (status, stale) = send(admin(&api, t, Method::DELETE, &path).header("if-match", "\"1\"")).await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let response = admin(&api, t, Method::DELETE, &path)
        .header("if-match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let (status, gone) = send(admin(&api, t, Method::GET, &path)).await;
    assert_problem(status, &gone, 404, "not_found");
    let columns: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM board_column WHERE board_id = $1")
        .bind(id.parse::<Uuid>().unwrap())
        .fetch_one(&db.owner_pool)
        .await
        .unwrap_or(0);
    assert_eq!(columns, 0);
}

#[tokio::test]
async fn the_board_of_an_archived_node_shows_but_does_not_change() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT", None).await;
    let todo = ticket(&db, t, task(plat, "PLAT", 1, "todo")).await;
    let board = create_board(&api, t, plat_board(plat)).await;
    let id = board["id"].as_str().unwrap();
    let (status, archived) = send(
        admin(&api, t, Method::PUT, &format!("/api/v1/nodes/{plat}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200, "{archived}");
    let read = get(&api, t, &format!("/api/v1/boards/{id}")).await;
    assert_eq!(read["node"]["archived"], true);
    let cards = get(&api, t, &format!("/api/v1/boards/{id}/cards")).await;
    assert_eq!(cards["total"], 1);
    let (status, refused) = send(
        admin(&api, t, Method::PUT, &format!("/api/v1/boards/{id}"))
            .header("if-match", "*")
            .json(&json!({"name": "Archived", "filter": ""})),
    )
    .await;
    assert_problem(status, &refused, 409, "node.archived");
    let (status, refused) = send(
        admin(&api, t, Method::POST, &format!("/api/v1/boards/{id}/moves"))
            .header("if-match", "*")
            .json(&json!({"issueId": todo, "columnId": column_id(&board, "In progress")})),
    )
    .await;
    assert_problem(status, &refused, 409, "node.archived");
    let (status, refused) = send(
        admin(&api, t, Method::POST, "/api/v1/boards")
            .json(&json!({"nodeId": plat, "name": "Another", "filter": ""})),
    )
    .await;
    assert_problem(status, &refused, 409, "node.archived");
}

#[tokio::test]
async fn boards_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT", None).await;
    let todo = ticket(&db, a, task(plat, "PLAT", 1, "todo")).await;
    let board = create_board(&api, a, plat_board(plat)).await;
    let id = board["id"].as_str().unwrap();
    for (method, path) in [
        (Method::GET, format!("/api/v1/boards/{id}")),
        (Method::GET, format!("/api/v1/boards/{id}/cards")),
    ] {
        let (status, hidden) = send(admin(&api, b, method, &path)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    let (status, hidden) = send(
        admin(&api, b, Method::PUT, &format!("/api/v1/boards/{id}"))
            .header("if-match", "*")
            .json(&json!({"name": "Mine", "filter": ""})),
    )
    .await;
    assert_problem(status, &hidden, 404, "not_found");
    let (status, hidden) = send(
        admin(&api, b, Method::POST, &format!("/api/v1/boards/{id}/moves"))
            .header("if-match", "*")
            .json(&json!({"issueId": todo, "columnId": column_id(&board, "In progress")})),
    )
    .await;
    assert_problem(status, &hidden, 404, "not_found");
    let (status, hidden) = send(
        admin(&api, b, Method::POST, "/api/v1/boards")
            .json(&json!({"nodeId": plat, "name": "Theirs", "filter": ""})),
    )
    .await;
    assert_problem(status, &hidden, 404, "not_found");
    assert_eq!(get(&api, b, "/api/v1/boards").await["items"], json!([]));
    // Tenant B's statuses list its own configurations only.
    assert_eq!(
        get(&api, b, "/api/v1/boards/statuses")
            .await
            .as_array()
            .unwrap()
            .len(),
        5
    );
}
