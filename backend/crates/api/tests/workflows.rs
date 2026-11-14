//! Integration tests of the workflows of a configuration (L04-US04) on
//! real PostgreSQL and Valkey: statuses with their category and portal
//! label, transitions from a status or from any, workflows added, copied,
//! followed by types and removed, the problems listed as the draft is
//! built and publishing refused while any, who may do it, and tenant
//! isolation.

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

/// Creates a configuration as the tenant admin; its id.
async fn config(api: &TestApi, tenant: &SeedTenant, en: &str) -> String {
    let body = json!({"nameTranslations": {"en": en}});
    let (status, config) =
        send(admin(api, tenant, Method::POST, "/api/v1/project-configs").json(&body)).await;
    assert_eq!(status, 201, "{config}");
    config["id"].as_str().unwrap().to_owned()
}

fn base(config: &str) -> String {
    format!("/api/v1/project-configs/{config}")
}

async fn workflows(api: &TestApi, tenant: &SeedTenant, config: &str) -> Value {
    let (status, list) = send(admin(
        api,
        tenant,
        Method::GET,
        &format!("{}/workflows", base(config)),
    ))
    .await;
    assert_eq!(status, 200, "{list}");
    list
}

fn by_field<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
}

/// The labels of the statuses of `workflow`, in their order.
fn status_keys(workflow: &Value) -> Vec<&str> {
    workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn statuses_and_transitions_are_built_in_the_draft_and_the_workflow_published_once_ready() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;

    // The standard workflow, ready, followed by every type.
    let list = workflows(&api, t, &id).await;
    assert_eq!(list["stage"], "published");
    let standard = list["items"][0].clone();
    let wid = standard["id"].as_str().unwrap().to_owned();
    assert_eq!(standard["key"], "standard");
    assert_eq!(standard["label"], "Standard workflow");
    assert_eq!(
        status_keys(&standard),
        ["todo", "in_progress", "in_review", "done", "cancelled"]
    );
    assert_eq!(standard["problems"], json!([]));
    assert_eq!(standard["issueTypeIds"].as_array().unwrap().len(), 5);
    let review = by_field(&standard["statuses"], "key", "in_review");
    assert_eq!(
        (&review["category"], &review["portalLabel"]),
        (&json!("in_progress"), &json!("Being processed"))
    );
    let cancel = by_field(&standard["transitions"], "name", "Cancel");
    assert_eq!(cancel["fromId"], Value::Null);
    let todo = by_field(&standard["statuses"], "key", "todo")["id"].clone();
    let doing = by_field(&standard["statuses"], "key", "in_progress")["id"].clone();

    // A status with its category, labels and portal label: in the statuses table.
    let statuses = format!("{}/workflows/{wid}/statuses", base(&id));
    let on_hold = json!({
        "key": "on_hold",
        "labelTranslations": {"fr": "En attente", "en": "On hold"},
        "portalLabelTranslations": {"fr": "En attente de vous", "en": "Waiting for you"},
        "category": "in_progress",
    });
    let (status, added) = send(admin(&api, t, Method::POST, &statuses).json(&on_hold)).await;
    assert_eq!(status, 201, "{added}");
    assert_eq!(status_keys(&added).last().copied(), Some("on_hold"));
    let held = by_field(&added["statuses"], "key", "on_hold").clone();
    assert_eq!(
        (&held["label"], &held["portalLabel"], &held["isInitial"]),
        (&json!("On hold"), &json!("Waiting for you"), &json!(false))
    );
    // Nothing leads to it yet: listed, and publishing is refused.
    assert_eq!(
        added["problems"],
        json!([{"code": "unreachable", "statusId": held["id"], "transitionId": null}])
    );
    let (status, impact) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("{}/publish-impact", base(&id)),
    ))
    .await;
    assert_eq!(status, 200, "{impact}");
    assert_eq!(
        impact["workflowProblems"],
        json!([{"field": "workflows.standard.statuses.on_hold", "code": "unreachable"}])
    );
    let publish = format!("{}/publish", base(&id));
    let (status, refused) = send(admin(&api, t, Method::POST, &publish).header("if-match", "*")).await;
    assert_problem(status, &refused, 409, "workflow.invalid");
    assert_eq!(
        refused["errors"],
        json!([{"field": "workflows.standard.statuses.on_hold", "code": "unreachable"}])
    );

    // Every status belongs to exactly one known category.
    let (status, unknown) = send(admin(&api, t, Method::POST, &statuses).json(&json!({
        "key": "parked", "labelTranslations": {"en": "Parked"}, "category": "blocked",
    })))
    .await;
    assert_eq!(status, 400, "{unknown}");
    let (status, missing) = send(admin(&api, t, Method::POST, &statuses).json(&json!({
        "key": "parked", "labelTranslations": {"en": "Parked"},
    })))
    .await;
    assert_eq!(status, 400, "{missing}");

    // Transitions to it: from In progress, and from all statuses.
    let transitions = format!("{}/workflows/{wid}/transitions", base(&id));
    let (status, paused) = send(admin(&api, t, Method::POST, &transitions).json(&json!({
        "nameTranslations": {"fr": "Mettre en attente", "en": "Put on hold"},
        "fromId": doing,
        "toId": held["id"],
    })))
    .await;
    assert_eq!(status, 201, "{paused}");
    assert_eq!(paused["problems"], json!([]));
    let (status, from_all) = send(admin(&api, t, Method::POST, &transitions).json(&json!({
        "nameTranslations": {"en": "Hold"},
        "fromId": null,
        "toId": held["id"],
    })))
    .await;
    assert_eq!(status, 201, "{from_all}");
    let hold = by_field(&from_all["transitions"], "name", "Hold");
    assert_eq!(hold["fromId"], Value::Null);
    // The same transition twice is a problem.
    let (_, twice) = send(admin(&api, t, Method::POST, &transitions).json(&json!({
        "nameTranslations": {"en": "Hold again"}, "toId": held["id"],
    })))
    .await;
    assert_eq!(twice["problems"][0]["code"], "duplicate_transition");
    let again = by_field(&twice["transitions"], "name", "Hold again")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::DELETE,
            &format!("{}/workflow-transitions/{again}", base(&id)),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);

    // No initial status: listed, and refused at publish.
    let todo_path = format!("{}/workflow-statuses/{}", base(&id), todo.as_str().unwrap());
    let (status, no_initial) = send(
        admin(&api, t, Method::PUT, &todo_path)
            .header("if-match", "*")
            .json(&json!({
                "labelTranslations": {"fr": "À faire", "en": "To do"}, "category": "todo", "isInitial": false,
            })),
    )
    .await;
    assert_eq!(status, 200, "{no_initial}");
    assert_eq!(no_initial["problems"][0]["code"], "no_initial");
    let (status, refused) = send(admin(&api, t, Method::POST, &publish).header("if-match", "*")).await;
    assert_problem(status, &refused, 409, "workflow.invalid");
    assert_eq!(
        refused["errors"],
        json!([{"field": "workflows.standard", "code": "no_initial"}])
    );
    // Back: ready, and published.
    send(
        admin(&api, t, Method::PUT, &todo_path)
            .header("if-match", "*")
            .json(&json!({
                "labelTranslations": {"fr": "À faire", "en": "To do"}, "category": "todo", "isInitial": true,
            })),
    )
    .await;
    let (status, published) = send(admin(&api, t, Method::POST, &publish).header("if-match", "*")).await;
    assert_eq!(status, 200, "{published}");
    let after = workflows(&api, t, &id).await;
    assert_eq!(after["stage"], "published");
    assert_eq!(status_keys(&after["items"][0]).last().copied(), Some("on_hold"));
}

