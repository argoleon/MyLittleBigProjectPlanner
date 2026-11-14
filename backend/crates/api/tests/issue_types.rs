//! Integration tests of the issue types of a configuration (L04-US02) on
//! real PostgreSQL and Valkey: the defaults, a type added to the draft and
//! available once published, the hierarchy and its depth, cycles and
//! sub-task rules refused, unique identifiers, removal and the publish
//! impact, order, who may do it, and tenant isolation.

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

fn types_path(config: &str) -> String {
    format!("/api/v1/project-configs/{config}/issue-types")
}

/// The types of `config`, with where they were read.
async fn types(api: &TestApi, tenant: &SeedTenant, config: &str) -> Value {
    let (status, list) = send(admin(api, tenant, Method::GET, &types_path(config))).await;
    assert_eq!(status, 200, "{list}");
    list
}

/// The type `key` of a list.
fn by_key<'a>(list: &'a Value, key: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == key)
        .unwrap_or_else(|| panic!("no {key} in {list}"))
}

fn keys_of(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["key"].as_str().unwrap().to_owned())
        .collect()
}

/// Changes the type `item` of `config` with `body`, whatever its version.
async fn change(api: &TestApi, tenant: &SeedTenant, config: &str, item: &Value, body: Value) -> (u16, Value) {
    let path = format!("{}/{}", types_path(config), item.as_str().unwrap());
    send(
        admin(api, tenant, Method::PUT, &path)
            .header("if-match", "*")
            .json(&body),
    )
    .await
}

#[tokio::test]
async fn a_type_is_added_to_the_draft_and_available_to_the_projects_once_published() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Support").await;

    // The defaults, from the published version: Epic > Story, Task, Bug > Sub-task.
    let published = types(&api, t, &id).await;
    assert_eq!(published["stage"], "published");
    assert_eq!(keys_of(&published), ["epic", "story", "task", "bug", "subtask"]);
    let levels: Vec<&Value> = published["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t["level"])
        .collect();
    assert_eq!(levels, [&json!(1), &json!(2), &json!(2), &json!(2), &json!(3)]);
    let epic = by_key(&published, "epic")["id"].clone();
    assert_eq!(by_key(&published, "subtask")["isSubtask"], true);
    assert_eq!(by_key(&published, "task")["label"], "Task");

    // Added with an icon, labels and its identifier: in the draft, opened for it.
    let incident = json!({
        "key": "incident",
        "labelTranslations": {"fr": "Incident", "en": "Incident"},
        "icon": "emergency",
        "color": "bug",
        "parentIds": [epic],
    });
    let (status, added) = send(
        admin(&api, t, Method::POST, &types_path(&id))
            .header("idempotency-key", "type-incident")
            .json(&incident),
    )
    .await;
    assert_eq!(status, 201, "{added}");
    assert_eq!(
        (&added["key"], &added["icon"], &added["color"], &added["level"]),
        (&json!("incident"), &json!("emergency"), &json!("bug"), &json!(2))
    );
    let (status, replayed) = send(
        admin(&api, t, Method::POST, &types_path(&id))
            .header("idempotency-key", "type-incident")
            .json(&incident),
    )
    .await;
    assert_eq!((status, &replayed["id"]), (201, &added["id"]));
    let drafted = types(&api, t, &id).await;
    assert_eq!(drafted["stage"], "draft");
    assert_eq!(keys_of(&drafted).last().map(String::as_str), Some("incident"));
    let (_, read) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}"),
    ))
    .await;
    assert_eq!(read["draft"]["changeCount"], 1);

    // Published: the projects' version has it.
    let publish = format!("/api/v1/project-configs/{id}/publish");
    let (status, _) = send(admin(&api, t, Method::POST, &publish).header("if-match", "*")).await;
    assert_eq!(status, 200);
    let after = types(&api, t, &id).await;
    assert_eq!(after["stage"], "published");
    assert_eq!(
        by_key(&after, "incident")["id"],
        added["id"],
        "the same type, version after version"
    );
}

