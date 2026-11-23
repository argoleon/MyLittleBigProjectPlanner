//! Integration tests of capacity (L07-US03) on real PostgreSQL and Valkey:
//! a resource's periods with the tenant default, one period per date,
//! versions, who manages and who reads, the tenant's workload settings for
//! its admins only, and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const SETTINGS: &str = "/api/v1/tenant/workload-settings";

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

async fn session_of(api: &TestApi, tenant: &SeedTenant, account: Uuid) -> (String, String) {
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account, tenant.id)
    .await
    .unwrap();
    (opened.token, opened.session.csrf_token)
}

fn by(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &(String, String),
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant, role: TenantRole) -> (String, String) {
    let account = db.add_member(tenant, role).await.account_id;
    session_of(api, tenant, account).await
}

/// A resource of `tenant` and the path of its periods.
async fn resource(api: &TestApi, tenant: &SeedTenant) -> String {
    let (status, created) = send(
        api.request(Method::POST, tenant, "/api/v1/resources")
            .json(&json!({"kind": "person", "displayName": "Camille Martin"})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    format!(
        "/api/v1/resources/{}/capacity-patterns",
        created["id"].as_str().unwrap()
    )
}

#[tokio::test]
async fn a_resource_has_the_tenant_default_then_its_dated_periods() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let path = resource(&api, a).await;

    // No period: the tenant default, 7 h × 5 days.
    let (status, list) = send(api.request(Method::GET, a, &path)).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        list,
        json!({"items": [], "defaultMinutes": [420, 420, 420, 420, 420, 0, 0], "canManage": true})
    );

    let res = api
        .request(Method::POST, a, &path)
        .json(&json!({"effectiveFrom": "2026-01-01", "minutes": [420, 420, 420, 420, 420, 0, 0], "label": " Full time "}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["etag"], "\"1\"");
    let (_, january) = json(res).await;
    assert_eq!(
        january,
        json!({
            "id": january["id"], "effectiveFrom": "2026-01-01", "minutes": [420, 420, 420, 420, 420, 0, 0],
            "weeklyMinutes": 2100, "label": "Full time", "version": 1
        })
    );
    let (status, march) = send(
        api.request(Method::POST, a, &path)
            .json(&json!({"effectiveFrom": "2026-03-01", "minutes": [420, 420, 0, 420, 210, 0, 0]})),
    )
    .await;
    assert_eq!((status, &march["weeklyMinutes"]), (201, &json!(1470)));

    // One period per date.
    let (status, problem) = send(
        api.request(Method::POST, a, &path)
            .json(&json!({"effectiveFrom": "2026-03-01", "minutes": [0, 0, 0, 0, 0, 0, 0]})),
    )
    .await;
    assert_eq!(
        (status, &problem["code"]),
        (409, &json!("capacity_pattern.date_taken"))
    );
    assert_eq!(
        problem["errors"][0],
        json!({"field": "effectiveFrom", "code": "taken"})
    );

    let (_, list) = send(api.request(Method::GET, a, &path)).await;
    let dates: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["effectiveFrom"].as_str().unwrap())
        .collect();
    assert_eq!(dates, ["2026-01-01", "2026-03-01"]);

    // Changed with its version, then deleted.
    let one = format!("{path}/{}", march["id"].as_str().unwrap());
    let body = json!({"effectiveFrom": "2026-04-01", "minutes": [420, 420, 0, 420, 420, 0, 0], "label": "Part time 80%"});
    let (status, _) = send(api.request(Method::PUT, a, &one).json(&body)).await;
    assert_eq!(status, 428);
    let (status, _) = send(
        api.request(Method::PUT, a, &one)
            .header("if-match", "\"7\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let (status, changed) = send(
        api.request(Method::PUT, a, &one)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!(
        (status, &changed["effectiveFrom"], &changed["version"]),
        (200, &json!("2026-04-01"), &json!(2))
    );
    let (status, _) = send(api.request(Method::DELETE, a, &one).header("if-match", "\"1\"")).await;
    assert_eq!(status, 412);
    let (status, _) = send(api.request(Method::DELETE, a, &one).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204);
    let (status, _) = send(api.request(Method::DELETE, a, &one).header("if-match", "*")).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn invalid_periods_are_refused_with_their_field() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let path = resource(&api, a).await;
    for (body, field, code) in [
        (
            json!({"effectiveFrom": "2026-01-01", "minutes": [420, 420, 420]}),
            "minutes",
            "length",
        ),
        (
            json!({"effectiveFrom": "2026-01-01", "minutes": [420, 1441, 0, 0, 0, 0, -5]}),
            "minutes.1",
            "out_of_range",
        ),
        (
            json!({"effectiveFrom": "2026-01-01", "minutes": [0, 0, 0, 0, 0, 0, 0], "label": "x".repeat(101)}),
            "label",
            "too_long",
        ),
    ] {
        let (status, problem) = send(api.request(Method::POST, a, &path).json(&body)).await;
        assert_eq!(status, 400, "{body}: {problem}");
        assert_eq!(
            problem["errors"][0],
            json!({"field": field, "code": code}),
            "{body}"
        );
    }
    let (status, _) = send(
        api.request(Method::POST, a, &path)
            .json(&json!({"effectiveFrom": "2026-02-30", "minutes": [0, 0, 0, 0, 0, 0, 0]})),
    )
    .await;
    assert_eq!(status, 400, "an impossible date");
    let (status, _) = send(api.request(
        Method::GET,
        a,
        &format!("/api/v1/resources/{}/capacity-patterns", Uuid::nil()),
    ))
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn resource_managers_manage_periods_and_members_read_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let path = resource(&api, a).await;
    let manager = member(&db, &api, a, TenantRole::ResourceManager).await;
    let plain = member(&db, &api, a, TenantRole::Member).await;
    let body = json!({"effectiveFrom": "2026-01-01", "minutes": [420, 420, 420, 420, 420, 0, 0]});

    let (status, created) = send(by(&api, a, &manager, Method::POST, &path).json(&body)).await;
    assert_eq!(status, 201, "{created}");
    let (status, list) = send(by(&api, a, &plain, Method::GET, &path)).await;
    assert_eq!(
        (
            status,
            list["items"].as_array().unwrap().len(),
            &list["canManage"]
        ),
        (200, 1, &json!(false))
    );
    let one = format!("{path}/{}", created["id"].as_str().unwrap());
    for request in [
        by(&api, a, &plain, Method::POST, &path).json(&body),
        by(&api, a, &plain, Method::PUT, &one)
            .header("if-match", "*")
            .json(&body),
        by(&api, a, &plain, Method::DELETE, &one).header("if-match", "*"),
    ] {
        let (status, problem) = send(request).await;
        assert_eq!((status, &problem["code"]), (403, &json!("forbidden")));
    }
}

#[tokio::test]
async fn the_tenant_admin_changes_the_default_capacity_every_resource_without_period_follows() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let path = resource(&api, a).await;

    let res = api.request(Method::GET, a, SETTINGS).send().await.unwrap();
    let etag = res.headers()["etag"].to_str().unwrap().to_owned();
    let (_, settings) = json(res).await;
    assert_eq!(
        (
            &settings["defaultCapacityMinutes"],
            &settings["overloadTolerancePercent"],
            &settings["overloadMode"]
        ),
        (
            &json!([420, 420, 420, 420, 420, 0, 0]),
            &json!(0),
            &json!("blocking")
        )
    );
    let body = json!({"defaultCapacityMinutes": [480, 480, 480, 480, 240, 0, 0], "overloadTolerancePercent": 10, "overloadMode": "warning"});
    let (status, _) = send(api.request(Method::PUT, a, SETTINGS).json(&body)).await;
    assert_eq!(status, 428);
    let (status, changed) = send(
        api.request(Method::PUT, a, SETTINGS)
            .header("if-match", &etag)
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["overloadMode"], "warning");
    let (status, _) = send(
        api.request(Method::PUT, a, SETTINGS)
            .header("if-match", &etag)
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let (_, list) = send(api.request(Method::GET, a, &path)).await;
    assert_eq!(list["defaultMinutes"], json!([480, 480, 480, 480, 240, 0, 0]));

    for (body, field) in [
        (
            json!({"defaultCapacityMinutes": [480], "overloadTolerancePercent": 0, "overloadMode": "blocking"}),
            "defaultCapacityMinutes",
        ),
        (
            json!({"defaultCapacityMinutes": [0, 0, 0, 0, 0, 0, 0], "overloadTolerancePercent": 101, "overloadMode": "blocking"}),
            "overloadTolerancePercent",
        ),
        (
            json!({"defaultCapacityMinutes": [0, 0, 0, 0, 0, 0, 0], "overloadTolerancePercent": 0, "overloadMode": "lenient"}),
            "overloadMode",
        ),
    ] {
        let (status, problem) = send(
            api.request(Method::PUT, a, SETTINGS)
                .header("if-match", "*")
                .json(&body),
        )
        .await;
        assert_eq!(
            (status, &problem["errors"][0]["field"]),
            (400, &json!(field)),
            "{body}"
        );
    }
    // Resource managers and members neither read nor change them.
    for role in [TenantRole::ResourceManager, TenantRole::Member] {
        let session = member(&db, &api, a, role).await;
        let (status, _) = send(by(&api, a, &session, Method::GET, SETTINGS)).await;
        assert_eq!(status, 403);
        let (status, _) = send(
            by(&api, a, &session, Method::PUT, SETTINGS)
                .header("if-match", "*")
                .json(&body),
        )
        .await;
        assert_eq!(status, 403);
    }
}

#[tokio::test]
async fn a_tenant_never_reaches_the_periods_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let path = resource(&api, &db.tenant_a).await;
    let (status, created) = send(
        api.request(Method::POST, &db.tenant_a, &path)
            .json(&json!({"effectiveFrom": "2026-01-01", "minutes": [420, 420, 420, 420, 420, 0, 0]})),
    )
    .await;
    assert_eq!(status, 201);
    let b = &db.tenant_b;
    let (status, _) = send(api.request(Method::GET, b, &path)).await;
    assert_eq!(status, 404);
    let (status, _) = send(
        api.request(Method::POST, b, &path)
            .json(&json!({"effectiveFrom": "2026-02-01", "minutes": [0, 0, 0, 0, 0, 0, 0]})),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(
        api.request(
            Method::DELETE,
            b,
            &format!("{path}/{}", created["id"].as_str().unwrap()),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 404);
}