#[tokio::test]
async fn workflows_are_added_copied_followed_and_removed() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let standard = workflows(&api, t, &id).await["items"][0].clone();
    let create = |body: Value| admin(&api, t, Method::POST, &format!("{}/workflows", base(&id))).json(&body);

    // A copy of the standard one, for bugs; an empty one.
    let (status, bug) = send(create(json!({
        "key": "bug", "labelTranslations": {"fr": "Workflow bug", "en": "Bug workflow"}, "copyOf": standard["id"],
    })))
    .await;
    assert_eq!(status, 201, "{bug}");
    assert_eq!(status_keys(&bug), status_keys(&standard));
    assert_eq!(bug["transitions"].as_array().unwrap().len(), 5);
    assert_ne!(bug["statuses"][0]["id"], standard["statuses"][0]["id"]);
    let (status, simple) = send(create(
        json!({"key": "simple", "labelTranslations": {"en": "Simple workflow"}}),
    ))
    .await;
    assert_eq!(status, 201, "{simple}");
    assert_eq!(
        simple["problems"],
        json!([{"code": "no_status", "statusId": null, "transitionId": null}])
    );
    let (status, taken) = send(create(
        json!({"key": "bug", "labelTranslations": {"en": "Another"}}),
    ))
    .await;
    assert_problem(status, &taken, 409, "workflow.key_taken");
    let (status, unknown) = send(create(json!({
        "key": "copy", "labelTranslations": {"en": "Copy"}, "copyOf": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a99",
    })))
    .await;
    assert_problem(status, &unknown, 400, "validation_failed");

    // Bug follows its own workflow.
    let (_, types) = send(admin(&api, t, Method::GET, &format!("{}/issue-types", base(&id)))).await;
    let bug_type = by_field(&types["items"], "key", "bug").clone();
    let (status, moved) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("{}/issue-types/{}", base(&id), bug_type["id"].as_str().unwrap()),
        )
        .header("if-match", "*")
        .json(&json!({
            "labelTranslations": bug_type["labelTranslations"],
            "icon": bug_type["icon"],
            "color": bug_type["color"],
            "parentIds": bug_type["parentIds"],
            "workflowId": bug["id"],
        })),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["workflowId"], bug["id"]);
    let list = workflows(&api, t, &id).await;
    assert_eq!(
        by_field(&list["items"], "key", "bug")["issueTypeIds"],
        json!([bug_type["id"]])
    );
    assert_eq!(
        by_field(&list["items"], "key", "standard")["issueTypeIds"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    // A type follows a workflow of its configuration.
    let (status, elsewhere) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("{}/issue-types/{}", base(&id), bug_type["id"].as_str().unwrap()),
        )
        .header("if-match", "*")
        .json(&json!({
            "labelTranslations": bug_type["labelTranslations"],
            "icon": bug_type["icon"],
            "color": bug_type["color"],
            "parentIds": bug_type["parentIds"],
            "workflowId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a98",
        })),
    )
    .await;
    assert_problem(status, &elsewhere, 400, "validation_failed");
    assert_eq!(
        elsewhere["errors"][0],
        json!({"field": "workflowId", "code": "not_found"})
    );

    // A transition between statuses of another workflow is refused.
    let (status, crossed) = send(
        admin(&api, t, Method::POST, &format!("{}/workflows/{}/transitions", base(&id), simple["id"].as_str().unwrap()))
            .json(&json!({"nameTranslations": {"en": "Cross"}, "fromId": null, "toId": standard["statuses"][0]["id"]})),
    )
    .await;
    assert_problem(status, &crossed, 400, "validation_failed");
    assert_eq!(
        crossed["errors"][0],
        json!({"field": "toId", "code": "not_found"})
    );

    // Removed: not while followed; the empty one goes.
    let delete = |workflow: &Value| {
        admin(
            &api,
            t,
            Method::DELETE,
            &format!("{}/workflows/{}", base(&id), workflow.as_str().unwrap()),
        )
        .header("if-match", "*")
    };
    let (status, in_use) = send(delete(&bug["id"])).await;
    assert_problem(status, &in_use, 409, "workflow.in_use");
    let (status, _) = send(delete(&simple["id"])).await;
    assert_eq!(status, 204);
    let renamed = format!("{}/workflows/{}", base(&id), standard["id"].as_str().unwrap());
    let (status, label) = send(
        admin(&api, t, Method::PUT, &renamed)
            .header("if-match", "*")
            .json(&json!({
                "labelTranslations": {"en": "Delivery workflow"},
            })),
    )
    .await;
    assert_eq!((status, &label["label"]), (200, &json!("Delivery workflow")));
    let (_, read) = send(admin(&api, t, Method::GET, &base(&id))).await;
    // Three workflows changed (bug, simple, standard) and one type: each once.
    assert_eq!(read["draft"]["changeCount"], 4);
}

