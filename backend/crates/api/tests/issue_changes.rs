//! Integration tests of changing tickets (L05-US02) on real PostgreSQL and
//! Valkey: edits with the version read (428 without it, 412 when stale),
//! the rules of the type at its status, the edit form, the outbox event
//! with each change; moves with sub-tasks and former keys, mapped across
//! configurations, values dropped; who may do it and tenant isolation.

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

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

fn field_codes(body: &Value) -> Vec<(String, String)> {
    let mut codes: Vec<(String, String)> = body["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no errors in {body}"))
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    codes.sort();
    codes
}

fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("accept-language", "en")
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

fn by<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
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

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: &str, label: &str, account: Uuid) {
    let role: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
            .bind(tenant.id)
            .bind(label)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let path = format!("/api/v1/nodes/{node}/grants");
    let (status, granted) =
        send(admin(api, tenant, Method::POST, &path).json(&json!({"roleId": role, "accountId": account})))
            .await;
    assert_eq!(status, 201, "{granted}");
}

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, code: &str) -> String {
    let body = json!({"type": "project", "name": name, "key": code});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// A new configuration "Software" with an Application list on stories,
/// required while they are to do, published and used by `project`; the
/// field.
async fn software(api: &TestApi, tenant: &SeedTenant, project: &str) -> Value {
    let (status, config) = send(
        admin(api, tenant, Method::POST, "/api/v1/project-configs")
            .json(&json!({"nameTranslations": {"en": "Software"}})),
    )
    .await;
    assert_eq!(status, 201, "{config}");
    let base = format!("/api/v1/project-configs/{}", config["id"].as_str().unwrap());
    let types = get(api, tenant, &format!("{base}/issue-types")).await["items"].clone();
    let story = by(&types, "key", "story")["id"].clone();
    let workflow = get(api, tenant, &format!("{base}/workflows")).await["items"][0].clone();
    let todo = by(&workflow["statuses"], "key", "todo")["id"].clone();
    let body = json!({
        "key": "application",
        "labelTranslations": {"en": "Application"},
        "kind": "single_select",
        "options": {"values": [{"labelTranslations": {"en": "Directory"}}, {"labelTranslations": {"en": "Mail"}}]},
        "issueTypeIds": [story],
        "rules": [{"issueTypeId": story, "statusId": todo, "mode": "required"}],
    });
    let (status, field) = send(admin(api, tenant, Method::POST, &format!("{base}/fields")).json(&body)).await;
    assert_eq!(status, 201, "{field}");
    let (status, published) =
        send(admin(api, tenant, Method::POST, &format!("{base}/publish")).header("if-match", "*")).await;
    assert_eq!(status, 200, "{published}");
    let path = format!("/api/v1/nodes/{project}/configuration");
    let (status, moved) =
        send(admin(api, tenant, Method::PUT, &path).json(&json!({"configId": config["id"]}))).await;
    assert_eq!(status, 200, "{moved}");
    get(
        api,
        tenant,
        &format!("{base}/fields/{}", field["id"].as_str().unwrap()),
    )
    .await
}

/// The type `type_key` of the project `code`, by the creation form.
async fn type_id(api: &TestApi, tenant: &SeedTenant, code: &str, type_key: &str) -> Value {
    let form = get(api, tenant, &format!("/api/v1/projects/{code}/issue-form")).await;
    by(&form["types"], "key", type_key)["id"].clone()
}

async fn create(api: &TestApi, tenant: &SeedTenant, code: &str, body: &Value) -> Value {
    let (status, created) = send(
        admin(
            api,
            tenant,
            Method::POST,
            &format!("/api/v1/projects/{code}/issues"),
        )
        .json(body),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    created
}

fn patch(
    api: &TestApi,
    tenant: &SeedTenant,
    path: &str,
    version: Option<&str>,
    body: &Value,
) -> reqwest::RequestBuilder {
    let request = admin(api, tenant, Method::PATCH, &format!("/api/v1/issues/{path}")).json(body);
    match version {
        Some(version) => request.header("if-match", version),
        None => request,
    }
}

async fn events(db: &TestDb, tenant: &SeedTenant, kind: &str) -> Vec<Value> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let events: Vec<Value> =
        infra::sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
            .bind(kind)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    events
}

#[tokio::test]
async fn a_ticket_is_edited_with_the_version_it_read_and_the_rules_of_its_status() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let application = software(&api, t, &plat).await;
    let field = application["id"].as_str().unwrap().to_owned();
    let (directory, mail) = (
        application["options"]["values"][0]["id"].clone(),
        application["options"]["values"][1]["id"].clone(),
    );
    let story = type_id(&api, t, "PLAT", "story").await;
    let (camille, _) = member(&db, &api, t).await;
    let created = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": story, "summary": "Migrate to OIDC", "fields": {&field: directory}}),
    )
    .await;

    // The edit form: the view screen at its status, Application required.
    let form = get(&api, t, "/api/v1/issues/PLAT-1/edit-form").await;
    assert_eq!(by(&form["fields"], "key", "application")["mode"], "required");

    // Without the version read: refused; with it: changed, version 2.
    let changes = json!({
        "summary": "  Migrate the authentication to OIDC ",
        "priority": "highest",
        "assigneeId": camille,
        "startDate": "2026-12-01",
        "dueDate": "2026-12-10",
        "fields": {&field: mail},
    });
    let (status, refused) = send(patch(&api, t, "PLAT-1", None, &changes)).await;
    assert_problem(status, &refused, 428, "precondition_required");
    let response = patch(&api, t, "PLAT-1", Some("\"1\""), &changes)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["etag"], "\"2\"");
    let (status, edited) = json(response).await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(
        (&edited["summary"], &edited["priority"], &edited["version"]),
        (
            &json!("Migrate the authentication to OIDC"),
            &json!("highest"),
            &json!(2)
        )
    );
    assert_eq!(edited["assignee"]["id"], json!(camille));
    assert_eq!(
        (&edited["startDate"], &edited["dueDate"]),
        (&json!("2026-12-01"), &json!("2026-12-10"))
    );
    assert_eq!(edited["fields"][0]["display"], "Mail");
    let updated = events(&db, t, "issue.updated").await;
    assert_eq!(updated.len(), 1);
    let fields: Vec<&str> = updated[0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["field"].as_str().unwrap())
        .collect();
    let field_path = format!("fields.{field}");
    assert_eq!(
        fields,
        [
            "summary",
            "priority",
            "assigneeId",
            "startDate",
            "dueDate",
            field_path.as_str()
        ]
    );
    assert_eq!(
        by(&updated[0]["changes"], "field", "priority"),
        &json!({"field": "priority", "old": "medium", "new": "highest"})
    );

    // Someone saved first: the version read is stale.
    let (status, stale) = send(patch(
        &api,
        t,
        "PLAT-1",
        Some("\"1\""),
        &json!({"summary": "Mine"}),
    ))
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    assert_eq!(
        get(&api, t, "/api/v1/issues/PLAT-1").await["summary"],
        "Migrate the authentication to OIDC"
    );

    // Nothing changed: nothing written, same version.
    let (status, same) = send(patch(
        &api,
        t,
        "PLAT-1",
        Some("\"2\""),
        &json!({"priority": "highest"}),
    ))
    .await;
    assert_eq!((status, &same["version"]), (200, &json!(2)));
    assert_eq!(events(&db, t, "issue.updated").await.len(), 1);

    // The rules of its status: Application stays while to do; every error at once.
    let (status, refused) = send(patch(
        &api,
        t,
        "PLAT-1",
        Some("*"),
        &json!({"summary": "", "startDate": "2026-12-20", "assigneeId": Uuid::nil(),
                "fields": {&field: null, Uuid::nil().to_string(): "x"}}),
    ))
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    let mut expected = vec![
        ("assigneeId".to_owned(), "unknown_member".to_owned()),
        ("dueDate".to_owned(), "before_start".to_owned()),
        (format!("fields.{field}"), "required".to_owned()),
        (format!("fields.{}", Uuid::nil()), "not_found".to_owned()),
        ("summary".to_owned(), "required".to_owned()),
    ];
    expected.sort();
    assert_eq!(field_codes(&refused), expected);

    // Cleared: the assignee, the dates, the description.
    let (status, cleared) = send(patch(
        &api,
        t,
        created["id"].as_str().unwrap(),
        Some("\"2\""),
        &json!({"assigneeId": null, "startDate": null, "dueDate": null, "description": null}),
    ))
    .await;
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(
        (&cleared["assignee"], &cleared["startDate"], &cleared["version"]),
        (&Value::Null, &Value::Null, &json!(3))
    );
    let (status, unknown) = send(patch(&api, t, "PLAT-1", Some("*"), &json!({"type": "bug"}))).await;
    assert_eq!(status, 400, "{unknown}");
}

