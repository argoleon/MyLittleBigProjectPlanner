//! Integration tests of replacing a generic resource with a person
//! (L08-US08) on real PostgreSQL and Valkey: the preview writing nothing,
//! days ahead moved and past ones kept, the generic resource deactivated or
//! not, the ticket's activity, segments joining the person's assignment,
//! overlaps refused, overloads refused then forced with the permission and
//! traced, validation and permission errors, and tenant isolation.
//!
//! The replacement moves days from the server's today on: the tests plan
//! in the past (November 2026) and far ahead (March 2030, a week without
//! public holidays) so that they hold whatever the day they run.

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

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant, role: TenantRole) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, role).await.account_id;
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

/// Five 7-hour days a week, blocking, no tolerance.
async fn settings(api: &TestApi, tenant: &SeedTenant) {
    let (status, saved) = send(
        admin(api, tenant, Method::PUT, "/api/v1/tenant/workload-settings")
            .header("if-match", "*")
            .json(&json!({
                "defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0],
                "overloadTolerancePercent": 0,
                "overloadMode": "blocking",
            })),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
}

/// The project PLAT with three undated tickets, PLAT-1 to PLAT-3.
async fn plat(api: &TestApi, tenant: &SeedTenant) {
    created(
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
    for summary in ["Account migration scripts", "Directory sync", "Audit export"] {
        created(
            api,
            tenant,
            "/api/v1/projects/PLAT/issues",
            json!({"typeId": story, "summary": summary}),
        )
        .await;
    }
}

async fn resource(api: &TestApi, tenant: &SeedTenant, kind: &str, name: &str) -> String {
    created(
        api,
        tenant,
        "/api/v1/resources",
        json!({"kind": kind, "displayName": name}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn segment(start: &str, end: &str, minutes: u32) -> Value {
    json!({"start": start, "end": end, "mode": "hours_per_day", "value": minutes})
}

async fn assign(api: &TestApi, tenant: &SeedTenant, issue: &str, resource: &str, segments: Vec<Value>) -> Value {
    created(
        api,
        tenant,
        &format!("/api/v1/issues/{issue}/assignments"),
        json!({"resourceId": resource, "segments": segments}),
    )
    .await
}

/// The segments of `resource` on `issue`, as (start, end, minutes).
async fn segments_of(api: &TestApi, tenant: &SeedTenant, issue: &str, resource: &str) -> Vec<(String, String, u64)> {
    let list = get(api, tenant, &format!("/api/v1/issues/{issue}/assignments")).await;
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["resource"]["id"] == resource)
        .flat_map(|a| a["segments"].as_array().unwrap().clone())
        .map(|s| {
            (
                s["start"].as_str().unwrap().to_owned(),
                s["end"].as_str().unwrap().to_owned(),
                s["value"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn span(start: &str, end: &str, minutes: u64) -> (String, String, u64) {
    (start.to_owned(), end.to_owned(), minutes)
}

fn replacement(id: &str) -> String {
    format!("/api/v1/resources/{id}/replacement")
}

fn preview(id: &str) -> String {
    format!("/api/v1/resources/{id}/replacement/preview")
}

#[tokio::test]
async fn days_ahead_move_to_the_person_past_ones_stay_and_the_generic_is_deactivated() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    settings(&api, a).await;
    plat(&api, a).await;
    let generic = resource(&api, a, "generic", "Backend developer").await;
    let person = resource(&api, a, "person", "Nathan Roux").await;
    assign(
        &api,
        a,
        "PLAT-1",
        &generic,
        vec![segment("2026-11-16", "2026-11-17", 120), segment("2030-03-11", "2030-03-12", 120)],
    )
    .await;
    assign(&api, a, "PLAT-2", &generic, vec![segment("2030-03-13", "2030-03-13", 60)]).await;

    // AC1: the preview tells what would move, and writes nothing.
    let (status, told) = send(admin(&api, a, Method::POST, &preview(&generic)).json(&json!({"personId": person}))).await;
    assert_eq!(status, 200, "{told}");
    let keys: Vec<&str> = told["moved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["issueKey"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 2, "{told}");
    assert!(keys.contains(&"PLAT-1") && keys.contains(&"PLAT-2"), "{told}");
    assert_eq!(told["blocked"], false);
    assert_eq!(told["deactivated"], false);
    assert_eq!(told["overloads"], json!([]));
    assert!(segments_of(&api, a, "PLAT-2", &person).await.is_empty());

    let (status, done) = send(admin(&api, a, Method::POST, &replacement(&generic)).json(&json!({"personId": person}))).await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["moved"].as_array().unwrap().len(), 2);
    assert_eq!(done["deactivated"], true);

    assert_eq!(segments_of(&api, a, "PLAT-1", &generic).await, vec![span("2026-11-16", "2026-11-17", 120)]);
    assert_eq!(segments_of(&api, a, "PLAT-1", &person).await, vec![span("2030-03-11", "2030-03-12", 120)]);
    assert!(segments_of(&api, a, "PLAT-2", &generic).await.is_empty());
    assert_eq!(segments_of(&api, a, "PLAT-2", &person).await, vec![span("2030-03-13", "2030-03-13", 60)]);
    let read = get(&api, a, &format!("/api/v1/resources/{generic}")).await;
    assert_eq!(read["active"], false, "{read}");

    // AC2: the ticket's activity tells who replaced whom.
    let activity = get(&api, a, "/api/v1/issues/PLAT-1/activity").await;
    let entry = activity["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "resource_replaced")
        .unwrap_or_else(|| panic!("{activity}"));
    assert_eq!(entry["old"]["text"], "Backend developer");
    assert_eq!(entry["new"]["text"], "Nathan Roux");

    // An inactive generic resource can be replaced again: nothing left.
    let (status, again) = send(
        admin(&api, a, Method::POST, &replacement(&generic)).json(&json!({"personId": person, "deactivate": false})),
    )
    .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["moved"], json!([]));
}

#[tokio::test]
async fn the_generic_stays_active_on_demand_and_segments_join_the_persons_assignment() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    settings(&api, a).await;
    plat(&api, a).await;
    let generic = resource(&api, a, "generic", "Backend developer").await;
    let person = resource(&api, a, "person", "Nathan Roux").await;
    assign(&api, a, "PLAT-1", &generic, vec![segment("2030-03-11", "2030-03-12", 120)]).await;
    assign(&api, a, "PLAT-1", &person, vec![segment("2030-03-14", "2030-03-15", 60)]).await;

    let (status, done) = send(
        admin(&api, a, Method::POST, &replacement(&generic)).json(&json!({"personId": person, "deactivate": false})),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["moved"][0]["merged"], true, "{done}");
    assert_eq!(done["deactivated"], false);
    assert_eq!(
        segments_of(&api, a, "PLAT-1", &person).await,
        vec![span("2030-03-11", "2030-03-12", 120), span("2030-03-14", "2030-03-15", 60)]
    );
    assert_eq!(get(&api, a, &format!("/api/v1/resources/{generic}")).await["active"], true);
}

#[tokio::test]
async fn overlapping_segments_refuse_the_whole_replacement() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    settings(&api, a).await;
    plat(&api, a).await;
    let generic = resource(&api, a, "generic", "Backend developer").await;
    let person = resource(&api, a, "person", "Nathan Roux").await;
    assign(&api, a, "PLAT-1", &generic, vec![segment("2030-03-11", "2030-03-12", 60)]).await;
    assign(&api, a, "PLAT-2", &generic, vec![segment("2030-03-11", "2030-03-12", 60)]).await;
    assign(&api, a, "PLAT-2", &person, vec![segment("2030-03-12", "2030-03-13", 60)]).await;

    let (status, told) = send(admin(&api, a, Method::POST, &preview(&generic)).json(&json!({"personId": person}))).await;
    assert_eq!(status, 200, "{told}");
    assert_eq!(told["overlaps"], json!([{"issueId": told["overlaps"][0]["issueId"], "issueKey": "PLAT-2"}]));
    assert_eq!(told["blocked"], true);

    let (status, problem) = send(admin(&api, a, Method::POST, &replacement(&generic)).json(&json!({"personId": person}))).await;
    assert_problem(status, &problem, 409, "assignment.segments_overlap");
    assert!(problem["detail"].as_str().unwrap().contains("PLAT-2"), "{problem}");
    // Nothing written, not even the ticket without overlap.
    assert_eq!(segments_of(&api, a, "PLAT-1", &generic).await.len(), 1);
    assert!(segments_of(&api, a, "PLAT-1", &person).await.is_empty());
    assert_eq!(get(&api, a, &format!("/api/v1/resources/{generic}")).await["active"], true);
}

#[tokio::test]
async fn an_overload_is_refused_then_forced_with_the_permission_and_traced() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    settings(&api, a).await;
    plat(&api, a).await;
    let generic = resource(&api, a, "generic", "Backend developer").await;
    let person = resource(&api, a, "person", "Nathan Roux").await;
    // 5 h already on Monday 11 March 2030; 4 h more make 9 h of 7 h.
    assign(&api, a, "PLAT-3", &person, vec![segment("2030-03-11", "2030-03-11", 300)]).await;
    assign(&api, a, "PLAT-1", &generic, vec![segment("2030-03-11", "2030-03-12", 240)]).await;
    let (_, manager) = member(&db, &api, a, TenantRole::ResourceManager).await;
    let body = json!({"personId": person});

    // A resource manager without `force_overload` on PLAT: refused, not forceable.
    let (status, told) = send(as_member(&api, a, &manager, Method::POST, &preview(&generic)).json(&body)).await;
    assert_eq!(status, 200, "{told}");
    assert_eq!(told["blocked"], true);
    assert_eq!(told["forceable"], false);
    assert_eq!(told["overloads"][0]["date"], "2030-03-11");
    assert_eq!(told["overloads"][0]["plannedMinutes"], 540);
    let (status, problem) = send(as_member(&api, a, &manager, Method::POST, &replacement(&generic)).json(&body)).await;
    assert_problem(status, &problem, 409, "workload.overload");
    assert_eq!(problem["overload"]["forceable"], false);
    assert_eq!(problem["overload"]["days"].as_array().unwrap().len(), 1);
    let (status, problem) = send(
        as_member(&api, a, &manager, Method::POST, &replacement(&generic))
            .json(&json!({"personId": person, "force": true})),
    )
    .await;
    assert_problem(status, &problem, 403, "permission.denied");
    assert!(segments_of(&api, a, "PLAT-1", &person).await.is_empty());

    // A tenant admin forces it: written, traced in both places.
    let (status, done) = send(
        admin(&api, a, Method::POST, &replacement(&generic)).json(&json!({"personId": person, "force": true})),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["overloads"].as_array().unwrap().len(), 1);
    assert_eq!(segments_of(&api, a, "PLAT-1", &person).await, vec![span("2030-03-11", "2030-03-12", 240)]);
    let forced = get(&api, a, "/api/v1/tenant/forced-overloads").await;
    let items = forced["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{forced}");
    assert_eq!(items[0]["issueKey"], "PLAT-1");
    assert_eq!(items[0]["resourceName"], "Nathan Roux");
    let activity = get(&api, a, "/api/v1/issues/PLAT-1/activity").await;
    let actions: Vec<&str> = activity["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    assert!(actions.contains(&"overload_forced") && actions.contains(&"resource_replaced"), "{activity}");
}

#[tokio::test]
async fn wrong_resources_members_and_other_tenants_are_refused() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let generic = resource(&api, a, "generic", "Backend developer").await;
    let other_generic = resource(&api, a, "generic", "Tester").await;
    let person = resource(&api, a, "person", "Nathan Roux").await;
    let gone = resource(&api, a, "person", "Iris Morel").await;
    let (status, body) = send(
        admin(&api, a, Method::PUT, &format!("/api/v1/resources/{gone}"))
            .header("if-match", "\"1\"")
            .json(&json!({"kind": "person", "displayName": "Iris Morel", "active": false})),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    for path in [replacement(&generic), preview(&generic)] {
        let field_error = |body: &Value| body["errors"][0].clone();
        let (status, problem) = send(admin(&api, a, Method::POST, &path).json(&json!({"personId": other_generic}))).await;
        assert_problem(status, &problem, 400, "validation_failed");
        assert_eq!(field_error(&problem), json!({"field": "personId", "code": "not_a_person"}));
        let (status, problem) = send(admin(&api, a, Method::POST, &path).json(&json!({"personId": gone}))).await;
        assert_problem(status, &problem, 400, "validation_failed");
        assert_eq!(field_error(&problem), json!({"field": "personId", "code": "inactive"}));
        let (status, problem) =
            send(admin(&api, a, Method::POST, &path).json(&json!({"personId": Uuid::from_u128(7)}))).await;
        assert_problem(status, &problem, 400, "validation_failed");
        assert_eq!(field_error(&problem), json!({"field": "personId", "code": "not_found"}));
    }
    // Only a generic resource is replaced.
    let (status, problem) =
        send(admin(&api, a, Method::POST, &replacement(&person)).json(&json!({"personId": person}))).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(problem["errors"][0], json!({"field": "id", "code": "not_generic"}));
    let (status, problem) = send(
        admin(&api, a, Method::POST, &replacement(&Uuid::from_u128(7).to_string())).json(&json!({"personId": person})),
    )
    .await;
    assert_problem(status, &problem, 404, "not_found");

    // A plain member is refused; another tenant does not find the resource.
    let (_, plain) = member(&db, &api, a, TenantRole::Member).await;
    for path in [replacement(&generic), preview(&generic)] {
        let (status, problem) =
            send(as_member(&api, a, &plain, Method::POST, &path).json(&json!({"personId": person}))).await;
        assert_problem(status, &problem, 403, "forbidden");
    }
    let b = &db.tenant_b;
    let (status, problem) =
        send(admin(&api, b, Method::POST, &replacement(&generic)).json(&json!({"personId": person}))).await;
    assert_problem(status, &problem, 404, "not_found");
    // Unknown fields are refused.
    let (status, problem) = send(
        admin(&api, a, Method::POST, &replacement(&generic)).json(&json!({"personId": person, "from": "2030-01-01"})),
    )
    .await;
    assert_eq!(status, 400, "{problem}");
}
