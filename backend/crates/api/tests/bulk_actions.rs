//! Integration tests of bulk actions on a search result (L06-US08) on real
//! PostgreSQL and Valkey: several tickets modified (system and shared
//! custom fields), moved through workflows that differ (only reachable
//! statuses offered, refusals reported, the others changed), moved to
//! another project with one plan for the batch (sub-tasks following);
//! tickets the member may not edit left unchanged; beyond 100 tickets, the
//! worker changes them one per job and the member is notified; limits,
//! the report and tenant isolation.

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

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, key: &str) -> Uuid {
    let (status, node) = send(
        admin(api, tenant, Method::POST, "/api/v1/nodes")
            .json(&json!({"type": "project", "name": name, "key": key})),
    )
    .await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().parse().unwrap()
}

/// `count` tickets `KEY-first…` of `project`, of the type and status keys
/// given, written directly; their ids.
/// Tickets to write: `count` of them from `KEY-first`.
struct Batch<'a> {
    project: Uuid,
    key: &'a str,
    first: i32,
    count: i32,
    type_key: &'a str,
    status: &'a str,
    parent: Option<Uuid>,
}

async fn tickets(db: &TestDb, tenant: &SeedTenant, batch: Batch<'_>) -> Vec<Uuid> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let ids: Vec<Uuid> = infra::sqlx::query_scalar(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority, parent_id)
        SELECT uuidv7(), app_current_tenant(), n.id, s, $2 || '-' || s,
               (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE t.key = $5),
               (SELECT st.item_id FROM workflow_status st JOIN project_config_version v ON v.id = st.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE st.key = $6),
               'Ticket ' || $2 || '-' || s, 'medium', $7
        FROM node n, generate_series($3::int, $3::int + $4::int - 1) AS s
        WHERE n.id = $1
        ORDER BY s
        RETURNING id
        "#,
    )
    .bind(batch.project)
    .bind(batch.key)
    .bind(batch.first)
    .bind(batch.count)
    .bind(batch.type_key)
    .bind(batch.status)
    .bind(batch.parent)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    ids
}

async fn one(
    db: &TestDb,
    tenant: &SeedTenant,
    project: Uuid,
    key: &str,
    n: i32,
    type_key: &str,
    status: &str,
) -> Uuid {
    tickets(
        db,
        tenant,
        Batch {
            project,
            key,
            first: n,
            count: 1,
            type_key,
            status,
            parent: None,
        },
    )
    .await[0]
}