#[tokio::test]
async fn the_hierarchy_defines_the_depth_and_never_loops() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let list = types(&api, t, &id).await;
    let (epic, story, task, bug, subtask) = (
        by_key(&list, "epic").clone(),
        by_key(&list, "story").clone(),
        by_key(&list, "task").clone(),
        by_key(&list, "bug").clone(),
        by_key(&list, "subtask").clone(),
    );
    let fields = |t: &Value, parents: Value| {
        json!({
            "labelTranslations": t["labelTranslations"],
            "icon": t["icon"],
            "color": t["color"],
            "isSubtask": t["isSubtask"],
            "parentIds": parents,
        })
    };

    // Epic > Story > Task > Sub-task: four levels.
    let (status, changed) = change(&api, t, &id, &task["id"], fields(&task, json!([story["id"]]))).await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["level"], 3);
    change(
        &api,
        t,
        &id,
        &subtask["id"],
        fields(&subtask, json!([task["id"]])),
    )
    .await;
    let chain = types(&api, t, &id).await;
    assert_eq!(by_key(&chain, "subtask")["level"], 4);
    assert_eq!(by_key(&chain, "story")["level"], 2);

    // Its own ancestor: Epic under Task, under Story, under Epic.
    let (status, cycle) = change(&api, t, &id, &epic["id"], fields(&epic, json!([task["id"]]))).await;
    assert_problem(status, &cycle, 409, "issue_type.cycle");
    let (status, own) = change(&api, t, &id, &bug["id"], fields(&bug, json!([bug["id"]]))).await;
    assert_problem(status, &own, 409, "issue_type.cycle");

    // Sub-tasks: a parent always, a parent never.
    let (status, orphan) = change(&api, t, &id, &subtask["id"], fields(&subtask, json!([]))).await;
    assert_problem(status, &orphan, 400, "validation_failed");
    assert_eq!(
        orphan["errors"][0],
        json!({"field": "parentIds", "code": "required"})
    );
    let (status, under) = change(&api, t, &id, &bug["id"], fields(&bug, json!([subtask["id"]]))).await;
    assert_problem(status, &under, 409, "issue_type.subtask_parent");
    let (status, unknown) = change(
        &api,
        t,
        &id,
        &bug["id"],
        fields(&bug, json!(["0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a99"])),
    )
    .await;
    assert_problem(status, &unknown, 400, "validation_failed");
    assert_eq!(
        unknown["errors"][0],
        json!({"field": "parentIds", "code": "not_found"})
    );

    // Nothing refused changed the draft: two types changed, each once.
    let (_, read) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}"),
    ))
    .await;
    assert_eq!(read["draft"]["changeCount"], 2);
}

#[tokio::test]
async fn identifiers_are_unique_and_fields_checked() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let add = |body: Value| admin(&api, t, Method::POST, &types_path(&id)).json(&body);

    let (status, taken) = send(add(json!({
        "key": "bug",
        "labelTranslations": {"en": "Another bug"},
        "icon": "bug_report",
        "color": "bug",
    })))
    .await;
    assert_problem(status, &taken, 409, "issue_type.key_taken");
    assert_eq!(taken["errors"][0], json!({"field": "key", "code": "taken"}));

    let (status, invalid) = send(add(json!({
        "key": "Bad-Key",
        "labelTranslations": {"de": "Fehler"},
        "icon": "Bug Report",
        "color": "bug",
    })))
    .await;
    assert_problem(status, &invalid, 400, "validation_failed");
    let fields: Vec<&Value> = invalid["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["field"])
        .collect();
    assert_eq!(
        fields,
        [
            &json!("key"),
            &json!("labelTranslations"),
            &json!("labelTranslations.de"),
            &json!("icon")
        ]
    );
    let (status, color) = send(add(json!({
        "key": "incident",
        "labelTranslations": {"en": "Incident"},
        "icon": "emergency",
        "color": "red",
    })))
    .await;
    assert_eq!(status, 400, "{color}");

    // The key stays: a change cannot carry one.
    let bug = by_key(&types(&api, t, &id).await, "bug").clone();
    let (status, rekeyed) = change(
        &api,
        t,
        &id,
        &bug["id"],
        json!({"key": "defect", "labelTranslations": {"en": "Bug"}, "icon": "bug_report", "color": "bug"}),
    )
    .await;
    assert_eq!(status, 400, "{rekeyed}");

    // A change needs the version it was read at.
    let path = format!("{}/{}", types_path(&id), bug["id"].as_str().unwrap());
    let body = json!({"labelTranslations": {"en": "Defect"}, "icon": "bug_report", "color": "bug", "parentIds": bug["parentIds"]});
    let (status, missing) = send(admin(&api, t, Method::PUT, &path).json(&body)).await;
    assert_problem(status, &missing, 428, "precondition_required");
    let (status, renamed) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(
        (&renamed["label"], &renamed["version"]),
        (&json!("Defect"), &json!(2))
    );
    let (status, stale) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
}

