//! Integration tests of the screens of a configuration (L04-US06) on real
//! PostgreSQL and Valkey: the general screens and a type's own creation
//! screen whose fields are added, removed and reordered; a transition
//! screen attached to "Reject" and kept while it shows it; the rules
//! winning over the screens when they are resolved; fields joining and
//! leaving the screens; refusals; who may do it and tenant isolation.

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

/// What the tests name in a configuration, by key.
struct Config {
    id: String,
    types: Value,
    workflow: Value,
}

impl Config {
    async fn of(api: &TestApi, tenant: &SeedTenant, id: String) -> Self {
        let types =
            get(api, tenant, &format!("/api/v1/project-configs/{id}/issue-types")).await["items"].clone();
        let workflow =
            get(api, tenant, &format!("/api/v1/project-configs/{id}/workflows")).await["items"][0].clone();
        Self { id, types, workflow }
    }

    fn path(&self, rest: &str) -> String {
        format!("/api/v1/project-configs/{}{rest}", self.id)
    }

    fn type_id(&self, key: &str) -> Value {
        by(&self.types, "key", key)["id"].clone()
    }

    fn status_id(&self, key: &str) -> Value {
        by(&self.workflow["statuses"], "key", key)["id"].clone()
    }

    fn transition(&self, name: &str) -> Value {
        by(&self.workflow["transitions"], "name", name).clone()
    }
}

async fn screens(api: &TestApi, tenant: &SeedTenant, config: &Config) -> Value {
    get(api, tenant, &config.path("/screens")).await["items"].clone()
}

/// Adds a custom field; its id.
async fn add_field(api: &TestApi, tenant: &SeedTenant, config: &Config, body: Value) -> Value {
    let (status, field) = send(admin(api, tenant, Method::POST, &config.path("/fields")).json(&body)).await;
    assert_eq!(status, 201, "{field}");
    field["id"].clone()
}