#[tokio::test]
async fn tickets_are_edited_with_edit_and_hidden_from_other_tenants() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let story = type_id(&api, a, "PLAT", "story").await;
    let ticket = create(&api, a, "PLAT", &json!({"typeId": story, "summary": "Mine"})).await;
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Reader", lea).await;
    let (hugo, contributor) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Contributor", hugo).await;
    let rename = json!({"summary": "Renamed"});

    for (method, path) in [
        (Method::PATCH, "/api/v1/issues/PLAT-1"),
        (Method::GET, "/api/v1/issues/PLAT-1/edit-form"),
    ] {
        let (status, refused) = send(
            as_member(&api, a, &reader, method, path)
                .header("if-match", "*")
                .json(&rename),
        )
        .await;
        assert_problem(status, &refused, 403, "permission.denied");
    }
    let (status, renamed) = send(
        as_member(&api, a, &contributor, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "\"1\"")
            .json(&rename),
    )
    .await;
    assert_eq!((status, &renamed["summary"]), (200, &json!("Renamed")));

    let by_id = format!("/api/v1/issues/{}", ticket["id"].as_str().unwrap());
    for path in ["/api/v1/issues/PLAT-1", by_id.as_str()] {
        let (status, hidden) = send(
            admin(&api, b, Method::PATCH, path)
                .header("if-match", "*")
                .json(&rename),
        )
        .await;
        assert_problem(status, &hidden, 404, "not_found");
    }

    let (status, _) = send(
        admin(&api, a, Method::PUT, &format!("/api/v1/nodes/{plat}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, archived) = send(patch(&api, a, "PLAT-1", Some("*"), &rename)).await;
    assert_problem(status, &archived, 409, "node.archived");
}

#[tokio::test]
async fn a_ticket_moves_with_its_sub_tasks_and_its_former_keys_still_find_it() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    project(&api, t, "Identity platform", "PLAT").await;
    let crm = project(&api, t, "Customer relations", "CRM").await;
    let epic = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": type_id(&api, t, "PLAT", "epic").await, "summary": "Unified sign-in"}),
    )
    .await;
    let story = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": type_id(&api, t, "PLAT", "story").await, "summary": "OIDC", "parentId": epic["id"]}),
    )
    .await;
    let sub = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": type_id(&api, t, "PLAT", "subtask").await, "summary": "Client", "parentId": story["id"]}),
    )
    .await;
    create(
        &api,
        t,
        "CRM",
        &json!({"typeId": type_id(&api, t, "PLAT", "story").await, "summary": "Already there"}),
    )
    .await;
    let to_crm = json!({"projectKey": "CRM"});
    let post = |path: &str, version: Option<&str>, body: &Value| {
        let request = admin(&api, t, Method::POST, &format!("/api/v1/issues/{path}")).json(body);
        match version {
            Some(version) => request.header("if-match", version.to_owned()),
            None => request,
        }
    };

    // A sub-task moves with its parent only; the project must be another one.
    let (status, refused) = send(post("PLAT-3/move", Some("*"), &to_crm)).await;
    assert_problem(status, &refused, 409, "issue.subtask_moves_with_parent");
    for (target, code) in [("PLAT", "same_project"), ("NOPE", "not_found")] {
        let (status, refused) = send(post("PLAT-2/move-plan", None, &json!({"projectKey": target}))).await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(
            field_codes(&refused),
            [("projectKey".to_owned(), code.to_owned())]
        );
    }

    // The plan: the same configuration, nothing to map; the epic link goes.
    let (status, plan) = send(post("PLAT-2/move-plan", None, &to_crm)).await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(plan["sameConfiguration"], true);
    let keys: Vec<&Value> = plan["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t["key"])
        .collect();
    assert_eq!(keys, [&json!("PLAT-2"), &json!("PLAT-3")]);
    assert_eq!(plan["parentCleared"]["key"], "PLAT-1");
    assert_eq!(
        (
            &plan["types"],
            &plan["statuses"],
            &plan["droppedFields"],
            &plan["problems"]
        ),
        (&json!([]), &json!([]), &json!([]), &json!([]))
    );

    let (status, refused) = send(post("PLAT-2/move", None, &to_crm)).await;
    assert_problem(status, &refused, 428, "precondition_required");
    let (status, refused) = send(post("PLAT-2/move", Some("\"7\""), &to_crm)).await;
    assert_problem(status, &refused, 412, "precondition_failed");
    let response = post("PLAT-2/move", Some("\"1\""), &to_crm).send().await.unwrap();
    assert_eq!(response.headers()["location"], "/api/v1/issues/CRM-2");
    let (status, moved) = json(response).await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(
        (
            &moved["key"],
            &moved["id"],
            &moved["project"]["id"],
            &moved["parentId"],
            &moved["version"]
        ),
        (
            &json!("CRM-2"),
            &story["id"],
            &json!(crm),
            &Value::Null,
            &json!(2)
        )
    );

    // Former keys find the tickets; the sub-task stays under its story.
    assert_eq!(get(&api, t, "/api/v1/issues/PLAT-2").await["key"], "CRM-2");
    let moved_sub = get(&api, t, "/api/v1/issues/PLAT-3").await;
    assert_eq!(
        (&moved_sub["key"], &moved_sub["parentId"]),
        (&json!("CRM-3"), &story["id"])
    );
    assert_eq!(moved_sub["id"], sub["id"]);
    assert_eq!(get(&api, t, "/api/v1/issues/PLAT-1").await["key"], "PLAT-1");
    let moves = events(&db, t, "issue.moved").await;
    let pairs: Vec<(&Value, &Value)> = moves.iter().map(|e| (&e["formerKey"], &e["key"])).collect();
    assert_eq!(
        pairs,
        [
            (&json!("PLAT-2"), &json!("CRM-2")),
            (&json!("PLAT-3"), &json!("CRM-3"))
        ]
    );
    // And it moves back with new keys, every former one kept.
    let (status, back) = send(post("CRM-2/move", Some("*"), &json!({"projectKey": "PLAT"}))).await;
    assert_eq!((status, &back["key"]), (200, &json!("PLAT-4")));
    for former in ["PLAT-2", "CRM-2"] {
        assert_eq!(
            get(&api, t, &format!("/api/v1/issues/{former}")).await["key"],
            "PLAT-4"
        );
    }
}