/// Runs the queued jobs (bulk items, dispatches) as the worker does.
async fn run_jobs(api: &TestApi, db: &TestDb) {
    let jobs = worker::jobs::Jobs {
        mailer: infra::email::Mailer::new(
            &infra::config::Secret::new(testkit::smtp_url()),
            "Planner <noreply@example.org>",
        )
        .unwrap(),
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
        storage: None,
        clamav: None,
        public_url: Some("https://{tenant}.example.test".to_owned()),
        events: None,
    };
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
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

/// `(key, outcome, reason)` of each item of a report.
fn outcomes(report: &Value) -> Vec<(String, String, Value)> {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["key"].as_str().unwrap().to_owned(),
                i["outcome"].as_str().unwrap_or("waiting").to_owned(),
                i["reason"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn several_tickets_get_the_values_set_each_with_its_history_and_event() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let story = one(&db, t, plat, "PLAT", 1, "story", "todo").await;
    let task = one(&db, t, plat, "PLAT", 2, "task", "in_progress").await;
    let (hugo, _) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Contributor", hugo).await;

    // What the selection offers: the custom fields both types have, the
    // statuses a transition reaches.
    let selection = json!({"issueIds": [story, task]});
    let (status, options) =
        send(admin(&api, t, Method::POST, "/api/v1/bulk-actions/options").json(&selection)).await;
    assert_eq!(status, 200, "{options}");
    assert_eq!(
        (&options["total"], &options["background"]),
        (&json!(2), &json!(false))
    );
    let shared: Vec<&str> = options["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(shared, ["story_points", "remaining"]);
    let reachable: Vec<(&str, u64)> = options["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["key"].as_str().unwrap(), s["reachable"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        reachable,
        [("cancelled", 2), ("in_progress", 1), ("in_review", 1)]
    );

    let remaining = by(&options["fields"], "key", "remaining")["fieldId"].clone();
    let (status, report) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "modify", "priority": "highest", "assigneeId": hugo, "dueDate": "2026-12-01",
                   "fields": {remaining.as_str().unwrap(): 4}}
    })))
    .await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(
        (&report["status"], &report["kind"], &report["background"]),
        (&json!("done"), &json!("modify"), &json!(false))
    );
    assert_eq!(
        outcomes(&report),
        [
            ("PLAT-1".to_owned(), "changed".to_owned(), Value::Null),
            ("PLAT-2".to_owned(), "changed".to_owned(), Value::Null)
        ]
    );
    assert_eq!(report["counts"]["changed"], 2);
    for key in ["PLAT-1", "PLAT-2"] {
        let issue = get(&api, t, &format!("/api/v1/issues/{key}")).await;
        assert_eq!(
            (
                &issue["priority"],
                &issue["assignee"]["id"],
                &issue["dueDate"],
                &issue["version"]
            ),
            (&json!("highest"), &json!(hugo), &json!("2026-12-01"), &json!(2))
        );
        let activity = get(&api, t, &format!("/api/v1/issues/{key}/activity")).await;
        assert_eq!(activity["items"][0]["action"], "updated", "{activity}");
    }
    assert_eq!(events(&db, t, "issue.updated").await.len(), 2, "an event each");

    // Again: already so, unchanged; a field one of them lacks, refused alone.
    let (status, again) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "modify", "priority": "highest"}
    })))
    .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["counts"]["unchanged"], 2);
    let (status, refused) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "modify", "priority": "low", "fields": {Uuid::nil().to_string(): 1}}
    })))
    .await;
    assert_eq!(status, 200, "{refused}");
    assert_eq!(refused["counts"]["refused"], 2);
    assert_eq!(refused["items"][0]["reason"], "validation_failed");
    assert_eq!(
        refused["items"][0]["errors"][0]["field"],
        format!("fields.{}", Uuid::nil())
    );
    assert_eq!(
        get(&api, t, "/api/v1/issues/PLAT-1").await["priority"],
        "highest",
        "never changed"
    );
}

