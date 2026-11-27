//! Write performance of the overload check (L08-US03, specification §19:
//! writes under 500 ms at the 95th percentile): a resource loaded by 20
//! tickets over a year, then 20 changes of a year-long assignment, each
//! checked against every day. Alone in its test binary, so that cargo runs
//! it after the others, not alongside them. The build is unoptimized: a
//! pass here is a conservative one.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::{TestApi, json, multi};
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

async fn created(api: &TestApi, tenant: &SeedTenant, path: &str, body: Value) -> Value {
    let (status, created) = send(admin(api, tenant, Method::POST, path).json(&body)).await;
    assert_eq!(status, 201, "{path}: {created}");
    created
}

fn segment(start: &str, end: &str, mode: &str, value: u32) -> Value {
    json!({"start": start, "end": end, "mode": mode, "value": value})
}

fn assignments(issue: &str) -> String {
    format!("/api/v1/issues/{issue}/assignments")
}

#[tokio::test]
async fn writes_on_a_loaded_resource_stay_under_half_a_second() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    created(
        &api,
        a,
        "/api/v1/nodes",
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let nathan = created(
        &api,
        a,
        "/api/v1/resources",
        json!({"kind": "generic", "displayName": "Nathan Roux"}),
    )
    .await["id"]
        .clone();
    let (_, form) = send(admin(&api, a, Method::GET, "/api/v1/projects/PLAT/issue-form")).await;
    let story = form["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "story")
        .unwrap()["id"]
        .clone();
    for i in 0..20 {
        let issue = created(
            &api,
            a,
            "/api/v1/projects/PLAT/issues",
            json!({"typeId": story, "summary": format!("Load {i}")}),
        )
        .await;
        created(
            &api,
            a,
            &assignments(issue["key"].as_str().unwrap()),
            json!({"resourceId": nathan, "segments": [segment("2027-01-01", "2027-12-31", "hours_per_day", 15)]}),
        )
        .await;
    }
    let checked = created(
        &api,
        a,
        "/api/v1/projects/PLAT/issues",
        json!({"typeId": story, "summary": "Checked"}),
    )
    .await;
    let assignment = created(
        &api,
        a,
        &assignments(checked["key"].as_str().unwrap()),
        json!({"resourceId": nathan, "segments": [segment("2027-01-01", "2027-12-31", "percentage", 10)]}),
    )
    .await;
    let path = format!("/api/v1/assignments/{}", assignment["id"].as_str().unwrap());
    let mut timings = Vec::new();
    for i in 0..20u32 {
        let started = Instant::now();
        let (status, body) = send(
            admin(&api, a, Method::PUT, &path)
                .header("if-match", "*")
                .json(&json!({
                    "segments": [segment("2027-01-01", "2027-12-31", "percentage", 10 + i % 5)],
                })),
        )
        .await;
        timings.push(started.elapsed());
        assert_eq!(status, 200, "{body}");
    }
    timings.sort_unstable();
    let p95 = timings[timings.len() * 95 / 100 - 1];
    let target = Duration::from_millis(500);
    assert!(p95 < target, "p95 {p95:?} ≥ {target:?}: {timings:?}");
}