#[tokio::test]
async fn a_move_to_another_configuration_maps_types_and_statuses_and_drops_what_it_lacks() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let application = software(&api, t, &plat).await;
    project(&api, t, "Customer relations", "CRM").await;
    let story = type_id(&api, t, "PLAT", "story").await;
    let directory = application["options"]["values"][0]["id"].clone();
    let field = application["id"].as_str().unwrap().to_owned();
    create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": story, "summary": "OIDC", "fields": {&field: directory}}),
    )
    .await;
    let post = |path: &str, body: &Value| {
        admin(&api, t, Method::POST, &format!("/api/v1/issues/{path}"))
            .header("if-match", "*")
            .json(body)
    };

    // The plan: the story type and its status to map, the same keys suggested.
    let (status, plan) = send(post("PLAT-1/move-plan", &json!({"projectKey": "CRM"}))).await;
    assert_eq!(status, 200, "{plan}");
    assert_eq!(plan["sameConfiguration"], false);
    let crm_story = by(&plan["targets"], "label", "Story")["id"].clone();
    assert_eq!(
        (&plan["types"][0]["id"], &plan["types"][0]["suggestedId"]),
        (&story, &crm_story)
    );
    let todo = plan["statuses"][0]["id"].clone();
    let crm_todo = plan["statuses"][0]["suggestedId"].clone();
    assert_eq!(plan["statuses"][0]["key"], "todo");
    assert_eq!(plan["statuses"][0]["typeIds"], json!([story]));
    assert_eq!(
        by(&plan["statusTargets"], "id", crm_todo.as_str().unwrap())["label"],
        "To do"
    );
    assert_eq!(
        plan["droppedFields"],
        json!([{"fieldId": field, "label": "Application"}])
    );
    assert_eq!(plan["problems"], json!([]));
    // A sub-task type for the story: what the move would refuse.
    let crm_subtask = by(&plan["targets"], "label", "Sub-task")["id"].clone();
    let wrong = json!({"projectKey": "CRM", "typeMappings": [{"fromId": story, "toId": crm_subtask}]});
    let (_, wrong_plan) = send(post("PLAT-1/move-plan", &wrong)).await;
    assert_eq!(
        wrong_plan["problems"],
        json!([{"field": format!("typeMappings.{}", story.as_str().unwrap()), "code": "not_allowed"}])
    );

    // The move: refused while unmapped, then made with the mappings.
    let (status, refused) = send(post("PLAT-1/move", &json!({"projectKey": "CRM"}))).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [(
            format!("typeMappings.{}", story.as_str().unwrap()),
            "required".to_owned()
        )]
    );
    let mapped = json!({
        "projectKey": "CRM",
        "typeMappings": [{"fromId": story, "toId": crm_story}],
        "statusMappings": [{"fromId": todo, "toId": crm_todo}],
    });
    let (status, moved) = send(post("PLAT-1/move", &mapped)).await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(
        (
            &moved["key"],
            &moved["type"]["id"],
            &moved["status"]["id"],
            &moved["fields"]
        ),
        (&json!("CRM-1"), &crm_story, &crm_todo, &json!([]))
    );
    let moved_event = &events(&db, t, "issue.moved").await[0];
    assert_eq!(moved_event["droppedFieldIds"], json!([field]));
}

