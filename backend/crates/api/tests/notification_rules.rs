//! Integration tests of the notification rules of a configuration
//! (L04-US07) on real PostgreSQL and Valkey: the default rules of a new
//! configuration, "Ticket created" notifying the reporter and the project
//! managers, every event targeting the people of the ticket, a role, a
//! group or members, refusals, who may do it and tenant isolation.

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

/// The role labelled `label` in English.
async fn role(api: &TestApi, tenant: &SeedTenant, label: &str) -> Value {
    let roles = get(api, tenant, "/api/v1/project-configs/transition-roles").await;
    roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["label"] == label)
        .unwrap_or_else(|| panic!("no role {label} in {roles}"))["id"]
        .clone()
}

fn rule<'a>(list: &'a Value, event: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["event"] == event)
        .unwrap_or_else(|| panic!("no {event} in {list}"))
}

#[tokio::test]
async fn ticket_created_notifies_the_reporter_and_the_project_managers() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let rules = format!("/api/v1/project-configs/{id}/notification-rules");

    // The defaults, Jira style.
    let list = get(&api, t, &rules).await;
    assert_eq!(list["stage"], "published");
    let defaults: Vec<(&str, &Value)> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["event"].as_str().unwrap(), &r["recipientKinds"]))
        .collect();
    assert_eq!(
        defaults,
        [
            ("created", &json!(["reporter", "assignee", "watchers"])),
            ("assigned", &json!(["assignee"])),
            ("status_changed", &json!(["reporter", "assignee", "watchers"])),
            ("commented", &json!(["reporter", "assignee", "watchers"])),
            ("mentioned", &json!(["mentioned"])),
            ("absence_conflict", &json!(["assignee"])),
        ]
    );

    // Created: the reporter and the project managers.
    let manager = role(&api, t, "Project manager").await;
    // Absence conflicts (L08-US04): the project managers too, by default.
    assert_eq!(rule(&list, "absence_conflict")["roleIds"], json!([manager]));
    let body = json!({"recipientKinds": ["reporter"], "roleIds": [manager]});
    let (status, saved) = send(
        admin(&api, t, Method::PUT, &format!("{rules}/created"))
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["stage"], "draft");
    let created = rule(&saved, "created");
    assert_eq!(
        (
            &created["recipientKinds"],
            &created["roleIds"],
            &created["version"]
        ),
        (&json!(["reporter"]), &json!([manager]), &json!(2))
    );
    assert_eq!(saved["references"]["roles"][0]["label"], "Project manager");
    let read = get(&api, t, &format!("/api/v1/project-configs/{id}")).await;
    assert_eq!(read["draft"]["changeCount"], 1);

    // Commented: a group and a member too, the people mentioned as well.
    let (status, quality) =
        send(admin(&api, t, Method::POST, "/api/v1/tenant/groups").json(&json!({"name": "Quality"}))).await;
    assert_eq!(status, 201);
    let bob = db.add_member(t, TenantRole::Member).await.account_id;
    let body = json!({
        "recipientKinds": ["assignee", "watchers", "mentioned"],
        "groupIds": [quality["id"]],
        "accountIds": [bob],
    });
    let (status, saved) = send(
        admin(&api, t, Method::PUT, &format!("{rules}/commented"))
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(rule(&saved, "commented")["accountIds"], json!([bob]));
    assert_eq!(
        saved["references"]["groups"],
        json!([{"id": quality["id"], "name": "Quality"}])
    );
    // Assigned turned off.
    let (status, saved) = send(
        admin(&api, t, Method::PUT, &format!("{rules}/assigned"))
            .header("if-match", "*")
            .json(&json!({})),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(rule(&saved, "assigned")["recipientKinds"], json!([]));

    // Published: the projects' version has them.
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::POST,
            &format!("/api/v1/project-configs/{id}/publish"),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 200);
    let list = get(&api, t, &rules).await;
    assert_eq!(list["stage"], "published");
    assert_eq!(rule(&list, "created")["roleIds"], json!([manager]));
}

#[tokio::test]
async fn a_rule_names_its_recipients_once_and_what_exists_in_the_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let rules = format!("/api/v1/project-configs/{id}/notification-rules");
    let foreign_role = role(&api, b, "Project manager").await;
    let foreign_member = db.add_member(b, TenantRole::Member).await.account_id;
    let body = json!({
        "recipientKinds": ["reporter", "reporter", "mentioned"],
        "roleIds": [foreign_role],
        "accountIds": [foreign_member],
    });
    let (status, refused) = send(
        admin(&api, a, Method::PUT, &format!("{rules}/created"))
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    let errors: Vec<(&str, &str)> = refused["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["field"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        errors,
        [
            ("recipientKinds[1]", "duplicate"),
            ("recipientKinds[2]", "not_applicable"),
            ("roleIds[0]", "not_found"),
            ("accountIds[0]", "unknown_member"),
        ]
    );
    // An unknown event: no such rule. A stale version.
    let (status, unknown) = send(
        admin(&api, a, Method::PUT, &format!("{rules}/updated"))
            .header("if-match", "*")
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &unknown, 404, "not_found");
    let (status, stale) = send(
        admin(&api, a, Method::PUT, &format!("{rules}/created"))
            .header("if-match", "\"9\"")
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    // Nothing went to a draft.
    assert_eq!(get(&api, a, &rules).await["stage"], "published");
}

#[tokio::test]
async fn only_configuration_admins_of_the_tenant_edit_notification_rules() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let rules = format!("/api/v1/project-configs/{id}/notification-rules");
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
    let one = format!("{rules}/created");
    for (method, path) in [(Method::GET, rules.clone()), (Method::PUT, one.clone())] {
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
    assert_eq!(
        get(&api, a, &rules).await["items"][0]["recipientKinds"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
