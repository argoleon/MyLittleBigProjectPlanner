//! Integration tests of the rules of workflow transitions (L04-US05) on
//! real PostgreSQL and Valkey: conditions on roles and groups, validators
//! and every post-function saved on a transition of the draft and named in
//! the workflow list, rules kept when a transition changes without them,
//! references checked (another tenant's role, a value that does not fit),
//! a removed field leaving them, the roles and candidates the editor picks
//! from, and who may do it.

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

/// A request of the seeded tenant admin of `tenant`, in English.
fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

/// Creates a configuration as the tenant admin; its id.
async fn config(api: &TestApi, tenant: &SeedTenant, en: &str) -> String {
    let body = json!({"nameTranslations": {"en": en}});
    let (status, config) =
        send(admin(api, tenant, Method::POST, "/api/v1/project-configs").json(&body)).await;
    assert_eq!(status, 201, "{config}");
    config["id"].as_str().unwrap().to_owned()
}

async fn group(api: &TestApi, tenant: &SeedTenant, name: &str) -> Value {
    let (status, group) =
        send(admin(api, tenant, Method::POST, "/api/v1/tenant/groups").json(&json!({"name": name}))).await;
    assert_eq!(status, 201, "{group}");
    group["id"].clone()
}

fn by<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
}

/// The standard workflow of `config`, and the list it comes in.
async fn standard(api: &TestApi, tenant: &SeedTenant, config: &str) -> (Value, Value) {
    let list = get(
        api,
        tenant,
        &format!("/api/v1/project-configs/{config}/workflows"),
    )
    .await;
    (list["items"][0].clone(), list)
}

/// The body that changes `transition` with `rules`.
fn with_rules(transition: &Value, rules: &Value) -> Value {
    json!({
        "nameTranslations": transition["nameTranslations"],
        "fromId": transition["fromId"],
        "toId": transition["toId"],
        "rules": rules,
    })
}

fn transition_path(config: &str, transition: &Value) -> String {
    format!(
        "/api/v1/project-configs/{config}/workflow-transitions/{}",
        transition["id"].as_str().unwrap()
    )
}

#[tokio::test]
async fn every_rule_type_is_saved_on_a_transition_and_named_in_the_list() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let (workflow, _) = standard(&api, t, &id).await;
    let validate = by(&workflow["transitions"], "name", "Validate").clone();
    assert_eq!(
        validate["rules"],
        json!({"conditions": {"roleIds": [], "groupIds": []}, "validators": [], "postFunctions": []})
    );

    // What the editor picks from.
    let roles = get(&api, t, "/api/v1/project-configs/transition-roles").await;
    let reviewer = roles[0]["id"].clone();
    assert!(roles[0]["label"].is_string());
    let quality = group(&api, t, "Quality").await;
    let bob = db.add_member(t, TenantRole::Member).await.account_id;
    let candidates = get(
        &api,
        t,
        "/api/v1/project-configs/transition-candidates?search=qual",
    )
    .await;
    assert_eq!(
        candidates,
        json!([{"kind": "group", "id": quality, "name": "Quality", "email": null}])
    );

    let fields = get(&api, t, &format!("/api/v1/project-configs/{id}/fields")).await;
    let points = by(&fields["items"], "key", "story_points")["id"].clone();
    let remaining = by(&fields["items"], "key", "remaining")["id"].clone();
    // "Validate": reviewers or the quality group; the remaining work first;
    // assigned to Bob, points set, remaining cleared, a comment.
    let rules = json!({
        "conditions": {"roleIds": [reviewer], "groupIds": [quality]},
        "validators": [remaining],
        "postFunctions": [
            {"kind": "assign", "assignTo": "member", "accountId": bob},
            {"kind": "set_field", "fieldId": points, "value": 3.5},
            {"kind": "clear_field", "fieldId": remaining},
            {"kind": "add_comment", "text": "Validated"},
            {"kind": "assign", "assignTo": "current_user"},
        ],
    });
    let path = transition_path(&id, &validate);
    let (status, saved) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&with_rules(&validate, &rules)),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    let stored = by(&saved["transitions"], "name", "Validate").clone();
    assert_eq!(stored["rules"], rules);
    assert_eq!(stored["version"], 2);

    // Renamed without its rules: they stay.
    let renamed = json!({
        "nameTranslations": {"en": "Accept"},
        "fromId": validate["fromId"],
        "toId": validate["toId"],
    });
    let (status, saved) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"2\"")
            .json(&renamed),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(by(&saved["transitions"], "name", "Accept")["rules"], rules);

    // The list names what the rules refer to.
    let (_, list) = standard(&api, t, &id).await;
    assert_eq!(list["references"]["roles"][0]["id"], reviewer);
    assert_eq!(
        list["references"]["groups"],
        json!([{"id": quality, "name": "Quality"}])
    );
    assert_eq!(list["references"]["members"][0]["id"], json!(bob));

    // Points leaves the draft: its post-function goes, the others stay.
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::DELETE,
            &format!("/api/v1/project-configs/{id}/fields/{}", points.as_str().unwrap()),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 409, "a system field stays");
    let custom = json!({"key": "reason", "labelTranslations": {"en": "Reason"}, "kind": "short_text", "allTypes": true});
    let (status, reason) = send(
        admin(
            &api,
            t,
            Method::POST,
            &format!("/api/v1/project-configs/{id}/fields"),
        )
        .json(&custom),
    )
    .await;
    assert_eq!(status, 201, "{reason}");
    let with_reason = json!({
        "validators": [reason["id"]],
        "postFunctions": [{"kind": "set_field", "fieldId": reason["id"], "value": "Accepted"}, {"kind": "assign", "assignTo": "nobody"}],
    });
    let (status, _) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "*")
            .json(&with_rules(&validate, &with_reason)),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::DELETE,
            &format!(
                "/api/v1/project-configs/{id}/fields/{}",
                reason["id"].as_str().unwrap()
            ),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    let (workflow, _) = standard(&api, t, &id).await;
    assert_eq!(
        by(&workflow["transitions"], "name", "Validate")["rules"],
        json!({
            "conditions": {"roleIds": [], "groupIds": []},
            "validators": [],
            "postFunctions": [{"kind": "assign", "assignTo": "nobody"}],
        })
    );
}

