//! Integration tests of calendars (L07-US04) on real PostgreSQL and Valkey:
//! the tenant calendar with French holidays, a site and its resources, the
//! exceptions of each level, the days of a resource with their capacity,
//! who manages and who reads, and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const CALENDARS: &str = "/api/v1/calendars";

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

/// The tenant calendar of `tenant`.
async fn tenant_calendar(api: &TestApi, tenant: &SeedTenant) -> Value {
    let (status, list) = send(
        api.request(Method::GET, tenant, CALENDARS)
            .header("accept-language", "en"),
    )
    .await;
    assert_eq!(status, 200, "{list}");
    list["items"][0].clone()
}

/// A resource of `tenant`, on the site `site` if any.
async fn resource(api: &TestApi, tenant: &SeedTenant, site: Option<&str>) -> String {
    let (status, created) = send(
        api.request(Method::POST, tenant, "/api/v1/resources")
            .json(&json!({"kind": "person", "displayName": "Camille Martin", "calendarId": site})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    created["id"].as_str().unwrap().to_owned()
}

/// The days of `resource` from `from` to `to`: (day, status, minutes).
async fn days(api: &TestApi, tenant: &SeedTenant, resource: &str, from: &str, to: &str) -> Vec<Value> {
    let (status, days) = send(api.request(
        Method::GET,
        tenant,
        &format!("/api/v1/resources/{resource}/days?from={from}&to={to}"),
    ))
    .await;
    assert_eq!(status, 200, "{days}");
    days.as_array().unwrap().clone()
}

#[tokio::test]
async fn a_new_tenant_has_its_calendar_with_french_holidays() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let tenant = tenant_calendar(&api, a).await;
    assert_eq!(
        (&tenant["kind"], &tenant["name"], &tenant["holidaySet"]),
        (&json!("tenant"), &json!("Organization"), &json!("fr"))
    );
    let (status, year) = send(api.request(
        Method::GET,
        a,
        &format!("{CALENDARS}/{}/year?year=2026", tenant["id"].as_str().unwrap()),
    ))
    .await;
    assert_eq!(status, 200, "{year}");
    assert_eq!(year["holidays"].as_array().unwrap().len(), 11);
    assert_eq!(
        year["holidays"][10],
        json!({"day": "2026-12-25", "name": "christmas"})
    );
    assert_eq!(
        (&year["holidaySet"], &year["inherited"]),
        (&json!("fr"), &json!(false))
    );
    let (status, _) = send(api.request(
        Method::GET,
        a,
        &format!("{CALENDARS}/{}/year?year=1800", tenant["id"].as_str().unwrap()),
    ))
    .await;
    assert_eq!(status, 400);

    // The tenant calendar is never deleted.
    let (status, problem) = send(
        api.request(
            Method::DELETE,
            a,
            &format!("{CALENDARS}/{}", tenant["id"].as_str().unwrap()),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!((status, &problem["code"]), (409, &json!("calendar.tenant")));
}

#[tokio::test]
async fn a_resource_on_the_paris_site_does_not_work_christmas_nor_the_closures() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (status, paris) = send(
        api.request(Method::POST, a, CALENDARS)
            .json(&json!({"nameTranslations": {"fr": "Paris"}})),
    )
    .await;
    assert_eq!(status, 201, "{paris}");
    assert_eq!(
        (&paris["kind"], &paris["holidaySet"]),
        (&json!("site"), &Value::Null)
    );
    let site = paris["id"].as_str().unwrap();
    let id = resource(&api, a, Some(site)).await;
    let (_, read) = send(api.request(Method::GET, a, &format!("/api/v1/resources/{id}"))).await;
    assert_eq!(read["site"], json!({"id": site, "name": "Paris"}));

    // Week of Christmas 2026: Friday 25 is a holiday (inherited set).
    let week = days(&api, a, &id, "2026-12-21", "2026-12-27").await;
    let summary: Vec<(&str, &str, u64)> = week
        .iter()
        .map(|d| {
            (
                d["day"].as_str().unwrap(),
                d["status"].as_str().unwrap(),
                d["minutes"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("2026-12-21", "working", 420),
            ("2026-12-22", "working", 420),
            ("2026-12-23", "working", 420),
            ("2026-12-24", "working", 420),
            ("2026-12-25", "holiday", 0),
            ("2026-12-26", "working", 0),
            ("2026-12-27", "working", 0),
        ]
    );
    assert_eq!(
        (&week[4]["holiday"], &week[4]["source"]),
        (&json!("christmas"), &json!("tenant"))
    );

    // The tenant closes on the 24th; the resource is off on the 23rd.
    let tenant = tenant_calendar(&api, a).await;
    let (status, closure) = send(
        api.request(
            Method::POST,
            a,
            &format!("{CALENDARS}/{}/exceptions", tenant["id"].as_str().unwrap()),
        )
        .json(&json!({"day": "2026-12-24", "kind": "non_working", "label": "Closed"})),
    )
    .await;
    assert_eq!(status, 201, "{closure}");
    let (status, _) = send(
        api.request(Method::POST, a, &format!("/api/v1/resources/{id}/exceptions"))
            .json(&json!({"day": "2026-12-23", "kind": "non_working", "label": "Training"})),
    )
    .await;
    assert_eq!(status, 201);
    // Paris works the 24th anyway: the site wins over the tenant.
    let (status, _) = send(
        api.request(Method::POST, a, &format!("{CALENDARS}/{site}/exceptions"))
            .json(&json!({"day": "2026-12-24", "kind": "working"})),
    )
    .await;
    assert_eq!(status, 201);
    let week = days(&api, a, &id, "2026-12-23", "2026-12-24").await;
    assert_eq!(
        (
            &week[0]["status"],
            &week[0]["source"],
            &week[0]["label"],
            &week[0]["minutes"]
        ),
        (
            &json!("non_working"),
            &json!("resource"),
            &json!("Training"),
            &json!(0)
        )
    );
    assert_eq!(
        (&week[1]["status"], &week[1]["source"], &week[1]["minutes"]),
        (&json!("working"), &json!("site"), &json!(420))
    );

    // One exception per day.
    let (status, problem) = send(
        api.request(Method::POST, a, &format!("{CALENDARS}/{site}/exceptions"))
            .json(&json!({"day": "2026-12-24", "kind": "non_working"})),
    )
    .await;
    assert_eq!(
        (status, &problem["code"]),
        (409, &json!("calendar_exception.day_taken"))
    );

    // A site with the Alsace-Moselle set: 26 December off too.
    let (status, changed) = send(
        api.request(Method::PUT, a, &format!("{CALENDARS}/{site}"))
            .header("if-match", "*")
            .json(&json!({"nameTranslations": {"fr": "Strasbourg"}, "holidaySet": "fr-alsace-moselle"})),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    let day = days(&api, a, &id, "2026-12-28", "2026-12-28").await;
    assert_eq!(day[0]["status"], "working");
    let (status, year) =
        send(api.request(Method::GET, a, &format!("{CALENDARS}/{site}/year?year=2026"))).await;
    assert_eq!((status, year["holidays"].as_array().unwrap().len()), (200, 13));
}

#[tokio::test]
async fn exceptions_change_with_their_version_and_ranges_are_bounded() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let id = resource(&api, a, None).await;
    let path = format!("/api/v1/resources/{id}/exceptions");
    let (status, created) = send(
        api.request(Method::POST, a, &path)
            .json(&json!({"day": "2026-05-25", "kind": "working", "label": " Worked "})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["label"], "Worked");
    // Whit Monday worked: its capacity is back.
    assert_eq!(
        days(&api, a, &id, "2026-05-25", "2026-05-25").await[0]["minutes"],
        420
    );

    let one = format!("{path}/{}", created["id"].as_str().unwrap());
    let body = json!({"day": "2026-05-26", "kind": "holiday", "label": "Local fair"});
    let (status, _) = send(api.request(Method::PUT, a, &one).json(&body)).await;
    assert_eq!(status, 428);
    let (status, _) = send(
        api.request(Method::PUT, a, &one)
            .header("if-match", "\"5\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let (status, moved) = send(
        api.request(Method::PUT, a, &one)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!((status, &moved["version"]), (200, &json!(2)));
    let (status, list) =
        send(api.request(Method::GET, a, &format!("{path}?from=2026-01-01&to=2026-12-31"))).await;
    assert_eq!(
        (
            status,
            list["items"].as_array().unwrap().len(),
            &list["canManage"]
        ),
        (200, 1, &json!(true))
    );
    let (status, _) = send(api.request(Method::DELETE, a, &one).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204);

    for query in [
        "from=2026-02-01&to=2026-01-01",
        "from=2026-01-01&to=2027-06-01",
        "from=nonsense&to=2026-01-01",
    ] {
        let (status, _) =
            send(api.request(Method::GET, a, &format!("/api/v1/resources/{id}/days?{query}"))).await;
        assert_eq!(status, 400, "{query}");
    }
    let (status, problem) = send(
        api.request(Method::POST, a, &path)
            .json(&json!({"day": "2026-05-27", "kind": "holiday", "label": "x".repeat(101)})),
    )
    .await;
    assert_eq!((status, &problem["errors"][0]["field"]), (400, &json!("label")));
    let tenant = tenant_calendar(&api, a).await;
    let (status, problem) = send(
        api.request(Method::PUT, a, &format!("/api/v1/resources/{id}"))
            .header("if-match", "*")
            .json(&json!({"kind": "person", "displayName": "Camille", "calendarId": tenant["id"]})),
    )
    .await;
    assert_eq!(
        (status, &problem["errors"][0]),
        (400, &json!({"field": "calendarId", "code": "not_a_site"}))
    );
}

#[tokio::test]
async fn resource_managers_manage_calendars_and_members_read_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let manager = session_of(
        &api,
        a,
        db.add_member(a, TenantRole::ResourceManager).await.account_id,
    )
    .await;
    let plain = session_of(&api, a, db.add_member(a, TenantRole::Member).await.account_id).await;
    let (status, site) = send(
        by(&api, a, &manager, Method::POST, CALENDARS).json(&json!({"nameTranslations": {"fr": "Lyon"}})),
    )
    .await;
    assert_eq!(status, 201, "{site}");
    let site_id = site["id"].as_str().unwrap();
    let id = resource(&api, a, Some(site_id)).await;

    let (status, list) = send(by(&api, a, &plain, Method::GET, CALENDARS)).await;
    assert_eq!(
        (
            status,
            list["items"].as_array().unwrap().len(),
            &list["canManage"]
        ),
        (200, 2, &json!(false))
    );
    let (status, _) = send(by(
        &api,
        a,
        &plain,
        Method::GET,
        &format!("/api/v1/resources/{id}/days?from=2026-01-01&to=2026-01-31"),
    ))
    .await;
    assert_eq!(status, 200);
    for request in [
        by(&api, a, &plain, Method::POST, CALENDARS).json(&json!({"nameTranslations": {"fr": "Nantes"}})),
        by(&api, a, &plain, Method::PUT, &format!("{CALENDARS}/{site_id}"))
            .header("if-match", "*")
            .json(&json!({"nameTranslations": {"fr": "X"}})),
        by(&api, a, &plain, Method::DELETE, &format!("{CALENDARS}/{site_id}")).header("if-match", "*"),
        by(
            &api,
            a,
            &plain,
            Method::POST,
            &format!("{CALENDARS}/{site_id}/exceptions"),
        )
        .json(&json!({"day": "2026-01-02", "kind": "non_working"})),
        by(
            &api,
            a,
            &plain,
            Method::POST,
            &format!("/api/v1/resources/{id}/exceptions"),
        )
        .json(&json!({"day": "2026-01-02", "kind": "non_working"})),
    ] {
        let (status, problem) = send(request).await;
        assert_eq!((status, &problem["code"]), (403, &json!("forbidden")));
    }
    // Deleted, the site leaves its resource on the tenant calendar.
    let (status, _) = send(
        by(
            &api,
            a,
            &manager,
            Method::DELETE,
            &format!("{CALENDARS}/{site_id}"),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    let (_, read) = send(api.request(Method::GET, a, &format!("/api/v1/resources/{id}"))).await;
    assert_eq!(read["site"], Value::Null);
}

#[tokio::test]
async fn a_tenant_never_reaches_the_calendars_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let alpha = tenant_calendar(&api, a).await;
    let path = format!("{CALENDARS}/{}", alpha["id"].as_str().unwrap());
    let id = resource(&api, a, None).await;
    for (method, path) in [
        (Method::GET, path.clone()),
        (Method::GET, format!("{path}/year?year=2026")),
        (
            Method::GET,
            format!("/api/v1/resources/{id}/days?from=2026-01-01&to=2026-01-31"),
        ),
        (
            Method::GET,
            format!("/api/v1/resources/{id}/exceptions?from=2026-01-01&to=2026-01-31"),
        ),
    ] {
        let (status, _) = send(api.request(method, b, &path)).await;
        assert_eq!(status, 404, "{path}");
    }
    let (status, _) = send(
        api.request(Method::POST, b, &format!("{path}/exceptions"))
            .json(&json!({"day": "2026-01-02", "kind": "non_working"})),
    )
    .await;
    assert_eq!(status, 404);
}
