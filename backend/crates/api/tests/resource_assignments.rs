//! Integration tests of the assignments of a resource (L08-US05) on real
//! PostgreSQL and Valkey: each assignment with its ticket and project
//! (`expand=issue`), segments and load; those on tickets the member does
//! not see as busy time; who may edit them; the period filter, pages and
//! refusals; editing from the list keeps the overload check; tenant
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
async fn project(api: &TestApi, tenant: &SeedTenant, key: &str, name: &str) -> String {
    let node = created(
        api,
        tenant,
        "/api/v1/nodes",
        json!({"type": "project", "name": name, "key": key}),
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

async fn role(api: &TestApi, tenant: &SeedTenant, label: &str) -> Value {
    get(api, tenant, "/api/v1/tenant/roles").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["labelTranslations"]["en"] == label)
        .unwrap()["id"]
        .clone()
}

#[tokio::test]
async fn a_resource_lists_its_assignments_and_busy_time_on_what_the_member_does_not_see() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let plat = project(&api, a, "PLAT", "Identity platform").await;
    project(&api, a, "OPS", "Operations").await;
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
    let plat_assignment = created(
        &api,
        a,
        "/api/v1/issues/PLAT-1/assignments",
        json!({"resourceId": nathan, "segments": [{"start": "2026-11-16", "end": "2026-11-20", "mode": "hours_per_day", "value": 240}]}),
    )
    .await;
    created(
        &api,
        a,
        "/api/v1/issues/OPS-1/assignments",
        json!({"resourceId": nathan, "provisional": true, "segments": [{"start": "2026-12-07", "end": "2026-12-11", "mode": "percentage", "value": 50}]}),
    )
    .await;
    let path = format!("/api/v1/resources/{nathan}/assignments");

    // AC2: the admin sees both, with their ticket and project.
    let page = get(&api, a, &format!("{path}?expand=issue")).await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    assert_eq!(page["nextCursor"], Value::Null);
    let ops = &items[0];
    assert_eq!(
        (
            &ops["issue"]["key"],
            &ops["issue"]["project"]["name"],
            &ops["busy"],
            &ops["provisional"],
            &ops["canEdit"]
        ),
        (
            &json!("OPS-1"),
            &json!("Operations"),
            &json!(false),
            &json!(true),
            &json!(true)
        )
    );
    let plat_item = &items[1];
    assert_eq!(plat_item["id"], plat_assignment["id"]);
    assert_eq!(plat_item["issue"]["project"]["key"], "PLAT");
    assert_eq!(
        (
            &plat_item["loadMinutes"],
            &plat_item["segments"][0]["workingDays"]
        ),
        (&json!(1200), &json!(5))
    );
    // Without expand: the ticket's id only.
    let bare = get(&api, a, &path).await;
    assert!(bare["items"][0].get("issue").is_none());
    assert!(bare["items"][0]["issueId"].is_string());

    // The period: from 1 December, only OPS-1.
    let later = get(&api, a, &format!("{path}?from=2026-12-01&expand=issue")).await;
    let keys: Vec<&Value> = later["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["issue"]["key"])
        .collect();
    assert_eq!(keys, [&json!("OPS-1")]);
    let first = get(&api, a, &format!("{path}?limit=1")).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = get(&api, a, &format!("{path}?limit=1&cursor={cursor}")).await;
    assert_eq!(second["items"][0]["id"], plat_assignment["id"]);

    // A member who reads PLAT only: OPS-1 is busy time, nothing editable.
    let (account, session) = member(&db, &api, a).await;
    created(
        &api,
        a,
        &format!("/api/v1/nodes/{plat}/grants"),
        json!({"roleId": role(&api, a, "Reader").await, "accountId": account}),
    )
    .await;
    let (status, seen) = send(as_member(
        &api,
        a,
        &session,
        Method::GET,
        &format!("{path}?expand=issue"),
    ))
    .await;
    assert_eq!(status, 200, "{seen}");
    let busy = &seen["items"][0];
    assert_eq!(
        (
            &busy["busy"],
            &busy["issueId"],
            &busy["loadMinutes"],
            &busy["canEdit"]
        ),
        (&json!(true), &Value::Null, &json!(1050), &json!(false))
    );
    assert!(busy.get("issue").is_none(), "{busy}");
    assert_eq!(busy["segments"][0]["start"], "2026-12-07", "its periods show");
    assert_eq!(seen["items"][1]["issue"]["key"], "PLAT-1");
    assert_eq!(seen["items"][1]["canEdit"], false, "a reader does not assign");

    // AC3: changed from the list, the overload check still applies.
    let change = format!("/api/v1/assignments/{}", plat_assignment["id"].as_str().unwrap());
    let (status, problem) = send(
        admin(&api, a, Method::PUT, &change)
            .header("if-match", "*")
            .json(&json!({"segments": [{"start": "2026-11-16", "end": "2026-11-20", "mode": "hours_per_day", "value": 480}]})),
    )
    .await;
    assert_eq!(
        (status, &problem["code"]),
        (409, &json!("workload.overload")),
        "{problem}"
    );

    // AC4: removed, it leaves the list.
    let (status, _) = send(admin(&api, a, Method::DELETE, &change).header("if-match", "*")).await;
    assert_eq!(status, 204);
    assert_eq!(get(&api, a, &path).await["items"].as_array().unwrap().len(), 1);

    // Refusals.
    for query in [
        "?expand=project",
        "?from=2026-12-10&to=2026-12-01",
        "?limit=0",
        "?from=yesterday",
    ] {
        let (status, body) = send(admin(&api, a, Method::GET, &format!("{path}{query}"))).await;
        assert_eq!(status, 400, "{query}: {body}");
    }
    let (status, _) = send(admin(
        &api,
        a,
        Method::GET,
        &format!("/api/v1/resources/{}/assignments", Uuid::now_v7()),
    ))
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(admin(&api, &db.tenant_b, Method::GET, &path)).await;
    assert_eq!(status, 404, "another tenant's resource");
}