#[tokio::test]
async fn rules_name_what_exists_in_the_tenant_and_set_fitting_values() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let (workflow, _) = standard(&api, a, &id).await;
    let start = by(&workflow["transitions"], "name", "Start").clone();
    let fields = get(&api, a, &format!("/api/v1/project-configs/{id}/fields")).await;
    let points = by(&fields["items"], "key", "story_points")["id"].clone();
    // Another tenant's role, group and member.
    let foreign_role = get(&api, b, "/api/v1/project-configs/transition-roles").await[0]["id"].clone();
    let foreign_group = group(&api, b, "Elsewhere").await;
    let foreign_member = db.add_member(b, TenantRole::Member).await.account_id;
    let rules = json!({
        "conditions": {"roleIds": [foreign_role], "groupIds": [foreign_group]},
        "validators": [points, points],
        "postFunctions": [
            {"kind": "assign", "assignTo": "member", "accountId": foreign_member},
            {"kind": "set_field", "fieldId": points, "value": 1.25},
            {"kind": "add_comment", "text": " "},
        ],
    });
    let (status, refused) = send(
        admin(&api, a, Method::PUT, &transition_path(&id, &start))
            .header("if-match", "*")
            .json(&with_rules(&start, &rules)),
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
            ("rules.conditions.roleIds[0]", "not_found"),
            ("rules.conditions.groupIds[0]", "not_found"),
            ("rules.validators[1]", "duplicate"),
            ("rules.postFunctions[0].accountId", "unknown_member"),
            ("rules.postFunctions[1].value", "too_many_decimals"),
            ("rules.postFunctions[2].text", "required"),
        ]
    );
    // A post-function missing what its kind takes.
    let (status, refused) = send(
        admin(&api, a, Method::PUT, &transition_path(&id, &start))
            .header("if-match", "*")
            .json(&with_rules(
                &start,
                &json!({"postFunctions": [{"kind": "clear_field"}]}),
            )),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        refused["errors"],
        json!([{"field": "rules.postFunctions[0].fieldId", "code": "required"}])
    );
    // A new transition with its rules.
    let body = json!({
        "nameTranslations": {"en": "Restart"},
        "fromId": null,
        "toId": start["toId"],
        "rules": {"postFunctions": [{"kind": "assign", "assignTo": "reporter"}]},
    });
    let (status, created) = send(
        admin(
            &api,
            a,
            Method::POST,
            &format!(
                "/api/v1/project-configs/{id}/workflows/{}/transitions",
                workflow["id"].as_str().unwrap()
            ),
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(
        by(&created["transitions"], "name", "Restart")["rules"]["postFunctions"],
        json!([{"kind": "assign", "assignTo": "reporter"}])
    );
    // Nothing was stored on Start.
    let (workflow, _) = standard(&api, a, &id).await;
    assert_eq!(
        by(&workflow["transitions"], "name", "Start")["rules"]["postFunctions"],
        json!([])
    );
}

#[tokio::test]
async fn only_configuration_admins_pick_roles_and_candidates() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let config_admin = db.add_member(a, TenantRole::ConfigurationAdmin).await.account_id;
    let member = db.add_member(a, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let store = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    );
    for (account, expected) in [(config_admin, 200), (member, 403)] {
        let session = store.create(account, a.id).await.unwrap();
        for path in [
            "/api/v1/project-configs/transition-roles",
            "/api/v1/project-configs/transition-candidates?limit=5",
        ] {
            let (status, body) = send(
                api.anonymous(Method::GET, a, path)
                    .header("cookie", format!("mlbpp_session={}", session.token)),
            )
            .await;
            assert_eq!(status, expected, "{path}: {body}");
        }
    }
    let (status, refused) = send(admin(
        &api,
        a,
        Method::GET,
        "/api/v1/project-configs/transition-candidates?limit=51",
    ))
    .await;
    assert_problem(status, &refused, 400, "invalid_query");
}
