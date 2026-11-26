//! Integration tests of assignments (L08-US01) on real PostgreSQL and
//! Valkey: segments in the three modes with their load from the resource's
//! capacity and holidays, the ticket's dates, overlapping segments, one
//! assignment per resource and ticket, never a team, the version read,
//! idempotent creation, `assign_resources` needed, archived projects and
//! tenant isolation.

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

async fn created(api: &TestApi, tenant: &SeedTenant, path: &str, body: Value) -> Value {
    let (status, created) = send(admin(api, tenant, Method::POST, path).json(&body)).await;
    assert_eq!(status, 201, "{path}: {created}");
    created
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

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: &str, label: &str, account: Uuid) {
    let role: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
            .bind(tenant.id)
            .bind(label)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    created(
        api,
        tenant,
        &format!("/api/v1/nodes/{node}/grants"),
        json!({"roleId": role, "accountId": account}),
    )
    .await;
}

/// The project PLAT with PLAT-1, dated 16 to 27 November 2026, and the
/// undated PLAT-2; the project's id.
async fn plat(api: &TestApi, tenant: &SeedTenant) -> String {
    let node = created(
        api,
        tenant,
        "/api/v1/nodes",
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let form = get(api, tenant, "/api/v1/projects/PLAT/issue-form").await;
    let story = form["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "story")
        .unwrap()["id"]
        .clone();
    created(
        api,
        tenant,
        "/api/v1/projects/PLAT/issues",
        json!({"typeId": story, "summary": "Account migration scripts", "startDate": "2026-11-16", "dueDate": "2026-11-27"}),
    )
    .await;
    created(
        api,
        tenant,
        "/api/v1/projects/PLAT/issues",
        json!({"typeId": story, "summary": "Undated"}),
    )
    .await;
    node["id"].as_str().unwrap().to_owned()
}

async fn resource(api: &TestApi, tenant: &SeedTenant, name: &str) -> String {
    created(
        api,
        tenant,
        "/api/v1/resources",
        json!({"kind": "generic", "displayName": name}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn segment(start: &str, end: &str, mode: &str, value: u32) -> Value {
    json!({"start": start, "end": end, "mode": mode, "value": value})
}

const PLAT1: &str = "/api/v1/issues/PLAT-1/assignments";

async fn events(db: &TestDb, tenant: &SeedTenant) -> Vec<Value> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let events: Vec<Value> = infra::sqlx::query_scalar(
        "SELECT payload FROM outbox_event WHERE kind = 'assignment.changed' ORDER BY created_at, id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    events
}

#[tokio::test]
async fn a_resource_is_assigned_with_segments_and_their_load() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    let dev = resource(&api, a, "Rust developer").await;
    // A 3.5 h Friday for Nathan.
    created(
        &api,
        a,
        &format!("/api/v1/resources/{nathan}/capacity-patterns"),
        json!({"effectiveFrom": "2026-01-01", "minutes": [420, 420, 420, 420, 210, 0, 0]}),
    )
    .await;
    // 4 h on that Friday overload it (L08-US03): in warning mode the
    // assignment is written and the day flagged.
    let (status, saved) = send(
        admin(&api, a, Method::PUT, "/api/v1/tenant/workload-settings")
            .header("if-match", "*")
            .json(&json!({
                "defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0],
                "overloadTolerancePercent": 0,
                "overloadMode": "warning",
            })),
    )
    .await;
    assert_eq!(status, 200, "{saved}");

    // AC1: 4 h/day from 16 to 20 November, then 50% the week after.
    let (status, assignment) = send(
        admin(&api, a, Method::POST, PLAT1)
            .header("idempotency-key", "assign-nathan")
            .json(&json!({
                "resourceId": nathan,
                "segments": [
                    segment("2026-11-16", "2026-11-20", "hours_per_day", 240),
                    segment("2026-11-23", "2026-11-27", "percentage", 50),
                ],
            })),
    )
    .await;
    assert_eq!(status, 201, "{assignment}");
    assert_eq!(assignment["resource"]["displayName"], "Nathan Roux");
    assert_eq!(assignment["provisional"], false);
    // AC2: 50% of four 7 h days and of a 3.5 h Friday: 4 × 210 + 105.
    let loads: Vec<&Value> = assignment["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["loadMinutes"])
        .collect();
    assert_eq!(loads, [&json!(1200), &json!(945)]);
    assert_eq!(assignment["segments"][0]["workingDays"], 5);
    assert_eq!(assignment["loadMinutes"], 2145);
    assert_eq!(assignment["outsideIssueDates"], false);
    assert_eq!(assignment["overloads"][0]["date"], "2026-11-20");
    let id = assignment["id"].as_str().unwrap().to_owned();

    // The same key replays the creation.
    let (status, replay) = send(
        admin(&api, a, Method::POST, PLAT1)
            .header("idempotency-key", "assign-nathan")
            .json(&json!({
                "resourceId": nathan,
                "segments": [
                    segment("2026-11-16", "2026-11-20", "hours_per_day", 240),
                    segment("2026-11-23", "2026-11-27", "percentage", 50),
                ],
            })),
    )
    .await;
    assert_eq!((status, &replay["id"]), (201, &json!(id)));

    // AC3: total effort spread over working days; Friday 27 is short but whole.
    let generic = created(
        &api,
        a,
        PLAT1,
        json!({"resourceId": dev, "provisional": true, "segments": [segment("2026-11-16", "2026-11-27", "total_effort", 2400)]}),
    )
    .await;
    assert_eq!(
        (&generic["loadMinutes"], &generic["segments"][0]["workingDays"]),
        (&json!(2400), &json!(10))
    );
    assert_eq!(generic["provisional"], true);

    // AC5: two independent assignments; the ticket's load is their sum.
    let list = get(&api, a, PLAT1).await;
    let names: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["resource"]["displayName"])
        .collect();
    assert_eq!(names, [&json!("Nathan Roux"), &json!("Rust developer")]);
    assert_eq!(list["loadMinutes"], 2145 + 2400);
    assert_eq!(list["canAssign"], true);
    let read = get(&api, a, &format!("/api/v1/assignments/{id}")).await;
    assert_eq!(read["segments"].as_array().unwrap().len(), 2);

    // Changed with the version read, then removed.
    let path = format!("/api/v1/assignments/{id}");
    let change = json!({"segments": [segment("2026-11-16", "2026-11-18", "hours_per_day", 420)]});
    let (status, problem) = send(admin(&api, a, Method::PUT, &path).json(&change)).await;
    assert_problem(status, &problem, 428, "precondition_required");
    let (status, problem) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"7\"")
            .json(&change),
    )
    .await;
    assert_problem(status, &problem, 412, "precondition_failed");
    let (status, changed) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&change),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        (&changed["loadMinutes"], &changed["version"]),
        (&json!(1260), &json!(2))
    );
    let (status, _) = send(admin(&api, a, Method::DELETE, &path).header("if-match", "\"1\"")).await;
    assert_eq!(status, 412);
    let (status, _) = send(admin(&api, a, Method::DELETE, &path).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204);
    let (status, _) = send(admin(&api, a, Method::GET, &path)).await;
    assert_eq!(status, 404);

    // Each change told the load engine which days it touched.
    let events = events(&db, a).await;
    assert_eq!(events.len(), 4, "{events:?}");
    assert_eq!(
        (&events[0]["resourceId"], &events[0]["from"], &events[0]["until"]),
        (&json!(nathan), &json!("2026-11-16"), &json!("2026-11-28"))
    );
    assert_eq!(
        events[2]["until"], "2026-11-28",
        "the change spans the days before and after it"
    );
}

