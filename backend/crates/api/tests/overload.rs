//! Integration tests of the overload check (L08-US03) on real PostgreSQL
//! and Valkey: blocking with the days listed, the tolerance, warning mode,
//! forcing with `force_overload` traced in the ticket's history and the
//! forced overloads, permission denials, provisional assignments by the
//! tenant setting, two writers at once on one resource, the preview equal
//! to the write, and tenant isolation.

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

/// Grants `account` a role of `permissions` on the project PLAT.
async fn grant(api: &TestApi, tenant: &SeedTenant, project: &str, account: Uuid, permissions: Value) {
    let role = created(
        api,
        tenant,
        "/api/v1/tenant/roles",
        json!({"labelTranslations": {"fr": "Planificateur", "en": "Planner"}, "permissions": permissions}),
    )
    .await;
    created(
        api,
        tenant,
        &format!("/api/v1/nodes/{project}/grants"),
        json!({"roleId": role["id"], "accountId": account}),
    )
    .await;
}

/// The project PLAT with PLAT-1 (16 to 27 November 2026) and the undated
/// PLAT-2 and PLAT-3; the project's id.
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
    for summary in ["Directory sync", "Audit export"] {
        created(
            api,
            tenant,
            "/api/v1/projects/PLAT/issues",
            json!({"typeId": story, "summary": summary}),
        )
        .await;
    }
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

fn assignments(issue: &str) -> String {
    format!("/api/v1/issues/{issue}/assignments")
}