#[tokio::test]
async fn a_removed_status_shows_in_the_impact_and_a_followed_workflow_stays() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let standard = workflows(&api, t, &id).await["items"][0].clone();

    // In review goes, with Send to review, Reject and Validate: Done is out of reach.
    let review = by_field(&standard["statuses"], "key", "in_review")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::DELETE,
            &format!("{}/workflow-statuses/{review}", base(&id)),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    let after = workflows(&api, t, &id).await["items"][0].clone();
    assert_eq!(after["transitions"].as_array().unwrap().len(), 2);
    let done = by_field(&after["statuses"], "key", "done")["id"].clone();
    assert_eq!(
        after["problems"],
        json!([{"code": "unreachable", "statusId": done, "transitionId": null}])
    );
    let (_, impact) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("{}/publish-impact", base(&id)),
    ))
    .await;
    assert_eq!(
        impact["removedStatuses"],
        json!([{"id": review, "key": "in_review", "label": "In review", "workflow": "Standard workflow", "ticketCount": 0}])
    );
    assert_eq!(impact["needsMigration"], false, "no ticket before L05");

    // Every type follows it: it stays.
    let delete = admin(
        &api,
        t,
        Method::DELETE,
        &format!("{}/workflows/{}", base(&id), standard["id"].as_str().unwrap()),
    )
    .header("if-match", "*");
    let (status, in_use) = send(delete).await;
    assert_problem(status, &in_use, 409, "workflow.in_use");
}

#[tokio::test]
async fn only_configuration_admins_of_the_tenant_edit_workflows() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let standard = workflows(&api, a, &id).await["items"][0].clone();
    let wid = standard["id"].as_str().unwrap();

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
    let body = json!({"key": "x", "labelTranslations": {"en": "X"}, "category": "todo"});
    for (method, path) in [
        (Method::GET, format!("{}/workflows", base(&id))),
        (Method::POST, format!("{}/workflows/{wid}/statuses", base(&id))),
        (Method::DELETE, format!("{}/workflows/{wid}", base(&id))),
    ] {
        let (status, refused) = send(as_member(method, &path).header("if-match", "*").json(&body)).await;
        assert_problem(status, &refused, 403, "forbidden");
    }
    for (method, path) in [
        (Method::GET, format!("{}/workflows", base(&id))),
        (Method::POST, format!("{}/workflows/{wid}/statuses", base(&id))),
        (
            Method::DELETE,
            format!(
                "{}/workflow-statuses/{}",
                base(&id),
                standard["statuses"][0]["id"].as_str().unwrap()
            ),
        ),
    ] {
        let (status, hidden) = send(admin(&api, b, method, &path).header("if-match", "*").json(&body)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    assert_eq!(status_keys(&workflows(&api, a, &id).await["items"][0]).len(), 5);
}