#[tokio::test]
async fn statuses_change_where_a_transition_leads_the_others_are_reported() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    // A review needing the remaining work (validator) in a configuration of its own.
    let (status, config) = send(
        admin(&api, t, Method::POST, "/api/v1/project-configs")
            .json(&json!({"nameTranslations": {"en": "Guarded"}})),
    )
    .await;
    assert_eq!(status, 201, "{config}");
    let base = format!("/api/v1/project-configs/{}", config["id"].as_str().unwrap());
    let fields = get(&api, t, &format!("{base}/fields")).await["items"].clone();
    let remaining = by(&fields, "key", "remaining")["id"].clone();
    let manager = role(&db, t, "Project manager").await;
    let open = json!({"roleIds": [], "groupIds": []});
    for (name, rules) in [
        // A review needs the remaining work.
        (
            "Send to review",
            json!({"conditions": open, "validators": [remaining], "postFunctions": [], "screenId": null}),
        ),
        // Validating is a project manager's.
        (
            "Validate",
            json!({"conditions": {"roleIds": [manager], "groupIds": []}, "validators": [], "postFunctions": [],
                   "screenId": null}),
        ),
    ] {
        let workflow = get(&api, t, &format!("{base}/workflows")).await["items"][0].clone();
        let transition = by(&workflow["transitions"], "name", name).clone();
        let (status, saved) = send(
            admin(
                &api,
                t,
                Method::PUT,
                &format!(
                    "{base}/workflow-transitions/{}",
                    transition["id"].as_str().unwrap()
                ),
            )
            .header("if-match", "*")
            .json(&json!({
                "nameTranslations": transition["nameTranslations"], "fromId": transition["fromId"],
                "toId": transition["toId"], "rules": rules
            })),
        )
        .await;
        assert_eq!(status, 200, "{name}: {saved}");
    }
    let (status, published) =
        send(admin(&api, t, Method::POST, &format!("{base}/publish")).header("if-match", "*")).await;
    assert_eq!(status, 200, "{published}");
    let ops = project(&api, t, "Operations", "OPS").await;
    let (status, moved) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("/api/v1/nodes/{plat}/configuration"),
        )
        .json(&json!({"configId": config["id"]})),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    let plat1 = one(&db, t, plat, "PLAT", 1, "task", "in_progress").await;
    let plat2 = one(&db, t, plat, "PLAT", 2, "task", "in_review").await;
    let ops1 = one(&db, t, ops, "OPS", 1, "task", "in_progress").await;
    let ops2 = one(&db, t, ops, "OPS", 2, "task", "todo").await;
    let selection = json!({"issueIds": [plat1, plat2, ops1, ops2]});

    let (status, report) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "transition", "statusKey": "in_review"}
    })))
    .await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(
        outcomes(&report),
        [
            // Its validator wants the remaining work: refused, reported.
            (
                "PLAT-1".to_owned(),
                "refused".to_owned(),
                json!("validation_failed")
            ),
            ("PLAT-2".to_owned(), "unchanged".to_owned(), Value::Null),
            ("OPS-1".to_owned(), "changed".to_owned(), Value::Null),
            // No transition from To do to In review.
            ("OPS-2".to_owned(), "refused".to_owned(), json!("transition.none")),
        ]
    );
    assert_eq!(
        report["items"][0]["errors"],
        json!([{"field": format!("fields.{}", remaining.as_str().unwrap()), "code": "required"}])
    );
    assert_eq!(
        get(&api, t, "/api/v1/issues/OPS-1").await["status"]["key"],
        "in_review"
    );
    assert_eq!(
        get(&api, t, "/api/v1/issues/PLAT-1").await["status"]["key"],
        "in_progress"
    );
    assert_eq!(events(&db, t, "issue.transitioned").await.len(), 1);

    // A transition the member may not run: restricted, reported; the
    // ticket of the other workflow validated.
    let (hugo, contributor) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Contributor", hugo).await;
    grant(&db, &api, t, ops, "Contributor", hugo).await;
    let (status, report) = send(
        as_member(&api, t, &contributor, Method::POST, "/api/v1/bulk-actions").json(&json!({
            "selection": {"issueIds": [plat2, ops1]},
            "action": {"kind": "transition", "statusKey": "done"}
        })),
    )
    .await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(
        outcomes(&report),
        [
            (
                "PLAT-2".to_owned(),
                "refused".to_owned(),
                json!("transition.restricted")
            ),
            ("OPS-1".to_owned(), "changed".to_owned(), Value::Null),
        ]
    );
    assert_eq!(
        get(&api, t, "/api/v1/issues/PLAT-2").await["status"]["key"],
        "in_review"
    );
    assert_eq!(
        get(&api, t, "/api/v1/issues/OPS-1").await["status"]["key"],
        "done"
    );
}

