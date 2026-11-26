//! Integration tests of the workload heatmap (L08-US06) on real PostgreSQL
//! and Valkey: load and capacity by week, day and month, provisional load
//! left out on demand, the team and node filters (the whole load of the
//! resources shown), busy time on tickets the member does not see, the
//! precomputed sums read when stored, the cell details with their tickets
//! and forced overloads, refusals and tenant isolation.

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

fn as_member(api: &TestApi, tenant: &SeedTenant, session: &Session, path: &str) -> reqwest::RequestBuilder {
    api.anonymous(Method::GET, tenant, path)
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
    assert!(status == 201 || status == 200, "{path}: {status} {created}");
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

async fn story(api: &TestApi, tenant: &SeedTenant, key: &str) {
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

struct Plan {
    program: String,
    ops: String,
    camille: String,
}

/// The program MOD with the project PLAT, and OPS apart; the team Platform
/// of Camille and Nathan, and a Rust developer. The week of 16 November
/// 2026: Camille 7 h a day on PLAT-1 and 50 min a day on OPS-1 (forced:
/// 112%), Nathan 50% on OPS-1, provisional, for two weeks.
async fn plan(api: &TestApi, a: &SeedTenant) -> Plan {
    let program = created(
        api,
        a,
        "/api/v1/nodes",
        json!({"type": "program", "name": "Modernization", "key": "MOD"}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    created(
        api,
        a,
        "/api/v1/nodes",
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": program}),
    )
    .await;
    let ops = created(
        api,
        a,
        "/api/v1/nodes",
        json!({"type": "project", "name": "Operations", "key": "OPS"}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    story(api, a, "PLAT").await;
    story(api, a, "OPS").await;
    let camille = resource(api, a, "Camille Martin").await;
    let nathan = resource(api, a, "Nathan Roux").await;
    resource(api, a, "Rust developer").await;
    let team = created(
        api,
        a,
        "/api/v1/teams",
        json!({"nameTranslations": {"fr": "Plateforme", "en": "Platform"}}),
    )
    .await;
    for who in [&camille, &nathan] {
        let (status, _) = send(admin(
            api,
            a,
            Method::PUT,
            &format!("/api/v1/teams/{}/members/{who}", team["id"].as_str().unwrap()),
        ))
        .await;
        assert!(status < 300);
    }
    created(
        api,
        a,
        "/api/v1/issues/PLAT-1/assignments",
        json!({"resourceId": camille, "segments": hours("2026-11-16", "2026-11-20", 420)}),
    )
    .await;
    created(
        api,
        a,
        "/api/v1/issues/OPS-1/assignments",
        json!({"resourceId": camille, "force": true, "segments": hours("2026-11-16", "2026-11-20", 50)}),
    )
    .await;
    created(
        api,
        a,
        "/api/v1/issues/OPS-1/assignments",
        json!({"resourceId": nathan, "provisional": true, "segments": [
            {"start": "2026-11-16", "end": "2026-11-27", "mode": "percentage", "value": 50}
        ]}),
    )
    .await;
    Plan {
        program,
        ops,
        camille,
    }
}

fn names(heatmap: &Value) -> Vec<&str> {
    heatmap["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["resource"]["displayName"].as_str().unwrap())
        .collect()
}

const WEEKS: &str = "/api/v1/workload/heatmap?scale=week&from=2026-11-16&to=2026-11-29";

#[tokio::test]
async fn the_heatmap_shows_load_against_capacity_by_week_day_and_month() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let plan = plan(&api, a).await;

    // Two weeks, every active resource by name.
    let heatmap = get(&api, a, WEEKS).await;
    assert_eq!(
        heatmap["periods"],
        json!([
            {"start": "2026-11-16", "end": "2026-11-22"},
            {"start": "2026-11-23", "end": "2026-11-29"},
        ])
    );
    assert_eq!(
        names(&heatmap),
        ["Camille Martin", "Nathan Roux", "Rust developer"]
    );
    let camille = &heatmap["rows"][0];
    // AC2: 35 h of capacity, 39 h 10 planned: 112%.
    assert_eq!(camille["load"], json!([2350, 0]));
    assert_eq!(camille["capacity"], json!([2100, 2100]));
    assert_eq!(camille["scheduled"], json!([2100, 2100]));
    assert_eq!(camille["hidden"], json!([0, 0]));
    let nathan = &heatmap["rows"][1];
    assert_eq!(
        (&nathan["load"], &nathan["provisional"]),
        (&json!([1050, 1050]), &json!([1050, 1050]))
    );
    assert_eq!(
        (&heatmap["tolerancePercent"], &heatmap["includeProvisional"]),
        (&json!(0), &json!(true))
    );

    // Provisional load left out.
    let firm = get(&api, a, &format!("{WEEKS}&includeProvisional=false")).await;
    assert_eq!(
        (&firm["rows"][1]["load"], &firm["rows"][1]["provisional"]),
        (&json!([0, 0]), &json!([0, 0]))
    );

    // AC4: the team, then the program: Camille only, with her whole load.
    let team = get(&api, a, "/api/v1/teams").await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        names(&get(&api, a, &format!("{WEEKS}&teamId={team}")).await),
        ["Camille Martin", "Nathan Roux"]
    );
    let program = get(&api, a, &format!("{WEEKS}&nodeId={}", plan.program)).await;
    assert_eq!(names(&program), ["Camille Martin"]);
    assert_eq!(program["rows"][0]["load"], json!([2350, 0]));
    assert_eq!(
        names(&get(&api, a, &format!("{WEEKS}&nodeId={}", plan.ops)).await),
        ["Camille Martin", "Nathan Roux"]
    );

    // Days: the week-end has no capacity; months: two columns.
    let days = get(
        &api,
        a,
        "/api/v1/workload/heatmap?scale=day&from=2026-11-16&to=2026-11-22",
    )
    .await;
    assert_eq!(days["periods"].as_array().unwrap().len(), 7);
    assert_eq!(days["rows"][0]["load"], json!([470, 470, 470, 470, 470, 0, 0]));
    assert_eq!(
        days["rows"][0]["scheduled"],
        json!([420, 420, 420, 420, 420, 0, 0])
    );
    let months = get(
        &api,
        a,
        "/api/v1/workload/heatmap?scale=month&from=2026-11-20&to=2026-12-05",
    )
    .await;
    assert_eq!(
        months["periods"],
        json!([
            {"start": "2026-11-01", "end": "2026-11-30"},
            {"start": "2026-12-01", "end": "2026-12-31"},
        ])
    );
    assert_eq!(months["rows"][0]["load"], json!([2350, 0]));

    // Stored sums are read as they are: computed by the worker, here forced.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::load_aggregates::recompute(&mut tx).await.unwrap();
    let changed = infra::sqlx::query(
        "UPDATE load_aggregate SET load_minutes = 2400 WHERE resource_id = $1 AND period_kind = 'week' AND period_start = '2026-11-16'",
    )
    .bind(Uuid::try_parse(&plan.camille).unwrap())
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        changed.rows_affected(),
        1,
        "the heatmap marked the weeks it computed"
    );
    tx.commit().await.unwrap();
    assert_eq!(get(&api, a, WEEKS).await["rows"][0]["load"], json!([2400, 0]));

    // Refusals.
    for query in [
        "scale=week&from=2026-11-29&to=2026-11-16",
        "scale=day&from=2026-01-01&to=2026-06-30",
        "scale=week&from=2026-01-01&to=2027-06-30",
        "scale=month&from=2026-01-01&to=2028-06-30",
        "scale=year&from=2026-01-01&to=2026-02-01",
        "from=2026-01-01",
    ] {
        let (status, body) = send(admin(
            &api,
            a,
            Method::GET,
            &format!("/api/v1/workload/heatmap?{query}"),
        ))
        .await;
        assert_eq!(status, 400, "{query}: {body}");
    }
    let (status, _) = send(admin(
        &api,
        a,
        Method::GET,
        &format!("{WEEKS}&nodeId={}", Uuid::now_v7()),
    ))
    .await;
    assert_eq!(status, 404);
    // Another tenant sees none of these resources.
    assert!(
        get(&api, &db.tenant_b, WEEKS).await["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_cell_tells_its_tickets_and_hides_those_the_member_does_not_see() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let plan = plan(&api, a).await;
    let cell = format!(
        "/api/v1/workload/cell?resourceId={}&from=2026-11-16&to=2026-11-22&nodeId={}",
        plan.camille, plan.program
    );

    // AC3: the tickets behind the 112%, the most loaded first.
    let details = get(&api, a, &cell).await;
    assert_eq!(
        (
            &details["capacityMinutes"],
            &details["loadMinutes"],
            &details["resource"]["displayName"]
        ),
        (&json!(2100), &json!(2350), &json!("Camille Martin"))
    );
    let items = details["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (
            &items[0]["issue"]["key"],
            &items[0]["minutes"],
            &items[0]["inNode"],
            &items[0]["issue"]["project"]["key"]
        ),
        (&json!("PLAT-1"), &json!(2100), &json!(true), &json!("PLAT"))
    );
    assert_eq!(
        items[0]["segments"],
        json!([{"mode": "hours_per_day", "value": 420}])
    );
    assert_eq!(
        (&items[1]["issue"]["key"], &items[1]["inNode"]),
        (&json!("OPS-1"), &json!(false))
    );
    // The overload forced on OPS-1.
    assert_eq!(details["forced"].as_array().unwrap().len(), 1);
    assert_eq!(details["forced"][0]["issueKey"], "OPS-1");
    assert!(details["forced"][0]["actorName"].is_string());

    // AC5: a member who reads OPS only: PLAT-1 is busy time, without name.
    let (account, session) = member(&db, &api, a).await;
    let reader = get(&api, a, "/api/v1/tenant/roles").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["labelTranslations"]["en"] == "Reader")
        .unwrap()["id"]
        .clone();
    created(
        &api,
        a,
        &format!("/api/v1/nodes/{}/grants", plan.ops),
        json!({"roleId": reader, "accountId": account}),
    )
    .await;
    let mine = format!(
        "/api/v1/workload/cell?resourceId={}&from=2026-11-16&to=2026-11-22",
        plan.camille
    );
    let (status, seen) = send(as_member(&api, a, &session, &mine)).await;
    assert_eq!(status, 200, "{seen}");
    let items = seen["items"].as_array().unwrap();
    assert_eq!(items[0]["issue"]["key"], "OPS-1");
    assert_eq!(
        (&items[1]["issue"], &items[1]["minutes"], &items[1]["segments"]),
        (&Value::Null, &json!(2100), &json!([]))
    );
    assert_eq!(seen["loadMinutes"], 2350, "busy time counts");
    // The heatmap tells the same busy part, and the program is not found.
    let (status, heatmap) = send(as_member(&api, a, &session, WEEKS)).await;
    assert_eq!(status, 200);
    assert_eq!(heatmap["rows"][0]["hidden"], json!([2100, 0]));
    assert_eq!(heatmap["rows"][0]["load"], json!([2350, 0]));
    let (status, _) = send(as_member(&api, a, &session, &cell)).await;
    assert_eq!(status, 404, "a node the member does not see");

    // Refusals.
    for query in [
        format!("resourceId={}&from=2026-11-22&to=2026-11-16", plan.camille),
        format!("resourceId={}&from=2026-01-01&to=2027-06-30", plan.camille),
    ] {
        let (status, _) = send(admin(
            &api,
            a,
            Method::GET,
            &format!("/api/v1/workload/cell?{query}"),
        ))
        .await;
        assert_eq!(status, 400, "{query}");
    }
    let (status, _) = send(admin(
        &api,
        a,
        Method::GET,
        &format!(
            "/api/v1/workload/cell?resourceId={}&from=2026-11-16&to=2026-11-22",
            Uuid::now_v7()
        ),
    ))
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(admin(&api, &db.tenant_b, Method::GET, &mine)).await;
    assert_eq!(status, 404, "another tenant's resource");
}