#[tokio::test]
async fn a_removed_type_shows_in_the_publish_impact() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let list = types(&api, t, &id).await;
    let delete = |item: &Value| {
        let path = format!("{}/{}", types_path(&id), item.as_str().unwrap());
        admin(&api, t, Method::DELETE, &path).header("if-match", "*")
    };

    // Bug goes, and leaves the parents of Sub-task.
    let (status, _) = send(delete(&by_key(&list, "bug")["id"])).await;
    assert_eq!(status, 204);
    let left = types(&api, t, &id).await;
    assert_eq!(keys_of(&left), ["epic", "story", "task", "subtask"]);
    let subtask = by_key(&left, "subtask");
    assert_eq!(subtask["parentIds"].as_array().unwrap().len(), 2);

    // The impact names it; no ticket uses it before L05, so nothing to migrate.
    let (status, impact) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}/publish-impact"),
    ))
    .await;
    assert_eq!(status, 200, "{impact}");
    assert_eq!(
        impact["removedTypes"],
        json!([{"id": by_key(&list, "bug")["id"], "key": "bug", "label": "Bug", "ticketCount": 0}])
    );
    assert_eq!(impact["needsMigration"], false);

    // Story and Task go too: Sub-task would have no parent left.
    let (status, _) = send(delete(&by_key(&list, "story")["id"])).await;
    assert_eq!(status, 204);
    let (status, orphan) = send(delete(&by_key(&list, "task")["id"])).await;
    assert_problem(status, &orphan, 409, "issue_type.orphan_subtask");
    // Without Sub-task, Task is not the last type: Epic stays.
    send(delete(&by_key(&list, "subtask")["id"])).await;
    let (status, _) = send(delete(&by_key(&list, "task")["id"])).await;
    assert_eq!(status, 204);
    let (status, last) = send(delete(&by_key(&list, "epic")["id"])).await;
    assert_problem(status, &last, 409, "issue_type.last_type");
    let (status, gone) = send(delete(&by_key(&list, "bug")["id"])).await;
    assert_problem(status, &gone, 404, "not_found");
}

#[tokio::test]
async fn types_are_ordered_and_only_by_configuration_admins_of_the_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let list = types(&api, a, &id).await;
    let ids: Vec<Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect();

    // Reordered: Bug first; every type once.
    let order = format!("{}/order", types_path(&id));
    let reversed: Vec<Value> = ids.iter().rev().cloned().collect();
    let (status, reordered) = send(admin(&api, a, Method::PUT, &order).json(&json!({"ids": reversed}))).await;
    assert_eq!(status, 200, "{reordered}");
    assert_eq!(keys_of(&reordered), ["subtask", "bug", "task", "story", "epic"]);
    let (status, partial) = send(admin(&api, a, Method::PUT, &order).json(&json!({"ids": &ids[..4]}))).await;
    assert_problem(status, &partial, 400, "validation_failed");
    assert_eq!(
        partial["errors"][0],
        json!({"field": "ids", "code": "not_a_permutation"})
    );
    let (_, read) = send(admin(
        &api,
        a,
        Method::GET,
        &format!("/api/v1/project-configs/{id}"),
    ))
    .await;
    assert_eq!(read["draft"]["changeCount"], 4, "the middle one did not move");

    // A plain member: 403; another tenant: 404.
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
    let (status, refused) = send(as_member(Method::GET, &types_path(&id))).await;
    assert_problem(status, &refused, 403, "forbidden");
    let (status, refused) = send(as_member(Method::PUT, &order).json(&json!({"ids": ids}))).await;
    assert_problem(status, &refused, 403, "forbidden");
    let (status, hidden) = send(admin(&api, b, Method::GET, &types_path(&id))).await;
    assert_problem(status, &hidden, 404, "not_found");
    let path = format!("{}/{}", types_path(&id), ids[0].as_str().unwrap());
    let (status, hidden) = send(admin(&api, b, Method::DELETE, &path).header("if-match", "*")).await;
    assert_problem(status, &hidden, 404, "not_found");
    assert_eq!(
        keys_of(&types(&api, a, &id).await),
        ["subtask", "bug", "task", "story", "epic"]
    );
}
