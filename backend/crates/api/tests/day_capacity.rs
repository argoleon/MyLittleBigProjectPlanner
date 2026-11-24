//! Integration tests of the capacity of resources day by day (L07-US06) on
//! real PostgreSQL and Valkey: public holidays, absences and capacity
//! periods, the resources selected (all active ones, a team, ids), pages,
//! parameter errors, who reads, and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::{AccountType, TenantRole};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const CAPACITY: &str = "/api/v1/capacity";

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

/// Creates (as tenant admin) and answers the JSON of `path`.
async fn create(api: &TestApi, tenant: &SeedTenant, path: &str, body: Value) -> Value {
    let (status, created) = send(api.request(Method::POST, tenant, path).json(&body)).await;
    assert_eq!(status, 201, "{path}: {created}");
    created
}

async fn resource(api: &TestApi, tenant: &SeedTenant, name: &str, body: Value) -> String {
    let mut fields = json!({"kind": "generic", "displayName": name});
    fields
        .as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    create(api, tenant, "/api/v1/resources", fields).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn capacity(api: &TestApi, tenant: &SeedTenant, query: &str) -> Value {
    let (status, page) = send(api.request(Method::GET, tenant, &format!("{CAPACITY}?{query}"))).await;
    assert_eq!(status, 200, "{query}: {page}");
    page
}

fn names(page: &Value) -> Vec<&str> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["displayName"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn capacity_takes_off_holidays_absences_and_follows_periods() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let paris = create(
        &api,
        a,
        "/api/v1/calendars",
        json!({"nameTranslations": {"fr": "Paris"}}),
    )
    .await;
    let camille = resource(&api, a, "Camille Martin", json!({"calendarId": paris["id"]})).await;
    create(
        &api,
        a,
        &format!("/api/v1/resources/{camille}/capacity-patterns"),
        json!({"effectiveFrom": "2026-01-01", "minutes": [420, 420, 420, 420, 210, 0, 0]}),
    )
    .await;
    // A part-time period from Wednesday 30 December.
    create(
        &api,
        a,
        &format!("/api/v1/resources/{camille}/capacity-patterns"),
        json!({"effectiveFrom": "2026-12-30", "minutes": [420, 420, 0, 420, 210, 0, 0]}),
    )
    .await;
    create(
        &api,
        a,
        &format!("/api/v1/resources/{camille}/absences"),
        json!({"kind": "leave", "start": "2026-12-28", "end": "2026-12-28"}),
    )
    .await;
    // The afternoon of Friday 18 December, a 3.5 h day.
    create(
        &api,
        a,
        &format!("/api/v1/resources/{camille}/absences"),
        json!({"kind": "training", "start": "2026-12-18", "startsAtNoon": true, "end": "2026-12-18"}),
    )
    .await;

    let page = capacity(
        &api,
        a,
        &format!("resources={camille}&from=2026-12-14&to=2026-12-31"),
    )
    .await;
    assert_eq!(
        (&page["from"], &page["to"]),
        (&json!("2026-12-14"), &json!("2026-12-31"))
    );
    let item = &page["items"][0];
    assert_eq!(item["resourceId"], json!(camille));
    assert_eq!(
        item["minutes"],
        json!([
            420, 420, 420, 420, 105, 0, 0, // 14–20: Friday afternoon off
            420, 420, 420, 420, 0, 0, 0, // 21–27: Christmas
            0, 420, 0, 420 // 28 leave, 29, then Wednesday 30 part time, 31
        ])
    );
    assert_eq!(item["totalMinutes"], json!(420 * 10 + 105));
    assert_eq!(
        item["unavailable"],
        json!([
            {"day": "2026-12-18", "reason": "absence", "part": "afternoon"},
            {"day": "2026-12-25", "reason": "holiday", "holiday": "christmas"},
            {"day": "2026-12-28", "reason": "absence", "part": "whole"},
        ])
    );
    assert_eq!(page["nextCursor"], Value::Null);

    // The days of the resource say the same.
    let (status, days) = send(api.request(
        Method::GET,
        a,
        &format!("/api/v1/resources/{camille}/days?from=2026-12-14&to=2026-12-31"),
    ))
    .await;
    assert_eq!(status, 200);
    let minutes: Vec<Value> = days
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["minutes"].clone())
        .collect();
    assert_eq!(Value::Array(minutes), item["minutes"]);
}

