//! Integration tests of the help to write queries (L06-US02) on real
//! PostgreSQL and Valkey: suggestions at the cursor (fields, operators,
//! values with their translated labels, functions), limited to what the
//! member sees and to their tenant, and the tree of a query for the builder.

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

async fn suggest_as(request: reqwest::RequestBuilder, query: &str, language: &str) -> Value {
    let (status, body) = send(
        request
            .header("accept-language", language)
            .json(&json!({"query": query})),
    )
    .await;
    assert_eq!(status, 200, "{query}: {body}");
    body
}

async fn suggest(api: &TestApi, tenant: &SeedTenant, query: &str) -> Value {
    suggest_as(
        api.request(Method::POST, tenant, "/api/v1/search/suggestions"),
        query,
        "en",
    )
    .await
}

/// The suggestions as `(kind, text)` pairs.
fn texts(body: &Value) -> Vec<(String, String)> {
    body["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["kind"].as_str().unwrap().to_owned(),
                s["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn values(body: &Value) -> Vec<String> {
    texts(body)
        .into_iter()
        .filter(|(kind, _)| kind == "value")
        .map(|(_, text)| text)
        .collect()
}

fn find<'a>(body: &'a Value, text: &str) -> &'a Value {
    body["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["text"] == text)
        .unwrap_or_else(|| panic!("no {text} in {body}"))
}

async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    let (status, node) = send(api.request(Method::POST, tenant, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn typing_stat_suggests_status_then_its_keys_with_their_labels() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;

    let fields = suggest(&api, a, "stat").await;
    assert_eq!(
        (fields["from"].clone(), fields["to"].clone()),
        (json!(0), json!(4))
    );
    assert_eq!(texts(&fields), [("field".to_owned(), "status".to_owned())]);

    let operators = suggest(&api, a, "status ").await;
    assert_eq!(
        texts(&operators),
        [
            ("operator", "="),
            ("operator", "!="),
            ("keyword", "IN"),
            ("keyword", "NOT IN")
        ]
        .map(|(k, t)| (k.to_owned(), t.to_owned()))
    );

    let statuses = suggest(&api, a, "status = ").await;
    for key in ["todo", "in_progress", "in_review", "done"] {
        assert!(values(&statuses).contains(&key.to_owned()), "{key} in {statuses}");
    }
    let in_progress = find(&statuses, "in_progress");
    assert_eq!(
        (in_progress["label"].as_str(), in_progress["detail"].as_str()),
        (Some("In progress"), Some("in_progress"))
    );
    // In French, and found by its French label.
    let request = api.request(Method::POST, a, "/api/v1/search/suggestions");
    let french = suggest_as(request, "status IN (todo, cour", "fr").await;
    assert_eq!(values(&french), ["in_progress"]);
    assert_eq!(find(&french, "in_progress")["label"], "En cours");
    assert_eq!(
        (french["from"].clone(), french["to"].clone()),
        (json!(17), json!(21))
    );

    // Custom fields by key, with their label; after a mistake, nothing.
    let custom = suggest(&api, a, "story").await;
    assert_eq!(find(&custom, "story_points")["label"], "Points");
    assert_eq!(
        texts(&suggest(&api, a, "status todo ").await),
        Vec::<(String, String)>::new()
    );
}

#[tokio::test]
async fn projects_and_nodes_are_those_the_member_sees_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let hr = node(
        &api,
        a,
        json!({"type": "portfolio", "name": "Human resources", "key": "PORTF-HR"}),
    )
    .await;
    node(
        &api,
        a,
        json!({"type": "project", "name": "Payroll", "key": "PAY", "parentId": hr}),
    )
    .await;
    let secret = node(
        &api,
        a,
        json!({"type": "project", "name": "Restructuring", "key": "SECRET", "parentId": hr}),
    )
    .await;
    let (status, _) = send(
        api.request(Method::PUT, a, &format!("/api/v1/nodes/{secret}/confidentiality"))
            .header("if-match", "*")
            .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200);
    node(
        &api,
        a,
        json!({"type": "project", "name": "Website", "key": "WEB"}),
    )
    .await;
    node(&api, b, json!({"type": "project", "name": "Paie", "key": "PAIE"})).await;

    // A reader of the portfolio: its projects, not the confidential one, not WEB.
    let member = db.add_member(a, TenantRole::Member).await;
    let reader: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = 'Reader'")
            .bind(a.id)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let (status, granted) = send(
        api.request(Method::POST, a, &format!("/api/v1/nodes/{hr}/grants"))
            .json(&json!({"roleId": reader, "accountId": member.account_id})),
    )
    .await;
    assert_eq!(status, 201, "{granted}");
    let settings = common::session_settings();
    let session = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(member.account_id, a.id)
    .await
    .unwrap();
    let as_reader = || {
        api.anonymous(Method::POST, a, "/api/v1/search/suggestions")
            .header("cookie", format!("mlbpp_session={}", session.token))
            .header("x-csrf-token", &session.session.csrf_token)
    };
    let projects = suggest_as(as_reader(), "project = ", "en").await;
    assert_eq!(values(&projects), ["PAY"]);
    assert_eq!(find(&projects, "PAY")["label"], "Payroll");
    let nodes = suggest_as(as_reader(), "project IN descendantsOf(\"P", "en").await;
    assert_eq!(values(&nodes), ["\"PAY\"", "\"PORTF-HR\""]);
    assert_eq!(find(&nodes, "\"PORTF-HR\"")["detail"], "portfolio");

    // The tenant admin sees them all, by key or name; never tenant B's.
    let all = suggest(&api, a, "project IN (").await;
    assert_eq!(values(&all), ["PAY", "SECRET", "WEB"]);
    assert!(texts(&all).contains(&("function".to_owned(), "descendantsOf(\"\")".to_owned())));
    assert_eq!(find(&all, "descendantsOf(\"\")")["caret"], 15);
    assert_eq!(values(&suggest(&api, a, "project = webs").await), ["WEB"]);
    assert_eq!(
        values(&suggest(&api, a, "project = pai").await),
        Vec::<String>::new()
    );
    assert_eq!(values(&suggest(&api, b, "project = ").await), ["PAIE"]);
}

#[tokio::test]
async fn members_priorities_and_functions_are_suggested_for_their_fields() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let colleague = db.add_member(a, TenantRole::Member).await;
    infra::sqlx::query("UPDATE account SET display_name = 'Camille Martin' WHERE id = $1")
        .bind(colleague.account_id)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let elsewhere = db.add_member(&db.tenant_b, TenantRole::Member).await;

    let people = suggest(&api, a, "assignee = cam").await;
    assert_eq!(values(&people), std::slice::from_ref(&colleague.email));
    assert_eq!(find(&people, &colleague.email)["label"], "Camille Martin");
    let me = suggest(&api, a, "reporter IN (").await;
    assert!(texts(&me).contains(&("function".to_owned(), "currentUser()".to_owned())));
    assert!(values(&me).contains(&colleague.email), "{me}");
    assert!(
        !values(&me).contains(&elsewhere.email),
        "never another tenant's member: {me}"
    );

    assert_eq!(
        values(&suggest(&api, a, "priority = h").await),
        ["highest", "high"]
    );
    let dates = suggest(&api, a, "due < start").await;
    assert_eq!(
        texts(&dates).into_iter().map(|(_, t)| t).collect::<Vec<_>>(),
        ["startOfDay()", "startOfWeek()", "startOfMonth()", "startOfYear()"]
    );
    let sorting = suggest(&api, a, "ORDER BY t").await;
    assert!(
        texts(&sorting).iter().all(|(_, t)| t != "text"),
        "text cannot sort: {sorting}"
    );
    assert!(texts(&sorting).contains(&("field".to_owned(), "type".to_owned())));
}