/// The keys of the fields a resolved screen shows, with their mode and
/// whether they were added.
async fn resolved(
    api: &TestApi,
    tenant: &SeedTenant,
    config: &Config,
    query: &str,
) -> Vec<(String, String, bool)> {
    let body = get(api, tenant, &config.path(&format!("/resolved-screen?{query}"))).await;
    body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["key"].as_str().unwrap().to_owned(),
                f["mode"].as_str().unwrap().to_owned(),
                f["added"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn shown(key: &str, mode: &str, added: bool) -> (String, String, bool) {
    (key.to_owned(), mode.to_owned(), added)
}

#[tokio::test]
async fn a_type_gets_its_own_creation_screen_whose_fields_are_added_removed_and_reordered() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let config = Config::of(&api, t, config(&api, t, "Software").await).await;

    // The general screens, with the system fields.
    let list = screens(&api, t, &config).await;
    let kinds: Vec<(&str, bool)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["kind"].as_str().unwrap(), s["isGeneral"].as_bool().unwrap()))
        .collect();
    assert_eq!(kinds, [("creation", true), ("view", true)]);
    let fields = get(&api, t, &config.path("/fields")).await["items"].clone();
    let points = by(&fields, "key", "story_points")["id"].clone();
    let remaining = by(&fields, "key", "remaining")["id"].clone();
    assert_eq!(list[0]["fieldIds"], json!([points, remaining]));
    assert_eq!(list[0]["name"], "Creation");

    // Bug creation: its own screen.
    let body = json!({
        "kind": "creation",
        "issueTypeId": config.type_id("bug"),
        "nameTranslations": {"fr": "Création d’un bug", "en": "Bug creation"},
        "fieldIds": [remaining],
    });
    let (status, created) = send(
        admin(&api, t, Method::POST, &config.path("/screens"))
            .header("idempotency-key", "bug-creation")
            .json(&body),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["isGeneral"], false);
    let path = config.path(&format!("/screens/{}", created["id"].as_str().unwrap()));
    // A field added, then the two reordered, then one removed.
    for (fields, version) in [
        (json!([remaining, points]), 1),
        (json!([points, remaining]), 2),
        (json!([points]), 3),
    ] {
        let change = json!({"nameTranslations": created["nameTranslations"], "fieldIds": fields});
        let (status, changed) = send(
            admin(&api, t, Method::PUT, &path)
                .header("if-match", format!("\"{version}\""))
                .json(&change),
        )
        .await;
        assert_eq!(status, 200, "{changed}");
        assert_eq!(changed["fieldIds"], fields);
    }
    let list = screens(&api, t, &config).await;
    assert_eq!(list[1]["issueTypeId"], config.type_id("bug"));
    assert_eq!(list.as_array().unwrap().len(), 3);
    // Resolved for a bug: its own screen; for a story: the general one.
    let bug = config.type_id("bug");
    let story = config.type_id("story");
    let query = |t: &Value| format!("kind=creation&issueTypeId={}", t.as_str().unwrap());
    let for_bug = get(
        &api,
        t,
        &config.path(&format!("/resolved-screen?{}", query(&bug))),
    )
    .await;
    assert_eq!(for_bug["screenId"], created["id"]);
    // Points is not a bug's field: the rules leave it out.
    assert_eq!(for_bug["fields"], json!([]));
    assert_eq!(
        resolved(&api, t, &config, &query(&story)).await,
        [
            shown("story_points", "visible", false),
            shown("remaining", "visible", false)
        ]
    );

    // Refused: twice for the type, a transition screen for a type, a field twice.
    let (status, twice) = send(admin(&api, t, Method::POST, &config.path("/screens")).json(&body)).await;
    assert_problem(status, &twice, 409, "screen.exists");
    for (body, field, code) in [
        (
            json!({"kind": "transition", "issueTypeId": bug, "nameTranslations": {"en": "X"}}),
            "issueTypeId",
            "not_applicable",
        ),
        (
            json!({"kind": "view", "nameTranslations": {"en": "X"}}),
            "issueTypeId",
            "required",
        ),
        (
            json!({"kind": "view", "issueTypeId": bug, "nameTranslations": {"en": "X"}, "fieldIds": [points, points]}),
            "fieldIds[1]",
            "duplicate",
        ),
    ] {
        let (status, refused) =
            send(admin(&api, t, Method::POST, &config.path("/screens")).json(&body)).await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(refused["errors"][0]["field"], field, "{refused}");
        assert_eq!(refused["errors"][0]["code"], code, "{refused}");
    }
    // The general screens stay; the bug's goes.
    let general = config.path(&format!("/screens/{}", list[0]["id"].as_str().unwrap()));
    let (status, refused) = send(admin(&api, t, Method::DELETE, &general).header("if-match", "*")).await;
    assert_problem(status, &refused, 409, "screen.general");
    let (status, _) = send(admin(&api, t, Method::DELETE, &path).header("if-match", "*")).await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn a_transition_screen_is_shown_by_reject_and_kept_while_it_is() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let config = Config::of(&api, t, config(&api, t, "Software").await).await;
    // The rejection reason: required for tasks once back in progress.
    let reason = add_field(
        &api,
        t,
        &config,
        json!({
            "key": "rejection_reason", "labelTranslations": {"en": "Rejection reason"}, "kind": "short_text",
            "allTypes": true,
            "rules": [{"issueTypeId": config.type_id("task"), "statusId": config.status_id("in_progress"), "mode": "required"}],
        }),
    )
    .await;
    let body = json!({"kind": "transition", "nameTranslations": {"fr": "Motif du refus", "en": "Rejection reason"}, "fieldIds": [reason]});
    let (status, screen) = send(admin(&api, t, Method::POST, &config.path("/screens")).json(&body)).await;
    assert_eq!(status, 201, "{screen}");
    assert_eq!(screen["issueTypeId"], Value::Null);

    // Attached to Reject.
    let reject = config.transition("Reject");
    let change = |screen_id: &Value| {
        json!({
            "nameTranslations": reject["nameTranslations"], "fromId": reject["fromId"], "toId": reject["toId"],
            "rules": {"screenId": screen_id},
        })
    };
    let transition_path = config.path(&format!(
        "/workflow-transitions/{}",
        reject["id"].as_str().unwrap()
    ));
    let (status, saved) = send(
        admin(&api, t, Method::PUT, &transition_path)
            .header("if-match", "*")
            .json(&change(&screen["id"])),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(
        by(&saved["transitions"], "name", "Reject")["rules"]["screenId"],
        screen["id"]
    );
    let list = screens(&api, t, &config).await;
    assert_eq!(
        by(&list, "kind", "transition")["usedBy"],
        json!([{"transitionId": reject["id"], "workflowId": config.workflow["id"], "transition": "Reject", "workflow": "Standard workflow"}])
    );
    // Used when Reject runs, at its target status: required for a task.
    let query = |key: &str| {
        format!(
            "kind=transition&issueTypeId={}&transitionId={}",
            config.type_id(key).as_str().unwrap(),
            reject["id"].as_str().unwrap()
        )
    };
    assert_eq!(
        resolved(&api, t, &config, &query("task")).await,
        [shown("rejection_reason", "required", false)]
    );
    assert_eq!(
        resolved(&api, t, &config, &query("bug")).await,
        [shown("rejection_reason", "visible", false)]
    );

    // Not a creation screen, which is no transition screen.
    let creation = list[0]["id"].clone();
    let (status, refused) = send(
        admin(&api, t, Method::PUT, &transition_path)
            .header("if-match", "*")
            .json(&change(&creation)),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        refused["errors"],
        json!([{"field": "rules.screenId", "code": "not_found"}])
    );

    // Kept while Reject shows it, naming it.
    let screen_path = config.path(&format!("/screens/{}", screen["id"].as_str().unwrap()));
    let (status, refused) = send(admin(&api, t, Method::DELETE, &screen_path).header("if-match", "*")).await;
    assert_problem(status, &refused, 409, "screen.in_use");
    assert_eq!(refused["detail"], "Reject (Standard workflow)");
    let (status, _) = send(
        admin(&api, t, Method::PUT, &transition_path)
            .header("if-match", "*")
            .json(&change(&Value::Null)),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(admin(&api, t, Method::DELETE, &screen_path).header("if-match", "*")).await;
    assert_eq!(status, 204);
    // Without a screen: only what its target status requires.
    assert_eq!(
        resolved(&api, t, &config, &query("task")).await,
        [shown("rejection_reason", "required", true)]
    );
}

#[tokio::test]
async fn rules_win_and_fields_join_and_leave_the_screens() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let config = Config::of(&api, t, config(&api, t, "Software").await).await;
    let bug = config.type_id("bug");
    // Application: on every type, hidden for bugs at creation; joins the general screens.
    let application = add_field(
        &api,
        t,
        &config,
        json!({
            "key": "application", "labelTranslations": {"en": "Application"}, "kind": "short_text", "allTypes": true,
            "rules": [{"issueTypeId": bug, "statusId": null, "mode": "hidden"}],
        }),
    )
    .await;
    // The environment: required for bugs at creation, taken off the creation screen.
    let environment = add_field(
        &api,
        t,
        &config,
        json!({
            "key": "environment", "labelTranslations": {"en": "Environment"}, "kind": "short_text", "allTypes": true,
            "rules": [{"issueTypeId": bug, "statusId": null, "mode": "required"}],
        }),
    )
    .await;
    let list = screens(&api, t, &config).await;
    for screen in list.as_array().unwrap() {
        let ids = screen["fieldIds"].as_array().unwrap();
        assert_eq!(&ids[ids.len() - 2..], [application.clone(), environment.clone()]);
    }
    let creation = list[0].clone();
    let without: Vec<&Value> = creation["fieldIds"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|id| **id != environment)
        .collect();
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &config.path(&format!("/screens/{}", creation["id"].as_str().unwrap())),
        )
        .header("if-match", "*")
        .json(&json!({"nameTranslations": creation["nameTranslations"], "fieldIds": without})),
    )
    .await;
    assert_eq!(status, 200);
    // A bug at creation: Points is not a bug's, Application hidden by its
    // rule, the environment required and added back at the end.
    let query = format!("kind=creation&issueTypeId={}", bug.as_str().unwrap());
    assert_eq!(
        resolved(&api, t, &config, &query).await,
        [
            shown("remaining", "visible", false),
            shown("environment", "required", true)
        ]
    );
    // Viewed in progress: the view screen, all visible.
    let view = format!(
        "kind=view&issueTypeId={}&statusId={}",
        bug.as_str().unwrap(),
        config.status_id("in_progress").as_str().unwrap()
    );
    assert_eq!(
        resolved(&api, t, &config, &view).await,
        [
            shown("remaining", "visible", false),
            shown("application", "visible", false),
            shown("environment", "visible", false)
        ]
    );
    let (status, refused) = send(admin(
        &api,
        t,
        Method::GET,
        &config.path(&format!(
            "/resolved-screen?kind=view&issueTypeId={}",
            bug.as_str().unwrap()
        )),
    ))
    .await;
    assert_problem(status, &refused, 400, "invalid_query");

    // Application removed: it leaves every screen.
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::DELETE,
            &config.path(&format!("/fields/{}", application.as_str().unwrap())),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    for screen in screens(&api, t, &config).await.as_array().unwrap() {
        assert!(!screen["fieldIds"].as_array().unwrap().contains(&application));
    }
}

#[tokio::test]
async fn only_configuration_admins_of_the_tenant_edit_screens() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let config = Config::of(&api, a, config(&api, a, "Software").await).await;
    let creation = screens(&api, a, &config).await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
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
    let new = json!({"kind": "transition", "nameTranslations": {"en": "X"}});
    let change = json!({"nameTranslations": {"en": "X"}});
    let one = config.path(&format!("/screens/{creation}"));
    let resolve = config.path(&format!(
        "/resolved-screen?kind=creation&issueTypeId={}",
        config.type_id("bug").as_str().unwrap()
    ));
    for (method, path, body) in [
        (Method::GET, config.path("/screens"), &new),
        (Method::POST, config.path("/screens"), &new),
        (Method::PUT, one.clone(), &change),
        (Method::DELETE, one.clone(), &change),
        (Method::GET, resolve.clone(), &change),
    ] {
        let (status, refused) = send(
            as_member(method.clone(), &path)
                .header("if-match", "*")
                .json(body),
        )
        .await;
        assert_problem(status, &refused, 403, "forbidden");
        let (status, hidden) = send(admin(&api, b, method, &path).header("if-match", "*").json(body)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    assert_eq!(screens(&api, a, &config).await.as_array().unwrap().len(), 2);
}