#[tokio::test]
async fn segments_stay_within_the_ticket_never_overlap_and_a_resource_once() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    let team = created(
        &api,
        a,
        "/api/v1/teams",
        json!({"nameTranslations": {"fr": "Plateforme"}}),
    )
    .await;

    let post = |body: Value| send(admin(&api, a, Method::POST, PLAT1).json(&body));
    // AC4: beyond the ticket's due date.
    let (status, problem) = post(
        json!({"resourceId": nathan, "segments": [segment("2026-11-23", "2026-11-30", "percentage", 30)]}),
    )
    .await;
    assert_problem(status, &problem, 409, "assignment.outside_issue_dates");
    assert_eq!(
        problem["errors"][0],
        json!({"field": "segments.0.end", "code": "outside_issue_dates"})
    );
    // AC6: overlapping segments.
    let (status, problem) = post(json!({"resourceId": nathan, "segments": [
        segment("2026-11-16", "2026-11-20", "hours_per_day", 240),
        segment("2026-11-20", "2026-11-24", "percentage", 30),
    ]}))
    .await;
    assert_problem(status, &problem, 409, "assignment.segments_overlap");
    // Bad values, no segment, a total effort over a weekend only.
    for (body, field, code) in [
        (
            json!({"resourceId": nathan, "segments": []}),
            "segments",
            "required",
        ),
        (
            json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-16", "percentage", 120)]}),
            "segments.0.value",
            "out_of_range",
        ),
        (
            json!({"resourceId": nathan, "segments": [segment("2026-11-20", "2026-11-16", "hours_per_day", 60)]}),
            "segments.0.end",
            "before_start",
        ),
        (
            json!({"resourceId": nathan, "segments": [segment("2026-11-21", "2026-11-22", "total_effort", 60)]}),
            "segments.0.start",
            "no_working_day",
        ),
        // Never a team: assignments always target a resource (L07-US02).
        (
            json!({"resourceId": team["id"], "segments": [segment("2026-11-16", "2026-11-16", "hours_per_day", 60)]}),
            "resourceId",
            "not_found",
        ),
    ] {
        let (status, problem) = post(body).await;
        assert_problem(status, &problem, 400, "validation_failed");
        assert_eq!(
            (&problem["errors"][0]["field"], &problem["errors"][0]["code"]),
            (&json!(field), &json!(code)),
            "{problem}"
        );
    }
    let (status, problem) = post(json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-16", "percentage", 10)], "team": "x"})).await;
    assert_problem(status, &problem, 400, "invalid_json");

    // An inactive resource is refused.
    let gone = created(
        &api,
        a,
        "/api/v1/resources",
        json!({"kind": "generic", "displayName": "Gone", "active": false}),
    )
    .await;
    let (status, problem) = post(json!({"resourceId": gone["id"], "segments": [segment("2026-11-16", "2026-11-16", "percentage", 10)]})).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(problem["errors"][0]["code"], "inactive");

    // One assignment per resource and ticket.
    created(
        &api,
        a,
        PLAT1,
        json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-20", "percentage", 50)]}),
    )
    .await;
    let (status, problem) = post(
        json!({"resourceId": nathan, "segments": [segment("2026-11-23", "2026-11-27", "percentage", 50)]}),
    )
    .await;
    assert_problem(status, &problem, 409, "assignment.resource_taken");

    // An undated ticket takes any segment.
    created(
        &api,
        a,
        "/api/v1/issues/PLAT-2/assignments",
        json!({"resourceId": nathan, "segments": [segment("2027-01-04", "2027-03-31", "percentage", 20)]}),
    )
    .await;

    // The ticket's dates change afterwards: allowed, and flagged.
    let (status, issue) = send(
        admin(&api, a, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"dueDate": "2026-11-18"})),
    )
    .await;
    assert_eq!(status, 200, "{issue}");
    let list = get(&api, a, PLAT1).await;
    assert_eq!(list["items"][0]["outsideIssueDates"], true);
}