#[tokio::test]
async fn a_query_reads_back_as_a_tree_or_answers_where_it_is_wrong() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (status, parsed) = send(
        api.request(Method::POST, a, "/api/v1/search/parse")
            .json(&json!({"query": "project = PLAT and status in (in_progress) order by key"})),
    )
    .await;
    assert_eq!(status, 200, "{parsed}");
    assert_eq!(
        parsed["query"],
        "project = PLAT AND status IN (in_progress) ORDER BY key ASC"
    );
    assert_eq!(parsed["filter"]["kind"], "and");
    assert_eq!(
        parsed["filter"]["operands"][1]["values"],
        json!([{"kind": "text", "value": "in_progress"}])
    );
    assert_eq!(parsed["orderBy"], json!([{"field": "key", "descending": false}]));

    let (status, problem) = send(
        api.request(Method::POST, a, "/api/v1/search/parse")
            .json(&json!({"query": "status = "})),
    )
    .await;
    assert_eq!(
        (
            status,
            problem["code"].as_str(),
            problem["query"]["position"].as_u64()
        ),
        (400, Some("query.invalid"), Some(9))
    );
    let (status, _) = send(
        api.anonymous(Method::POST, a, "/api/v1/search/suggestions")
            .json(&json!({"query": "s"})),
    )
    .await;
    assert_eq!(status, 401);
}