#[tokio::test]
async fn every_active_resource_by_default_a_team_or_named_ones_page_by_page() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let alice = resource(&api, a, "Alice", json!({})).await;
    let bruno = resource(&api, a, "Bruno", json!({})).await;
    resource(&api, a, "Chloé", json!({})).await;
    let gone = resource(&api, a, "Dan", json!({"active": false})).await;
    let team = create(
        &api,
        a,
        "/api/v1/teams",
        json!({"nameTranslations": {"fr": "Plateforme"}}),
    )
    .await;
    let team = team["id"].as_str().unwrap();
    for member in [&bruno, &gone] {
        let (status, _) =
            send(api.request(Method::PUT, a, &format!("/api/v1/teams/{team}/members/{member}"))).await;
        assert!(status < 300, "{status}");
    }
    let range = "from=2026-12-21&to=2026-12-21";

    assert_eq!(
        names(&capacity(&api, a, range).await),
        ["Alice", "Bruno", "Chloé"]
    );
    assert_eq!(
        names(&capacity(&api, a, &format!("{range}&team={team}")).await),
        ["Bruno"]
    );
    // Named ones, inactive included, whatever the order given.
    assert_eq!(
        names(&capacity(&api, a, &format!("{range}&resources={gone},{alice}")).await),
        ["Alice", "Dan"]
    );
    assert_eq!(
        names(&capacity(&api, a, &format!("{range}&team={team}&resources={gone},{alice}")).await),
        ["Dan"]
    );
    assert_eq!(
        names(&capacity(&api, a, &format!("{range}&team={}", Uuid::now_v7())).await),
        Vec::<&str>::new()
    );

    let first = capacity(&api, a, &format!("{range}&limit=2")).await;
    assert_eq!(names(&first), ["Alice", "Bruno"]);
    let cursor = first["nextCursor"].as_str().unwrap();
    let second = capacity(&api, a, &format!("{range}&limit=2&cursor={cursor}")).await;
    assert_eq!(names(&second), ["Chloé"]);
    assert_eq!(second["nextCursor"], Value::Null);
}

#[tokio::test]
async fn bad_parameters_are_refused() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let many: Vec<String> = (0..201).map(|_| Uuid::now_v7().to_string()).collect();
    for query in [
        String::new(),
        "from=2026-12-01".to_owned(),
        "from=2026-12-02&to=2026-12-01".to_owned(),
        "from=2026-01-01&to=2027-01-02".to_owned(),
        "from=2026-12-01&to=2026-12-31&limit=0".to_owned(),
        "from=2026-12-01&to=2026-12-31&limit=201".to_owned(),
        "from=2026-12-01&to=2026-12-31&resources=nope".to_owned(),
        format!("from=2026-12-01&to=2026-12-31&resources={}", many.join(",")),
        "from=2026-12-01&to=2026-12-31&cursor=bad".to_owned(),
        "from=yesterday&to=2026-12-31".to_owned(),
    ] {
        let (status, problem) = send(api.request(Method::GET, a, &format!("{CAPACITY}?{query}"))).await;
        assert_eq!(
            (status, &problem["code"]),
            (400, &json!("invalid_query")),
            "{query}: {problem}"
        );
    }
    // 366 days are fine.
    capacity(&api, a, "from=2026-01-01&to=2027-01-01").await;
}

#[tokio::test]
async fn internal_members_read_it_external_ones_and_strangers_do_not() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    resource(&api, a, "Alice", json!({})).await;
    let query = format!("{CAPACITY}?from=2026-12-21&to=2026-12-27");

    let plain = db
        .add_member_with(a, TenantRole::Member, AccountType::Internal)
        .await
        .account_id;
    let plain = session_of(&api, a, plain).await;
    let (status, page) = send(by(&api, a, &plain, Method::GET, &query)).await;
    assert_eq!((status, names(&page)), (200, vec!["Alice"]));

    let external = db
        .add_member_with(a, TenantRole::Member, AccountType::External)
        .await
        .account_id;
    let external = session_of(&api, a, external).await;
    let (status, problem) = send(by(&api, a, &external, Method::GET, &query)).await;
    assert_eq!((status, &problem["code"]), (403, &json!("forbidden")));

    let (status, problem) = send(api.anonymous(Method::GET, a, &query)).await;
    assert_eq!((status, &problem["code"]), (401, &json!("unauthenticated")));
}

#[tokio::test]
async fn a_tenant_never_reads_the_capacity_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let alice = resource(&api, &db.tenant_a, "Alice", json!({})).await;
    resource(&api, &db.tenant_b, "Zoé", json!({})).await;

    let page = capacity(&api, &db.tenant_b, "from=2026-12-21&to=2026-12-27").await;
    assert_eq!(names(&page), ["Zoé"]);
    let page = capacity(
        &api,
        &db.tenant_b,
        &format!("from=2026-12-21&to=2026-12-27&resources={alice}"),
    )
    .await;
    assert_eq!(names(&page), Vec::<&str>::new());
}
