//! Integration tests of the status and type mapping wizard (L04-US08) on
//! real PostgreSQL and Valkey: the plan of what a draft removes with its
//! targets and suggestions, a mapping refused while incomplete, a migration
//! started, followed and run (as the worker runs it) until the draft is
//! published, one at a time; who may do it and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

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

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn config(api: &TestApi, tenant: &SeedTenant, en: &str) -> String {
    let body = json!({"nameTranslations": {"en": en}});
    let (status, config) =
        send(admin(api, tenant, Method::POST, "/api/v1/project-configs").json(&body)).await;
    assert_eq!(status, 201, "{config}");
    config["id"].as_str().unwrap().to_owned()
}

fn by<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
}

/// A draft without "In review", with "In acceptance", and without "Bug".
async fn draft_removing_in_review_and_bug(api: &TestApi, t: &SeedTenant, id: &str) -> (Value, Value) {
    let base = format!("/api/v1/project-configs/{id}");
    let workflow = get(api, t, &format!("{base}/workflows")).await["items"][0].clone();
    let review = by(&workflow["statuses"], "key", "in_review")["id"].clone();
    let body = json!({"key": "in_acceptance", "labelTranslations": {"en": "In acceptance"}, "category": "in_progress"});
    let (status, _) = send(
        admin(
            api,
            t,
            Method::POST,
            &format!("{base}/workflows/{}/statuses", workflow["id"].as_str().unwrap()),
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201);
    let (status, _) = send(
        admin(
            api,
            t,
            Method::DELETE,
            &format!("{base}/workflow-statuses/{}", review.as_str().unwrap()),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    // The standard workflow stays publishable: Validate was in review's, so
    // Done is now reached from In acceptance.
    let workflow = get(api, t, &format!("{base}/workflows")).await["items"][0].clone();
    let body = json!({
        "nameTranslations": {"en": "Accept"},
        "fromId": by(&workflow["statuses"], "key", "in_progress")["id"],
        "toId": by(&workflow["statuses"], "key", "in_acceptance")["id"],
    });
    let (status, _) = send(
        admin(
            api,
            t,
            Method::POST,
            &format!(
                "{base}/workflows/{}/transitions",
                workflow["id"].as_str().unwrap()
            ),
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201);
    let body = json!({
        "nameTranslations": {"en": "Finish"},
        "fromId": by(&workflow["statuses"], "key", "in_acceptance")["id"],
        "toId": by(&workflow["statuses"], "key", "done")["id"],
    });
    let (status, _) = send(
        admin(
            api,
            t,
            Method::POST,
            &format!(
                "{base}/workflows/{}/transitions",
                workflow["id"].as_str().unwrap()
            ),
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201);
    let types = get(api, t, &format!("{base}/issue-types")).await["items"].clone();
    let bug = by(&types, "key", "bug")["id"].clone();
    let (status, _) = send(
        admin(
            api,
            t,
            Method::DELETE,
            &format!("{base}/issue-types/{}", bug.as_str().unwrap()),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    (review, bug)
}

#[tokio::test]
async fn removed_statuses_and_types_are_mapped_then_migrated_and_the_draft_published() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let base = format!("/api/v1/project-configs/{id}");
    let (review, bug) = draft_removing_in_review_and_bug(&api, t, &id).await;

    // The plan: In review and Bug, with their targets and a suggestion.
    let plan = get(&api, t, &format!("{base}/migration-plan")).await;
    assert_eq!(plan["ticketTotal"], 0, "no ticket before L05");
    let status = &plan["statuses"][0];
    assert_eq!(
        (
            &status["id"],
            &status["label"],
            &status["workflow"],
            &status["ticketCount"]
        ),
        (
            &review,
            &json!("In review"),
            &json!("Standard workflow"),
            &json!(0)
        )
    );
    let labels: Vec<(&str, bool)> = status["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["label"].as_str().unwrap(), t["isNew"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        labels,
        [
            ("To do", false),
            ("In progress", false),
            ("Done", false),
            ("Cancelled", false),
            ("In acceptance", true)
        ]
    );
    // Same category: In progress.
    let in_progress = by(&status["targets"], "label", "In progress")["id"].clone();
    let acceptance = by(&status["targets"], "label", "In acceptance")["id"].clone();
    assert_eq!(status["suggestedId"], in_progress);
    let types = &plan["types"];
    assert_eq!((&types[0]["id"], &types[0]["key"]), (&bug, &json!("bug")));
    let epic = by(&types[0]["targets"], "label", "Epic")["id"].clone();
    assert_eq!(
        types[0]["suggestedId"], epic,
        "the first type that is no sub-task"
    );
    let migrations = format!("{base}/migrations");
    let version = format!("\"{}\"", plan["version"]);

    // Incomplete: refused, naming what is missing.
    let incomplete = json!({"statusMappings": [{"fromId": review, "toId": acceptance}]});
    let (status, refused) = send(
        admin(&api, t, Method::POST, &migrations)
            .header("if-match", &version)
            .json(&incomplete),
    )
    .await;
    assert_problem(status, &refused, 400, "mapping_incomplete");
    assert_eq!(
        refused["errors"],
        json!([{"field": format!("typeMappings.{}", bug.as_str().unwrap()), "code": "required"}])
    );
    let wrong = json!({
        "statusMappings": [{"fromId": review, "toId": epic}],
        "typeMappings": [{"fromId": bug, "toId": epic}],
    });
    let (status, refused) = send(
        admin(&api, t, Method::POST, &migrations)
            .header("if-match", &version)
            .json(&wrong),
    )
    .await;
    assert_problem(status, &refused, 400, "mapping_incomplete");
    assert_eq!(refused["errors"][0]["code"], "not_a_target");

    // Complete: started, followed.
    let complete = json!({
        "statusMappings": [{"fromId": review, "toId": acceptance}],
        "typeMappings": [{"fromId": bug, "toId": epic}],
    });
    let response = admin(&api, t, Method::POST, &migrations)
        .header("if-match", &version)
        .json(&complete)
        .send()
        .await
        .unwrap();
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let (status, started) = json(response).await;
    assert_eq!(status, 202, "{started}");
    assert_eq!(
        (&started["state"], &started["percent"]),
        (&json!("pending"), &json!(100))
    );
    let (status, twice) = send(
        admin(&api, t, Method::POST, &migrations)
            .header("if-match", "*")
            .json(&complete),
    )
    .await;
    assert_problem(status, &twice, 409, "config.migration_running");

    // The worker runs it: the draft is published.
    let migration = infra::uuid::Uuid::parse_str(started["id"].as_str().unwrap()).unwrap();
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    assert_eq!(
        infra::config_migrations::run(&mut tx, migration).await.unwrap(),
        infra::config_migrations::Outcome::Published
    );
    tx.commit().await.unwrap();
    let done = get(&api, t, &location).await;
    assert_eq!(done["state"], "done");
    assert!(done["finishedAt"].is_string());
    let read = get(&api, t, &base).await;
    assert_eq!(
        (&read["draft"], &read["published"]["number"]),
        (&Value::Null, &json!(2))
    );
    // Nothing left to map.
    let (status, no_draft) = send(admin(&api, t, Method::GET, &format!("{base}/migration-plan"))).await;
    assert_problem(status, &no_draft, 409, "config.no_draft");
}

#[tokio::test]
async fn a_migration_starts_on_the_configuration_the_plan_was_read_on() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let base = format!("/api/v1/project-configs/{id}");
    let (status, no_draft) = send(
        admin(&api, t, Method::POST, &format!("{base}/migrations"))
            .header("if-match", "*")
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &no_draft, 409, "config.no_draft");
    let (review, bug) = draft_removing_in_review_and_bug(&api, t, &id).await;
    let plan = get(&api, t, &format!("{base}/migration-plan")).await;
    let body = json!({
        "statusMappings": [{"fromId": review, "toId": plan["statuses"][0]["suggestedId"]}],
        "typeMappings": [{"fromId": bug, "toId": plan["types"][0]["suggestedId"]}],
    });
    let stale = format!("\"{}\"", plan["version"].as_i64().unwrap() - 1);
    let (status, refused) = send(
        admin(&api, t, Method::POST, &format!("{base}/migrations"))
            .header("if-match", stale)
            .json(&body),
    )
    .await;
    assert_problem(status, &refused, 412, "precondition_failed");
    let (status, _) = send(admin(&api, t, Method::POST, &format!("{base}/migrations")).json(&body)).await;
    assert_eq!(status, 428);
}

#[tokio::test]
async fn only_configuration_admins_of_the_tenant_migrate() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let base = format!("/api/v1/project-configs/{id}");
    draft_removing_in_review_and_bug(&api, a, &id).await;
    let member = db.add_member(a, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let session = infra::sessions::SessionStore::new(
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
    let as_member = |method: Method, path: &str| {
        api.anonymous(method, a, path)
            .header("cookie", format!("mlbpp_session={}", session.token))
            .header("x-csrf-token", &session.session.csrf_token)
    };
    let unknown = format!("{base}/migrations/{}", infra::id::new_id());
    for (method, path) in [
        (Method::GET, format!("{base}/migration-plan")),
        (Method::POST, format!("{base}/migrations")),
        (Method::GET, unknown.clone()),
    ] {
        let (status, refused) = send(
            as_member(method.clone(), &path)
                .header("if-match", "*")
                .json(&json!({})),
        )
        .await;
        assert_problem(status, &refused, 403, "forbidden");
        let (status, hidden) = send(
            admin(&api, b, method, &path)
                .header("if-match", "*")
                .json(&json!({})),
        )
        .await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    let (status, missing) = send(admin(&api, a, Method::GET, &unknown)).await;
    assert_problem(status, &missing, 404, "not_found");
}
