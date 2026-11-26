//! Integration tests of absence conflicts (L08-US04) on real PostgreSQL and
//! Valkey: hours planned on a leave show on the ticket's assignment and on
//! the absence, the conflicts list filters by resource, project and status
//! and keeps to the projects the member sees, and the configurations carry
//! the `absence_conflict` rule; tenant isolation. (The worker side, an
//! absence declared on planned hours, is tested in the worker crate.)

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
    assert_eq!(status, 200, "{path}: {body}");
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

/// A project `key` with one story; the project's id.
async fn project(api: &TestApi, tenant: &SeedTenant, key: &str) -> String {
    let node = created(
        api,
        tenant,
        "/api/v1/nodes",
        json!({"type": "project", "name": format!("Project {key}"), "key": key}),
    )
    .await;
    let form = get(api, tenant, &format!("/api/v1/projects/{key}/issue-form")).await;
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
        &format!("/api/v1/projects/{key}/issues"),
        json!({"typeId": story, "summary": format!("Work on {key}")}),
    )
    .await;
    node["id"].as_str().unwrap().to_owned()
}

/// 7 h/day the week of 3 March 2031 for `resource` on the first ticket of `key`.
async fn seven_hours(api: &TestApi, tenant: &SeedTenant, key: &str, resource: &str) -> Value {
    created(
        api,
        tenant,
        &format!("/api/v1/issues/{key}-1/assignments"),
        json!({"resourceId": resource, "segments": [
            {"start": "2031-03-03", "end": "2031-03-07", "mode": "hours_per_day", "value": 420}
        ]}),
    )
    .await
}

#[tokio::test]
async fn hours_planned_on_a_leave_show_on_the_ticket_the_absence_and_the_list() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let plat = project(&api, a, "PLAT").await;
    let ops = project(&api, a, "OPS").await;
    let nathan = created(
        &api,
        a,
        "/api/v1/resources",
        json!({"kind": "generic", "displayName": "Nathan Roux"}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Warning mode: hours on a day of absence overload it (L08-US03).
    let (status, _) = send(
        admin(&api, a, Method::PUT, "/api/v1/tenant/workload-settings")
            .header("if-match", "*")
            .json(&json!({"defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0], "overloadTolerancePercent": 0, "overloadMode": "warning"})),
    )
    .await;
    assert_eq!(status, 200);
    let absences = format!("/api/v1/resources/{nathan}/absences");
    let leave = created(
        &api,
        a,
        &absences,
        json!({"kind": "leave", "start": "2031-03-05", "end": "2031-03-05"}),
    )
    .await;
    assert_eq!(leave["conflictMinutes"], 0);

    // 7 h/day on PLAT-1 and on OPS-1: Wednesday's hours are to reschedule.
    let assignment = seven_hours(&api, a, "PLAT", &nathan).await;
    assert_eq!(
        assignment["conflicts"],
        json!([{"date": "2031-03-05", "minutes": 420}])
    );
    seven_hours(&api, a, "OPS", &nathan).await;
    let list = get(&api, a, "/api/v1/issues/PLAT-1/assignments").await;
    assert_eq!(list["items"][0]["conflicts"][0]["minutes"], 420);

    // AC4: the absence tells the hours in conflict, all tickets together.
    let read = get(&api, a, &format!("{absences}?from=2031-03-01&to=2031-03-31")).await;
    assert_eq!(read["items"][0]["conflictMinutes"], 840);

    // The list, by resource, project and status.
    let page = get(
        &api,
        a,
        &format!("/api/v1/workload/conflicts?resourceId={nathan}"),
    )
    .await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    assert_eq!(page["nextCursor"], Value::Null);
    let plat_only = get(
        &api,
        a,
        &format!("/api/v1/workload/conflicts?projectId={plat}&status=open"),
    )
    .await;
    let item = &plat_only["items"][0];
    assert_eq!(plat_only["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            &item["issue"]["key"],
            &item["resource"]["displayName"],
            &item["day"],
            &item["minutes"],
            &item["status"]
        ),
        (
            &json!("PLAT-1"),
            &json!("Nathan Roux"),
            &json!("2031-03-05"),
            &json!(420),
            &json!("open")
        )
    );
    assert_eq!(item["assignmentId"], assignment["id"]);
    assert_eq!(item["resolvedAt"], Value::Null);
    let resolved = get(&api, a, "/api/v1/workload/conflicts?status=resolved").await;
    assert!(resolved["items"].as_array().unwrap().is_empty());
    let (status, _) = send(admin(
        &api,
        a,
        Method::GET,
        "/api/v1/workload/conflicts?status=closed",
    ))
    .await;
    assert_eq!(status, 400);
    let (status, _) = send(admin(&api, a, Method::GET, "/api/v1/workload/conflicts?limit=0")).await;
    assert_eq!(status, 400);

    // Moved off the leave: resolved.
    let path = format!("/api/v1/assignments/{}", assignment["id"].as_str().unwrap());
    let (status, changed) = send(admin(&api, a, Method::PUT, &path).header("if-match", "*").json(
        &json!({"segments": [
            {"start": "2031-03-03", "end": "2031-03-04", "mode": "hours_per_day", "value": 420}
        ]}),
    ))
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["conflicts"], json!([]));
    let resolved = get(
        &api,
        a,
        &format!("/api/v1/workload/conflicts?projectId={plat}&status=resolved"),
    )
    .await;
    assert_eq!(resolved["items"][0]["status"], "resolved");
    assert!(resolved["items"][0]["resolvedAt"].is_string());

    // A member who sees OPS only lists its conflict; PLAT is not found.
    let (account, session) = member(&db, &api, a).await;
    let reader: Value = get(&api, a, "/api/v1/tenant/roles").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["labelTranslations"]["en"] == "Reader")
        .unwrap()
        .clone();
    created(
        &api,
        a,
        &format!("/api/v1/nodes/{ops}/grants"),
        json!({"roleId": reader["id"], "accountId": account}),
    )
    .await;
    let (status, seen) = send(as_member(
        &api,
        a,
        &session,
        Method::GET,
        "/api/v1/workload/conflicts",
    ))
    .await;
    assert_eq!(status, 200, "{seen}");
    let keys: Vec<&Value> = seen["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["issue"]["key"])
        .collect();
    assert_eq!(keys, [&json!("OPS-1")]);
    let (status, _) = send(as_member(
        &api,
        a,
        &session,
        Method::GET,
        &format!("/api/v1/workload/conflicts?projectId={plat}"),
    ))
    .await;
    assert_eq!(status, 404);

    // Another tenant lists none.
    let other = get(&api, &db.tenant_b, "/api/v1/workload/conflicts").await;
    assert!(other["items"].as_array().unwrap().is_empty());
}