#[tokio::test]
async fn assigning_needs_assign_resources_on_a_project_not_archived() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let project = plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    let (reader_id, reader) = member(&db, &api, a).await;
    let (contributor_id, contributor) = member(&db, &api, a).await;
    let (manager_id, manager) = member(&db, &api, a).await;
    let (_, stranger) = member(&db, &api, a).await;
    grant(&db, &api, a, &project, "Reader", reader_id).await;
    grant(&db, &api, a, &project, "Contributor", contributor_id).await;
    grant(&db, &api, a, &project, "Project manager", manager_id).await;
    let body = json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 120)]});

    // Readers see the assignments; only those with assign_resources write them.
    let (status, list) = send(as_member(&api, a, &reader, Method::GET, PLAT1)).await;
    assert_eq!((status, &list["canAssign"]), (200, &json!(false)));
    for session in [&reader, &contributor] {
        let (status, problem) = send(as_member(&api, a, session, Method::POST, PLAT1).json(&body)).await;
        assert_problem(status, &problem, 403, "permission.denied");
    }
    let (status, assignment) = send(as_member(&api, a, &manager, Method::POST, PLAT1).json(&body)).await;
    assert_eq!(status, 201, "{assignment}");
    let path = format!("/api/v1/assignments/{}", assignment["id"].as_str().unwrap());
    let (status, list) = send(as_member(&api, a, &manager, Method::GET, PLAT1)).await;
    assert_eq!((status, &list["canAssign"]), (200, &json!(true)));
    let (status, problem) =
        send(as_member(&api, a, &contributor, Method::DELETE, &path).header("if-match", "*")).await;
    assert_problem(status, &problem, 403, "permission.denied");
    // Who does not see the project does not see its assignments.
    let (status, _) = send(as_member(&api, a, &stranger, Method::GET, PLAT1)).await;
    assert_eq!(status, 404);
    let (status, _) = send(as_member(&api, a, &stranger, Method::GET, &path)).await;
    assert_eq!(status, 404);

    // Archived: readable, no longer written.
    let (status, archived) = send(
        admin(
            &api,
            a,
            Method::PUT,
            &format!("/api/v1/nodes/{project}/archiving"),
        )
        .header("if-match", "*")
        .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200, "{archived}");
    let (status, problem) = send(
        as_member(&api, a, &manager, Method::PUT, &path)
            .header("if-match", "*")
            .json(&json!({"segments": [segment("2026-11-16", "2026-11-17", "hours_per_day", 60)]})),
    )
    .await;
    assert_problem(status, &problem, 409, "node.archived");
    let (status, list) = send(as_member(&api, a, &manager, Method::GET, PLAT1)).await;
    assert_eq!(
        (
            status,
            &list["canAssign"],
            list["items"].as_array().unwrap().len()
        ),
        (200, &json!(false), 1)
    );
}

#[tokio::test]
async fn a_tenant_never_reaches_the_assignments_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    plat(&api, a).await;
    plat(&api, b).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    let assignment = created(&api, a, PLAT1, json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 120)]})).await;
    let path = format!("/api/v1/assignments/{}", assignment["id"].as_str().unwrap());

    assert!(get(&api, b, PLAT1).await["items"].as_array().unwrap().is_empty());
    for (method, body) in [
        (Method::GET, None),
        (
            Method::PUT,
            Some(json!({"segments": [segment("2026-11-16", "2026-11-17", "hours_per_day", 60)]})),
        ),
        (Method::DELETE, None),
    ] {
        let mut request = admin(&api, b, method, &path).header("if-match", "*");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let (status, problem) = send(request).await;
        assert_problem(status, &problem, 404, "not_found");
    }
    // Alpha's resource is unknown to beta.
    let (status, problem) = send(admin(&api, b, Method::POST, PLAT1).json(
        &json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-16", "percentage", 10)]}),
    ))
    .await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(get(&api, a, PLAT1).await["items"].as_array().unwrap().len(), 1);
}
