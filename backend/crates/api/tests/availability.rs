//! Integration tests of the availability search (L08-US10) on real
//! PostgreSQL and Valkey: free hours and free days over a period, by team,
//! sorted by free time; absences deducted; provisional load counted or
//! not; load on a confidential project counting without being named;
//! refusals and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
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

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{path}: {body}");
    body
}

async fn created(api: &TestApi, tenant: &SeedTenant, path: &str, body: Value) -> Value {
    let (status, created) = send(admin(api, tenant, Method::POST, path).json(&body)).await;
    assert!(status == 201 || status == 200, "{path}: {status} {created}");
    created
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (infra::uuid::Uuid, Session) {
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

async fn project(api: &TestApi, tenant: &SeedTenant, key: &str, name: &str) -> String {
    let id = created(
        api,
        tenant,
        "/api/v1/nodes",
        json!({"type": "project", "name": name, "key": key}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
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
    id
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

fn hours(start: &str, end: &str, minutes: u32) -> Value {
    json!([{"start": start, "end": end, "mode": "hours_per_day", "value": minutes}])
}

/// The week of 16 November 2026 (no public holiday), 7 h a day:
/// - Camille, 4 h a day Monday to Wednesday on PLAT-1: 23 h free, 2 free days;
/// - Nathan, 7 h provisional Monday and Tuesday on PLAT-1: 21 h free and 3
///   free days with provisional load, 35 h and 5 days without;
/// - the Rust developer, on leave Thursday and 2 h Friday on the
///   confidential SEC-1: 26 h free, 3 free days;
/// - Alice, nothing planned, outside the team Platform of the three others.
///
/// The team's id.
async fn plan(api: &TestApi, a: &SeedTenant) -> String {
    project(api, a, "PLAT", "Identity platform").await;
    let secret = project(api, a, "SEC", "Secret acquisition").await;
    let (status, _) = send(
        admin(
            api,
            a,
            Method::PUT,
            &format!("/api/v1/nodes/{secret}/confidentiality"),
        )
        .header("if-match", "*")
        .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200);
    let camille = resource(api, a, "Camille Martin").await;
    let nathan = resource(api, a, "Nathan Roux").await;
    let rust = resource(api, a, "Rust developer").await;
    resource(api, a, "Alice Bernard").await;
    let team = created(
        api,
        a,
        "/api/v1/teams",
        json!({"nameTranslations": {"fr": "Plateforme", "en": "Platform"}}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for who in [&camille, &nathan, &rust] {
        let (status, _) = send(admin(
            api,
            a,
            Method::PUT,
            &format!("/api/v1/teams/{team}/members/{who}"),
        ))
        .await;
        assert!(status < 300);
    }
    created(
        api,
        a,
        "/api/v1/issues/PLAT-1/assignments",
        json!({"resourceId": camille, "segments": hours("2026-11-16", "2026-11-18", 240)}),
    )
    .await;
    created(
        api,
        a,
        "/api/v1/issues/PLAT-1/assignments",
        json!({"resourceId": nathan, "provisional": true, "segments": hours("2026-11-16", "2026-11-17", 420)}),
    )
    .await;
    created(
        api,
        a,
        &format!("/api/v1/resources/{rust}/absences"),
        json!({"kind": "leave", "start": "2026-11-19", "end": "2026-11-19"}),
    )
    .await;
    created(
        api,
        a,
        "/api/v1/issues/SEC-1/assignments",
        json!({"resourceId": rust, "segments": hours("2026-11-20", "2026-11-20", 120)}),
    )
    .await;
    team
}

const WEEK: &str = "/api/v1/workload/availability?from=2026-11-16&to=2026-11-22";

/// Name, free minutes and free days of each resource found.
fn found(page: &Value) -> Vec<(String, u64, u64)> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["resource"]["displayName"].as_str().unwrap().to_owned(),
                item["freeMinutes"].as_u64().unwrap(),
                item["freeDays"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn row(name: &str, free_minutes: u64, free_days: u64) -> (String, u64, u64) {
    (name.to_owned(), free_minutes, free_days)
}

#[tokio::test]
async fn resources_with_free_days_or_hours_are_found_most_free_first() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let team = plan(&api, a).await;

    // AC1 and AC3: team Platform, 3 free days; the leave and the
    // provisional days taken out, Camille has only 2.
    let page = get(&api, a, &format!("{WEEK}&teamId={team}&minDays=3")).await;
    assert_eq!(
        found(&page),
        [row("Rust developer", 1560, 3), row("Nathan Roux", 1260, 3)]
    );
    assert_eq!(
        (&page["from"], &page["to"], &page["includeProvisional"]),
        (&json!("2026-11-16"), &json!("2026-11-22"), &json!(true))
    );
    let rust = &page["items"][0];
    assert_eq!(
        (
            &rust["capacityMinutes"],
            &rust["loadMinutes"],
            &rust["workingDays"]
        ),
        (&json!(1680), &json!(120), &json!(4))
    );
    assert_eq!(rust["resource"]["kind"], "generic");

    // AC2: 22 free hours, anyone: by free hours, most first.
    let page = get(&api, a, &format!("{WEEK}&minHours=22")).await;
    assert_eq!(
        found(&page),
        [
            row("Alice Bernard", 2100, 5),
            row("Rust developer", 1560, 3),
            row("Camille Martin", 1380, 2)
        ]
    );
    // No need: everyone, still by free hours.
    assert_eq!(found(&get(&api, a, WEEK).await).len(), 4);

    // Provisional load left out: Nathan is free all week.
    let page = get(
        &api,
        a,
        &format!("{WEEK}&teamId={team}&minDays=5&includeProvisional=false"),
    )
    .await;
    assert_eq!(found(&page), [row("Nathan Roux", 2100, 5)]);
    assert_eq!(page["includeProvisional"], false);
}

#[tokio::test]
async fn load_on_a_confidential_project_counts_without_being_named() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let team = plan(&api, a).await;

    // AC4: a member who reads nothing of SEC.
    let (_, session) = member(&db, &api, a).await;
    let path = format!("{WEEK}&teamId={team}&minDays=3");
    let (status, page) = send(
        api.anonymous(Method::GET, a, &path)
            .header("accept-language", "en")
            .header("cookie", format!("mlbpp_session={}", session.0))
            .header("x-csrf-token", &session.1),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(
        found(&page),
        [row("Rust developer", 1560, 3), row("Nathan Roux", 1260, 3)]
    );
    let text = page.to_string();
    assert!(!text.contains("SEC") && !text.contains("Secret"), "{text}");
}

#[tokio::test]
async fn bad_searches_are_refused_and_tenants_are_apart() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plan(&api, a).await;

    for (query, field, code) in [
        (
            "from=2026-11-16&to=2026-11-22&minHours=10&minDays=2",
            "minDays",
            "not_allowed",
        ),
        (
            "from=2026-11-16&to=2026-11-22&minHours=0",
            "minHours",
            "out_of_range",
        ),
        (
            "from=2026-11-16&to=2026-11-22&minDays=93",
            "minDays",
            "out_of_range",
        ),
        ("from=2026-11-22&to=2026-11-16", "to", "before_start"),
        ("from=2026-01-01&to=2026-06-30", "to", "out_of_range"),
    ] {
        let (status, problem) = send(admin(
            &api,
            a,
            Method::GET,
            &format!("/api/v1/workload/availability?{query}"),
        ))
        .await;
        assert_eq!(status, 400, "{query}: {problem}");
        assert_eq!(problem["code"], "validation_failed", "{query}");
        assert_eq!(
            (&problem["errors"][0]["field"], &problem["errors"][0]["code"]),
            (&json!(field), &json!(code)),
            "{query}"
        );
    }
    let (status, problem) = send(admin(
        &api,
        a,
        Method::GET,
        "/api/v1/workload/availability?from=2026-11-16",
    ))
    .await;
    assert_eq!(status, 400, "{problem}");
    let (status, _) = send(api.anonymous(Method::GET, a, WEEK)).await;
    assert_eq!(status, 401);

    // Tenant B sees none of tenant A's resources.
    let names: Vec<String> = found(&get(&api, &db.tenant_b, WEEK).await)
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    for name in ["Camille Martin", "Nathan Roux", "Rust developer", "Alice Bernard"] {
        assert!(!names.contains(&name.to_owned()), "{names:?}");
    }
}