/// Sets the tenant's tolerance, mode and provisional setting.
async fn settings(
    api: &TestApi,
    tenant: &SeedTenant,
    tolerance: u32,
    mode: &str,
    counts_provisional: Option<bool>,
) -> Value {
    let mut body = json!({
        "defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0],
        "overloadTolerancePercent": tolerance,
        "overloadMode": mode,
    });
    if let Some(counts) = counts_provisional {
        body["overloadCountsProvisional"] = json!(counts);
    }
    let (status, saved) = send(
        admin(api, tenant, Method::PUT, "/api/v1/tenant/workload-settings")
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    saved
}

/// 4 h already planned on Wednesday 18 November, through PLAT-2.
async fn four_hours_on_the_18th(
    api: &TestApi,
    tenant: &SeedTenant,
    resource: &str,
    provisional: bool,
) -> Value {
    created(
        api,
        tenant,
        &assignments("PLAT-2"),
        json!({"resourceId": resource, "provisional": provisional, "segments": [segment("2026-11-18", "2026-11-18", "hours_per_day", 240)]}),
    )
    .await
}

fn four_hours_a_day(resource: &str) -> Value {
    json!({"resourceId": resource, "segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 240)]})
}

/// The daily load rows of `assignment`, by day.
async fn load_rows(db: &TestDb, tenant: &SeedTenant, assignment: &str) -> Vec<(String, i32)> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let rows: Vec<(String, i32)> = infra::sqlx::query_as(
        "SELECT day::text, minutes FROM resource_day_load WHERE assignment_id = $1 ORDER BY day",
    )
    .bind(Uuid::try_parse(assignment).unwrap())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rows
}

#[tokio::test]
async fn an_overload_is_refused_with_its_days_and_forced_with_the_permission() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let project = plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    four_hours_on_the_18th(&api, a, &nathan, false).await;

    // AC1: 4 h/day more, blocking mode by default: refused, the 18th listed.
    let (status, problem) =
        send(admin(&api, a, Method::POST, &assignments("PLAT-1")).json(&four_hours_a_day(&nathan))).await;
    assert_problem(status, &problem, 409, "workload.overload");
    assert_eq!(
        problem["overload"]["days"],
        json!([{"date": "2026-11-18", "plannedMinutes": 480, "availableMinutes": 420, "limitMinutes": 420}])
    );
    assert_eq!(problem["overload"]["forceable"], true, "a tenant admin may force");
    assert!(
        get(&api, a, &assignments("PLAT-1")).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // A planner without "Force overload" is refused, and may not force.
    let (planner, session) = member(&db, &api, a).await;
    grant(&api, a, &project, planner, json!(["view", "assign_resources"])).await;
    let (status, problem) = send(
        as_member(&api, a, &session, Method::POST, &assignments("PLAT-1")).json(&four_hours_a_day(&nathan)),
    )
    .await;
    assert_problem(status, &problem, 409, "workload.overload");
    assert_eq!(problem["overload"]["forceable"], false);
    let mut forced = four_hours_a_day(&nathan);
    forced["force"] = json!(true);
    let (status, problem) =
        send(as_member(&api, a, &session, Method::POST, &assignments("PLAT-1")).json(&forced)).await;
    assert_problem(status, &problem, 403, "permission.denied");

    // AC4: forced by an admin, written and traced.
    let assignment = created(&api, a, &assignments("PLAT-1"), forced.clone()).await;
    assert_eq!(
        assignment["overloads"],
        json!([{"date": "2026-11-18", "plannedMinutes": 480, "availableMinutes": 420, "limitMinutes": 420}])
    );
    let list = get(&api, a, &assignments("PLAT-1")).await;
    assert_eq!(
        list["items"][0]["overloads"][0]["date"], "2026-11-18",
        "the ticket flags it"
    );
    let other = get(&api, a, &assignments("PLAT-2")).await;
    assert_eq!(
        other["items"][0]["overloads"][0]["date"], "2026-11-18",
        "on both tickets"
    );

    let page = get(&api, a, "/api/v1/tenant/forced-overloads").await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{page}");
    assert_eq!(
        (
            &items[0]["issueKey"],
            &items[0]["resourceName"],
            &items[0]["assignmentId"]
        ),
        (&json!("PLAT-1"), &json!("Nathan Roux"), &assignment["id"])
    );
    let actor = items[0]["actor"]["id"].clone();
    assert!(actor.is_string(), "{page}");
    assert_eq!(items[0]["days"][0]["plannedMinutes"], 480);
    assert_eq!(page["nextCursor"], Value::Null);
    let (status, problem) = send(as_member(
        &api,
        a,
        &session,
        Method::GET,
        "/api/v1/tenant/forced-overloads",
    ))
    .await;
    assert_problem(status, &problem, 403, "forbidden");

    let activity = get(&api, a, "/api/v1/issues/PLAT-1/activity").await;
    let entry = activity["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "overload_forced")
        .unwrap_or_else(|| panic!("{activity}"));
    assert_eq!(entry["new"]["text"], "Nathan Roux");
    assert_eq!(entry["actor"]["id"], actor, "the same author");

    // A change that adds nothing to the 18th passes without forcing again;
    // one that adds to it trips.
    let path = format!("/api/v1/assignments/{}", assignment["id"].as_str().unwrap());
    let same = json!({"segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 240)]});
    let (status, changed) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&same),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    let more = json!({"segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 300)]});
    let (status, problem) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"2\"")
            .json(&more),
    )
    .await;
    assert_problem(status, &problem, 409, "workload.overload");
    assert_eq!(problem["overload"]["days"][0]["plannedMinutes"], 540);
    assert_eq!(
        get(&api, a, "/api/v1/tenant/forced-overloads").await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn the_tolerance_lets_some_load_through_and_warning_mode_flags_it() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    four_hours_on_the_18th(&api, a, &nathan, false).await;

    // AC2: 10% of 7 h is 42 min: 4 h + 3 h 30 = 7 h 30 fits.
    settings(&api, a, 10, "blocking", None).await;
    let fits = json!({"resourceId": nathan, "segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 210)]});
    let assignment = created(&api, a, &assignments("PLAT-1"), fits).await;
    assert_eq!(assignment["overloads"], json!([]));
    // 7 h 43 on the 18th does not.
    let path = format!("/api/v1/assignments/{}", assignment["id"].as_str().unwrap());
    let over = json!({"segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 223)]});
    let (status, problem) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&over),
    )
    .await;
    assert_problem(status, &problem, 409, "workload.overload");
    assert_eq!(problem["overload"]["days"][0]["limitMinutes"], 462);

    // AC3: warning mode writes it and flags the day.
    settings(&api, a, 10, "warning", None).await;
    let (status, changed) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&over),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        changed["overloads"],
        json!([{"date": "2026-11-18", "plannedMinutes": 463, "availableMinutes": 420, "limitMinutes": 462}])
    );
    assert!(
        get(&api, a, "/api/v1/tenant/forced-overloads").await["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing forced"
    );
}

#[tokio::test]
async fn provisional_assignments_count_by_the_tenant_setting() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    four_hours_on_the_18th(&api, a, &nathan, true).await;

    // Counted by default.
    let current = get(&api, a, "/api/v1/tenant/workload-settings").await;
    assert_eq!(current["overloadCountsProvisional"], true);
    let (status, problem) =
        send(admin(&api, a, Method::POST, &assignments("PLAT-1")).json(&four_hours_a_day(&nathan))).await;
    assert_problem(status, &problem, 409, "workload.overload");

    // Not counted: the provisional 4 h no longer block, nor are flagged.
    let saved = settings(&api, a, 0, "blocking", Some(false)).await;
    assert_eq!(saved["overloadCountsProvisional"], false);
    let assignment = created(&api, a, &assignments("PLAT-1"), four_hours_a_day(&nathan)).await;
    assert_eq!(assignment["overloads"], json!([]));
    // Left out of a change, the setting stays.
    assert_eq!(
        settings(&api, a, 0, "blocking", None).await["overloadCountsProvisional"],
        false
    );
    // A provisional assignment never trips either: 4 + 4 + 4 h on the 18th.
    let provisional = json!({"resourceId": nathan, "provisional": true, "segments": [segment("2026-11-18", "2026-11-18", "hours_per_day", 240)]});
    let written = created(&api, a, &assignments("PLAT-3"), provisional).await;
    assert_eq!(written["overloads"], json!([]));

    // Counted again: the firm assignment now takes part in an overload.
    settings(&api, a, 0, "blocking", Some(true)).await;
    let list = get(&api, a, &assignments("PLAT-1")).await;
    assert_eq!(list["items"][0]["overloads"][0]["plannedMinutes"], 720);
}

#[tokio::test]
async fn two_writers_at_once_never_exceed_the_capacity_together() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plat(&api, a).await;

    // AC5: two project managers put 4 h on the same day of a 7 h resource,
    // at the same time; a few rounds, each with its own resource.
    for round in 0..5 {
        let resource = resource(&api, a, &format!("Nathan Roux {round}")).await;
        let body = json!({"resourceId": resource, "segments": [segment("2026-11-18", "2026-11-18", "hours_per_day", 240)]});
        let (one, two) = tokio::join!(
            send(admin(&api, a, Method::POST, &assignments("PLAT-2")).json(&body)),
            send(admin(&api, a, Method::POST, &assignments("PLAT-3")).json(&body)),
        );
        let mut statuses = [one.0, two.0];
        statuses.sort_unstable();
        assert_eq!(statuses, [201, 409], "round {round}: {one:?} {two:?}");
        let refused = if one.0 == 409 { &one.1 } else { &two.1 };
        assert_eq!(refused["code"], "workload.overload", "round {round}");
        assert_eq!(
            refused["overload"]["days"][0]["plannedMinutes"], 480,
            "round {round}: the second saw the first"
        );
    }
}

#[tokio::test]
async fn the_preview_shows_each_day_and_equals_the_write() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let project = plat(&api, a).await;
    let nathan = resource(&api, a, "Nathan Roux").await;
    four_hours_on_the_18th(&api, a, &nathan, false).await;
    let plat1 = get(&api, a, "/api/v1/issues/PLAT-1").await["id"].clone();

    // AC6: existing load, this assignment and the total, per day.
    let candidate = json!({
        "issueId": plat1,
        "resourceId": nathan,
        "segments": [segment("2026-11-16", "2026-11-22", "total_effort", 1210)],
    });
    let (status, preview) =
        send(admin(&api, a, Method::POST, "/api/v1/assignments/preview").json(&candidate)).await;
    assert_eq!(status, 200, "{preview}");
    let days = preview["days"].as_array().unwrap();
    assert_eq!(
        days.len(),
        5,
        "the weekend has neither capacity nor load: {preview}"
    );
    assert_eq!(
        days[2],
        json!({"date": "2026-11-18", "capacityMinutes": 420, "limitMinutes": 420, "existingMinutes": 240,
               "assignmentMinutes": 242, "totalMinutes": 482, "overloaded": true})
    );
    assert_eq!(
        days[0]["assignmentMinutes"], 242,
        "the spare minutes go to the first days"
    );
    assert_eq!(days[4]["assignmentMinutes"], 242);
    assert_eq!(preview["overloads"][0]["date"], "2026-11-18");
    assert_eq!(
        (
            &preview["mode"],
            &preview["blocked"],
            &preview["forceable"],
            &preview["tolerancePercent"]
        ),
        (&json!("blocking"), &json!(true), &json!(true), &json!(0))
    );

    // Forced, the write gives the rows the preview showed.
    let mut body = candidate.clone();
    body.as_object_mut().unwrap().remove("issueId");
    body["force"] = json!(true);
    let written = created(&api, a, &assignments("PLAT-1"), body).await;
    let id = written["id"].as_str().unwrap().to_owned();
    let previewed: Vec<(String, i32)> = days
        .iter()
        .map(|d| {
            (
                d["date"].as_str().unwrap().to_owned(),
                i32::try_from(d["assignmentMinutes"].as_i64().unwrap()).unwrap(),
            )
        })
        .collect();
    assert_eq!(load_rows(&db, a, &id).await, previewed);

    // Editing it: its own load is not existing load, and lowering it passes.
    let (status, edit) = send(
        admin(&api, a, Method::POST, "/api/v1/assignments/preview").json(&json!({
            "issueId": plat1,
            "resourceId": nathan,
            "assignmentId": id,
            "segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 180)],
        })),
    )
    .await;
    assert_eq!(status, 200, "{edit}");
    assert_eq!(
        (
            &edit["days"][2]["existingMinutes"],
            &edit["days"][2]["totalMinutes"],
            &edit["blocked"]
        ),
        (&json!(240), &json!(420), &json!(false))
    );

    // Refusals: segments as for a write, an assignment of another ticket,
    // a member without assign_resources, another tenant.
    let (status, problem) = send(
        admin(&api, a, Method::POST, "/api/v1/assignments/preview").json(&json!({
            "issueId": plat1, "resourceId": nathan, "segments": [],
        })),
    )
    .await;
    assert_problem(status, &problem, 400, "validation_failed");
    let (status, problem) = send(
        admin(&api, a, Method::POST, "/api/v1/assignments/preview").json(&json!({
            "issueId": plat1, "resourceId": nathan, "assignmentId": Uuid::now_v7(),
            "segments": [segment("2026-11-16", "2026-11-20", "hours_per_day", 180)],
        })),
    )
    .await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(problem["errors"][0]["field"], "assignmentId");
    let (viewer, session) = member(&db, &api, a).await;
    grant(&api, a, &project, viewer, json!(["view"])).await;
    let (status, problem) =
        send(as_member(&api, a, &session, Method::POST, "/api/v1/assignments/preview").json(&candidate))
            .await;
    assert_problem(status, &problem, 403, "permission.denied");
    let (status, _) =
        send(admin(&api, &db.tenant_b, Method::POST, "/api/v1/assignments/preview").json(&candidate)).await;
    assert_eq!(status, 404);
    let (status, _) = send(admin(
        &api,
        &db.tenant_b,
        Method::GET,
        "/api/v1/tenant/forced-overloads",
    ))
    .await;
    assert_eq!(status, 200, "tenant B lists its own");
    assert!(
        get(&api, &db.tenant_b, "/api/v1/tenant/forced-overloads").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