#[tokio::test]
async fn a_move_needs_edit_on_the_ticket_and_create_ticket_on_the_target() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let crm = project(&api, a, "Customer relations", "CRM").await;
    let story = type_id(&api, a, "PLAT", "story").await;
    create(&api, a, "PLAT", &json!({"typeId": story, "summary": "Mine"})).await;
    let (hugo, contributor) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Contributor", hugo).await;
    grant(&db, &api, a, &crm, "Reader", hugo).await;
    let to_crm = json!({"projectKey": "CRM"});

    for path in ["/api/v1/issues/PLAT-1/move-plan", "/api/v1/issues/PLAT-1/move"] {
        let (status, refused) = send(
            as_member(&api, a, &contributor, Method::POST, path)
                .header("if-match", "*")
                .json(&to_crm),
        )
        .await;
        assert_problem(status, &refused, 403, "permission.denied");
        let (status, hidden) = send(
            admin(&api, b, Method::POST, path)
                .header("if-match", "*")
                .json(&to_crm),
        )
        .await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    grant(&db, &api, a, &crm, "Contributor", hugo).await;
    let (status, moved) = send(
        as_member(&api, a, &contributor, Method::POST, "/api/v1/issues/PLAT-1/move")
            .header("if-match", "*")
            .json(&to_crm),
    )
    .await;
    assert_eq!((status, &moved["key"]), (200, &json!("CRM-1")));
}