#[tokio::test]
async fn tickets_move_with_one_plan_for_the_batch_and_their_sub_tasks_follow() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let crm = project(&api, t, "Customer relations", "CRM").await;
    let story = one(&db, t, plat, "PLAT", 1, "story", "in_progress").await;
    let sub = tickets(
        &db,
        t,
        Batch {
            project: plat,
            key: "PLAT",
            first: 2,
            count: 1,
            type_key: "subtask",
            status: "todo",
            parent: Some(story),
        },
    )
    .await[0];
    let lone_parent = one(&db, t, plat, "PLAT", 3, "task", "todo").await;
    let lone_sub = tickets(
        &db,
        t,
        Batch {
            project: plat,
            key: "PLAT",
            first: 4,
            count: 1,
            type_key: "subtask",
            status: "todo",
            parent: Some(lone_parent),
        },
    )
    .await[0];
    let bug = one(&db, t, plat, "PLAT", 5, "bug", "done").await;
    let selection = json!({"issueIds": [story, sub, lone_sub, bug]});

    // Same configuration: nothing to map.
    let (status, plan) = send(
        admin(&api, t, Method::POST, "/api/v1/bulk-actions/move-plan").json(&json!({
            "selection": selection, "projectKey": "CRM"
        })),
    )
    .await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(
        (
            &plan["sameConfiguration"],
            &plan["tickets"],
            &plan["subtasksAlone"],
            &plan["types"],
            &plan["problems"]
        ),
        (&json!(true), &json!(2), &json!(1), &json!([]), &json!([]))
    );
    let (status, unknown) = send(
        admin(&api, t, Method::POST, "/api/v1/bulk-actions/move-plan").json(&json!({
            "selection": selection, "projectKey": "NOPE"
        })),
    )
    .await;
    assert_problem(status, &unknown, 400, "validation_failed");

    let (status, report) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "move", "projectKey": "CRM"}
    })))
    .await;
    assert_eq!(status, 200, "{report}");
    let moved: Vec<(String, String, Value, Value)> = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["key"].as_str().unwrap().to_owned(),
                i["outcome"].as_str().unwrap().to_owned(),
                i["reason"].clone(),
                i["newKey"].clone(),
            )
        })
        .collect();
    assert_eq!(
        moved,
        [
            (
                "PLAT-1".to_owned(),
                "changed".to_owned(),
                Value::Null,
                json!("CRM-1")
            ),
            (
                "PLAT-2".to_owned(),
                "with_parent".to_owned(),
                Value::Null,
                Value::Null
            ),
            (
                "PLAT-4".to_owned(),
                "refused".to_owned(),
                json!("issue.subtask_moves"),
                Value::Null
            ),
            (
                "PLAT-5".to_owned(),
                "changed".to_owned(),
                Value::Null,
                json!("CRM-3")
            ),
        ]
    );
    // The sub-task followed its parent; the lone one stayed.
    assert_eq!(get(&api, t, "/api/v1/issues/PLAT-2").await["key"], "CRM-2");
    assert_eq!(get(&api, t, "/api/v1/issues/PLAT-4").await["key"], "PLAT-4");
    assert_eq!(events(&db, t, "issue.moved").await.len(), 3);
    let _ = crm;
}

#[tokio::test]
async fn tickets_the_member_may_not_edit_are_left_unchanged_and_reported() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let ops = project(&api, t, "Operations", "OPS").await;
    let hr = project(&api, t, "Human resources", "HR").await;
    let mine = one(&db, t, plat, "PLAT", 1, "task", "todo").await;
    let read_only = one(&db, t, ops, "OPS", 1, "task", "todo").await;
    let hidden = one(&db, t, hr, "HR", 1, "task", "todo").await;
    let (hugo, session) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Contributor", hugo).await;
    grant(&db, &api, t, ops, "Reader", hugo).await;

    let (status, report) = send(
        as_member(&api, t, &session, Method::POST, "/api/v1/bulk-actions").json(&json!({
            "selection": {"issueIds": [mine, read_only, hidden]},
            "action": {"kind": "modify", "priority": "high"}
        })),
    )
    .await;
    assert_eq!(status, 200, "{report}");
    // The ticket they do not see is not even selected.
    assert_eq!(
        outcomes(&report),
        [
            ("PLAT-1".to_owned(), "changed".to_owned(), Value::Null),
            (
                "OPS-1".to_owned(),
                "refused".to_owned(),
                json!("permission.denied")
            ),
        ]
    );
    assert_eq!(get(&api, t, "/api/v1/issues/OPS-1").await["priority"], "medium");
    assert_eq!(get(&api, t, "/api/v1/issues/HR-1").await["priority"], "medium");

    // An archived project: refused, its ticket unchanged.
    let (status, archived) = send(
        admin(&api, t, Method::PUT, &format!("/api/v1/nodes/{plat}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200, "{archived}");
    let (status, report) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": {"issueIds": [mine]},
        "action": {"kind": "modify", "priority": "low"}
    })))
    .await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["items"][0]["reason"], "node.archived");

    // The report is its requester's; another tenant sees nothing.
    let id = report["id"].as_str().unwrap();
    let (status, theirs) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/bulk-actions/{id}"),
    ))
    .await;
    assert_problem(status, &theirs, 404, "not_found");
    let (status, other) = send(admin(
        &api,
        &db.tenant_b,
        Method::GET,
        &format!("/api/v1/bulk-actions/{id}"),
    ))
    .await;
    assert_problem(status, &other, 404, "not_found");
    let (status, empty) = send(
        admin(&api, &db.tenant_b, Method::POST, "/api/v1/bulk-actions").json(&json!({
            "selection": {"issueIds": [mine]},
            "action": {"kind": "modify", "priority": "low"}
        })),
    )
    .await;
    assert_problem(status, &empty, 400, "validation_failed");
    let _ = hidden;
}

