//! Integration tests of load against capacity by node and team (L08-US07)
//! on real PostgreSQL and Valkey: a program sums its distinct projects, a
//! project reached twice once, a link weighted at 50% for half, a
//! confidential one without detail; higher up the weighting no longer
//! applies; a team sums its members; visibility, refusals, isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

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

async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    created(api, tenant, "/api/v1/nodes", body).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A story in project `key`, Nathan on it at `percent` the week of 16 November 2026.
async fn work(api: &TestApi, tenant: &SeedTenant, key: &str, resource: &str, percent: u32) {
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
    created(
        api,
        tenant,
        &format!("/api/v1/issues/{key}-1/assignments"),
        json!({"resourceId": resource, "segments": [
            {"start": "2026-11-16", "end": "2026-11-20", "mode": "percentage", "value": percent}
        ]}),
    )
    .await;
}

fn first_period(aggregate: &Value) -> (&Value, &Value, &Value) {
    let period = &aggregate["periods"][0];
    (
        &period["loadMinutes"],
        &period["capacityMinutes"],
        &period["usedPercent"],
    )
}

#[tokio::test]
async fn nodes_and_teams_sum_load_and_capacity_by_period() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    // The portfolio, its program, a sub-program; PLAT for half of the
    // program, BILL under both programs, SEC confidential.
    let portfolio = node(
        &api,
        a,
        json!({"type": "portfolio", "name": "Digital", "key": "DIGITAL"}),
    )
    .await;
    let program = node(
        &api,
        a,
        json!({"type": "program", "name": "Modernization", "key": "MOD", "parentId": portfolio}),
    )
    .await;
    let sub = node(
        &api,
        a,
        json!({"type": "program", "name": "Billing wave", "key": "WAVE", "parentId": program}),
    )
    .await;
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Platform", "key": "PLAT", "parentId": program}),
    )
    .await;
    let bill = node(
        &api,
        a,
        json!({"type": "project", "name": "Billing", "key": "BILL", "parentId": sub}),
    )
    .await;
    let sec = node(
        &api,
        a,
        json!({"type": "project", "name": "Merger", "key": "SEC", "parentId": program}),
    )
    .await;
    for (path, weight) in [
        (format!("/api/v1/nodes/{plat}/parents/{program}"), 50),
        (format!("/api/v1/nodes/{bill}/parents/{program}"), 100),
    ] {
        let (status, body) =
            send(admin(&api, a, Method::PUT, &path).json(&json!({"weightPercent": weight}))).await;
        assert!(status < 300, "{path}: {body}");
    }
    let (status, _) = send(
        admin(
            &api,
            a,
            Method::PUT,
            &format!("/api/v1/nodes/{sec}/confidentiality"),
        )
        .header("if-match", "*")
        .json(&json!({"confidential": true})),
    )
    .await;
    assert!(status < 300);
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
    let camille = created(
        &api,
        a,
        "/api/v1/resources",
        json!({"kind": "generic", "displayName": "Camille Martin"}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // 40% on PLAT, 40% on BILL, 20% on SEC: 14 h, 14 h and 7 h of 35 h.
    work(&api, a, "PLAT", &nathan, 40).await;
    work(&api, a, "BILL", &nathan, 40).await;
    work(&api, a, "SEC", &nathan, 20).await;
    // The worker's job, run here.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::load_aggregates::recompute(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let week = "scale=week&from=2026-11-16&to=2026-11-22";

    // AC1, AC2: the program counts BILL once, PLAT for half, SEC without detail.
    let mod_week = get(&api, a, &format!("/api/v1/nodes/{program}/workload?{week}")).await;
    assert_eq!(
        first_period(&mod_week),
        (&json!(420 + 840 + 420), &json!(420 + 840 + 420), &json!(100))
    );
    assert_eq!(
        (&mod_week["count"], &mod_week["confidentialProjects"]),
        (&json!(3), &json!(1))
    );
    assert_eq!(mod_week["periods"][0]["start"], "2026-11-16");
    // Higher up, PLAT counts whole: the portfolio has all of Nathan's week.
    let portfolio_week = get(&api, a, &format!("/api/v1/nodes/{portfolio}/workload?{week}")).await;
    assert_eq!(
        first_period(&portfolio_week),
        (&json!(2100), &json!(2100), &json!(100))
    );
    // A project alone.
    let plat_week = get(&api, a, &format!("/api/v1/nodes/{plat}/workload?{week}")).await;
    assert_eq!(first_period(&plat_week), (&json!(840), &json!(840), &json!(100)));
    // By month (the default): November.
    let months = get(
        &api,
        a,
        &format!("/api/v1/nodes/{program}/workload?from=2026-11-01&to=2026-12-31"),
    )
    .await;
    assert_eq!(months["scale"], "month");
    assert_eq!(months["periods"].as_array().unwrap().len(), 2);
    assert_eq!(months["periods"][0]["loadMinutes"], 1680);
    assert_eq!(
        (
            &months["periods"][1]["loadMinutes"],
            &months["periods"][1]["usedPercent"]
        ),
        (&json!(0), &Value::Null)
    );

    // AC3: a team sums its members over the month: 20 working days each.
    let team = created(
        &api,
        a,
        "/api/v1/teams",
        json!({"nameTranslations": {"fr": "Plateforme", "en": "Platform"}}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for who in [&nathan, &camille] {
        let (status, _) = send(admin(
            &api,
            a,
            Method::PUT,
            &format!("/api/v1/teams/{team}/members/{who}"),
        ))
        .await;
        assert!(status < 300);
    }
    let team_month = get(
        &api,
        a,
        &format!("/api/v1/teams/{team}/workload?from=2026-11-01&to=2026-11-30"),
    )
    .await;
    assert_eq!(
        first_period(&team_month),
        (&json!(2100), &json!(2 * 20 * 420), &json!(13))
    );
    assert_eq!(team_month["count"], 2);

    // Refusals; a member who sees nothing finds no node; another tenant neither.
    for query in [
        "scale=week&from=2026-11-22&to=2026-11-16",
        "scale=day&from=2026-11-16&to=2026-11-22",
        "scale=week&from=2026-01-01&to=2027-06-30",
    ] {
        let (status, body) = send(admin(
            &api,
            a,
            Method::GET,
            &format!("/api/v1/nodes/{program}/workload?{query}"),
        ))
        .await;
        assert_eq!(status, 400, "{query}: {body}");
    }
    let member = db.add_member(a, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(member, a.id)
    .await
    .unwrap();
    let (status, _) = send(
        api.anonymous(
            Method::GET,
            a,
            &format!("/api/v1/nodes/{program}/workload?{week}"),
        )
        .header("cookie", format!("mlbpp_session={}", opened.token)),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(admin(
        &api,
        &db.tenant_b,
        Method::GET,
        &format!("/api/v1/nodes/{program}/workload?{week}"),
    ))
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(admin(
        &api,
        &db.tenant_b,
        Method::GET,
        &format!("/api/v1/teams/{team}/workload?{week}"),
    ))
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(admin(
        &api,
        a,
        Method::GET,
        &format!("/api/v1/teams/{}/workload?{week}", Uuid::now_v7()),
    ))
    .await;
    assert_eq!(status, 404);
}