#[tokio::test]
async fn beyond_a_hundred_tickets_the_worker_changes_them_and_tells_the_member() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let count = i32::try_from(domain::bulk::SYNC_MAX).unwrap() + 1;
    tickets(
        &db,
        t,
        Batch {
            project: plat,
            key: "PLAT",
            first: 1,
            count,
            type_key: "task",
            status: "todo",
            parent: None,
        },
    )
    .await;
    let (hugo, session) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Contributor", hugo).await;

    let (status, queued) = send(
        as_member(&api, t, &session, Method::POST, "/api/v1/bulk-actions").json(&json!({
            "selection": {"query": "project = PLAT ORDER BY key ASC"},
            "action": {"kind": "transition", "statusKey": "in_progress"}
        })),
    )
    .await;
    assert_eq!(status, 202, "{queued}");
    assert_eq!(
        (
            &queued["status"],
            &queued["background"],
            &queued["total"],
            &queued["counts"]["waiting"]
        ),
        (&json!("running"), &json!(true), &json!(count), &json!(count))
    );
    let id = queued["id"].as_str().unwrap().to_owned();
    run_jobs(&api, &db).await;
    let (status, done) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/bulk-actions/{id}"),
    ))
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(
        (&done["status"], &done["counts"]["changed"]),
        (&json!("done"), &json!(count))
    );
    assert!(done["completedAt"].is_string());
    // In the bell.
    let (status, notifications) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        "/api/v1/me/notifications",
    ))
    .await;
    assert_eq!(status, 200, "{notifications}");
    let notice = &notifications["items"][0];
    assert_eq!(notice["event"], "bulk_done");
    assert_eq!(
        notice["bulk"],
        json!({"id": id, "kind": "transition", "total": count, "changed": count, "unchanged": 0, "refused": 0})
    );
    assert_eq!(
        events(&db, t, "issue.transitioned").await.len(),
        usize::try_from(count).unwrap()
    );
}

#[tokio::test]
async fn a_bulk_action_is_checked_before_any_ticket() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let ticket = one(&db, t, plat, "PLAT", 1, "task", "todo").await;
    let post = |body: Value| send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&body));
    let (status, problem) =
        post(json!({"selection": {}, "action": {"kind": "modify", "priority": "low"}})).await;
    assert_problem(status, &problem, 400, "validation_failed");
    let (status, problem) =
        post(json!({"selection": {"issueIds": [ticket]}, "action": {"kind": "modify"}})).await;
    assert_problem(status, &problem, 400, "validation_failed");
    let (status, problem) =
        post(json!({"selection": {"query": "status = "}, "action": {"kind": "modify", "priority": "low"}}))
            .await;
    assert_problem(status, &problem, 400, "query.invalid");
    let (status, problem) =
        post(json!({"selection": {"issueIds": [ticket]}, "action": {"kind": "move", "projectKey": "NOPE"}}))
            .await;
    assert_problem(status, &problem, 400, "validation_failed");
    // More than a bulk action takes: refused before any ticket.
    let too_many = i32::try_from(domain::bulk::MAX).unwrap() + 1;
    tickets(
        &db,
        t,
        Batch {
            project: plat,
            key: "PLAT",
            first: 2,
            count: too_many,
            type_key: "task",
            status: "todo",
            parent: None,
        },
    )
    .await;
    let (status, problem) = post(
        json!({"selection": {"query": "project = PLAT"}, "action": {"kind": "modify", "priority": "low"}}),
    )
    .await;
    assert_problem(status, &problem, 422, "bulk.too_many");
    assert_eq!(get(&api, t, "/api/v1/issues/PLAT-1").await["priority"], "medium");
}

#[tokio::test]
async fn a_batch_moves_to_another_configuration_with_its_types_and_statuses_mapped_once() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let sales = project(&api, t, "Sales", "SALES").await;
    let (status, config) = send(
        admin(&api, t, Method::POST, "/api/v1/project-configs")
            .json(&json!({"nameTranslations": {"en": "Sales"}})),
    )
    .await;
    assert_eq!(status, 201, "{config}");
    // A new configuration: published with the default types, ids of its own.
    let (status, moved) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("/api/v1/nodes/{sales}/configuration"),
        )
        .json(&json!({"configId": config["id"]})),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    let a = one(&db, t, plat, "PLAT", 1, "task", "in_progress").await;
    let b = one(&db, t, plat, "PLAT", 2, "task", "done").await;
    let c = one(&db, t, plat, "PLAT", 3, "bug", "todo").await;
    let selection = json!({"issueIds": [a, b, c]});

    // One plan: each type and status once, across the three tickets.
    let (status, plan) = send(
        admin(&api, t, Method::POST, "/api/v1/bulk-actions/move-plan").json(&json!({
            "selection": selection, "projectKey": "SALES"
        })),
    )
    .await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(
        (&plan["sameConfiguration"], &plan["tickets"]),
        (&json!(false), &json!(3))
    );
    let needed: Vec<&str> = plan["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["key"].as_str().unwrap())
        .collect();
    assert_eq!(needed.len(), 2, "{plan}");
    assert!(needed.contains(&"task") && needed.contains(&"bug"), "{plan}");
    let suggested_types: Vec<Value> = plan["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| json!({"fromId": t["id"], "toId": t["suggestedId"]}))
        .collect();
    let status_keys: Vec<&str> = plan["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(status_keys.len(), 3, "{plan}");
    let suggested_statuses: Vec<Value> = plan["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| json!({"fromId": s["id"], "toId": s["suggestedId"]}))
        .collect();
    assert_eq!(plan["problems"], json!([]));

    // Without a mapping a type of the target lacks: each ticket refused, none moved.
    let (status, refused) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "move", "projectKey": "SALES", "typeMappings": [], "statusMappings": []}
    })))
    .await;
    assert_eq!(status, 200, "{refused}");
    assert_eq!(refused["counts"]["refused"], 3, "{refused}");
    assert_eq!(refused["items"][0]["reason"], "move.not_mapped");

    // With the plan's mappings: all moved, each with its new key.
    let (status, report) = send(admin(&api, t, Method::POST, "/api/v1/bulk-actions").json(&json!({
        "selection": selection,
        "action": {"kind": "move", "projectKey": "SALES", "typeMappings": suggested_types, "statusMappings": suggested_statuses}
    })))
    .await;
    assert_eq!(status, 200, "{report}");
    let keys: Vec<&Value> = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["newKey"])
        .collect();
    assert_eq!(keys, [&json!("SALES-1"), &json!("SALES-2"), &json!("SALES-3")]);
    let moved = get(&api, t, "/api/v1/issues/SALES-2").await;
    assert_eq!(
        (&moved["type"]["key"], &moved["status"]["key"]),
        (&json!("task"), &json!("done"))
    );
}
