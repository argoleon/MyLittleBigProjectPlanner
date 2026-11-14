//! Integration tests of tickets (L05-US01) on real PostgreSQL and Valkey:
//! the creation form of a type, creation with its custom fields and rules,
//! keys in sequence, reading by key or id, the parent of a sub-task, the
//! idempotent replay, the outbox event written with the ticket, "my
//! tickets", who may create, and tenant isolation.

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

/// A new member of `tenant`, with a session opened for them.
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

/// Grants the default role labelled `label` (English) to `account` on `node`.
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

/// A project of `tenant` under the default configuration; its id.
async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, key: &str) -> String {
    let body = json!({"type": "project", "name": name, "key": key});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// The configuration of `project`, with an Application list required on
/// bugs at creation and a Reviewers user field, published; Application.
async fn with_application(api: &TestApi, tenant: &SeedTenant, project: &str) -> Value {
    let body = json!({"nameTranslations": {"en": "Software"}});
    let (status, config) =
        send(admin(api, tenant, Method::POST, "/api/v1/project-configs").json(&body)).await;
    assert_eq!(status, 201, "{config}");
    let base = format!("/api/v1/project-configs/{}", config["id"].as_str().unwrap());
    let types = get(api, tenant, &format!("{base}/issue-types")).await["items"].clone();
    let bug = by(&types, "key", "bug")["id"].clone();
    let body = json!({
        "key": "application",
        "labelTranslations": {"fr": "Application", "en": "Application"},
        "kind": "single_select",
        "options": {"values": [
            {"labelTranslations": {"fr": "Annuaire", "en": "Directory"}},
            {"labelTranslations": {"fr": "Messagerie", "en": "Mail"}},
        ]},
        "issueTypeIds": [by(&types, "key", "story")["id"], bug],
        "rules": [{"issueTypeId": bug, "statusId": null, "mode": "required"}],
    });
    let (status, field) = send(admin(api, tenant, Method::POST, &format!("{base}/fields")).json(&body)).await;
    assert_eq!(status, 201, "{field}");
    let body = json!({
        "key": "reviewers",
        "labelTranslations": {"fr": "Relecteurs", "en": "Reviewers"},
        "kind": "user",
        "options": {"multiple": true},
        "allTypes": true,
    });
    let (status, reviewers) =
        send(admin(api, tenant, Method::POST, &format!("{base}/fields")).json(&body)).await;
    assert_eq!(status, 201, "{reviewers}");
    for (key, kind) in [("team", "group"), ("related", "issue_link")] {
        let body = json!({"key": key, "labelTranslations": {"en": key}, "kind": kind, "allTypes": true});
        let (status, added) =
            send(admin(api, tenant, Method::POST, &format!("{base}/fields")).json(&body)).await;
        assert_eq!(status, 201, "{added}");
    }
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

async fn create(api: &TestApi, tenant: &SeedTenant, project: &str, body: &Value) -> (u16, Value) {
    send(
        admin(
            api,
            tenant,
            Method::POST,
            &format!("/api/v1/projects/{project}/issues"),
        )
        .json(body),
    )
    .await
}

async fn outbox_events(db: &TestDb, tenant: &SeedTenant) -> Vec<(String, Uuid, Value)> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let events =
        infra::sqlx::query_as("SELECT kind, aggregate_id, payload FROM outbox_event ORDER BY created_at, id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    events
}

#[tokio::test]
async fn a_ticket_is_created_with_the_fields_of_its_type_and_read_by_key() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let application = with_application(&api, t, &plat).await;
    let field = application["id"].as_str().unwrap().to_owned();
    let mail = application["options"]["values"][1]["id"].clone();
    let form = get(&api, t, "/api/v1/projects/PLAT/issue-form").await;
    let field_id = |key: &str| {
        by(&form["fields"], "key", key)["fieldId"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (team, related) = (field_id("team"), field_id("related"));
    let (status, group) =
        send(admin(&api, t, Method::POST, "/api/v1/tenant/groups").json(&json!({"name": "Platform team"})))
            .await;
    assert_eq!(status, 201, "{group}");
    let reviewers = by(&form["fields"], "key", "reviewers")["fieldId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (camille, _) = member(&db, &api, t).await;

    // The form: the types, and the creation screen of the type chosen.
    let form = get(&api, t, "/api/v1/projects/PLAT/issue-form").await;
    assert_eq!(form["project"]["key"], "PLAT");
    let story = by(&form["types"], "key", "story")["id"].clone();
    let bug = by(&form["types"], "key", "bug")["id"].clone();
    assert_eq!(by(&form["types"], "key", "subtask")["isSubtask"], true);
    assert_eq!(
        form["typeId"],
        by(&form["types"], "key", "epic")["id"],
        "the first that is no sub-task"
    );
    let bug_form = get(
        &api,
        t,
        &format!(
            "/api/v1/projects/PLAT/issue-form?typeId={}",
            bug.as_str().unwrap()
        ),
    )
    .await;
    let shown = by(&bug_form["fields"], "key", "application");
    assert_eq!(
        (&shown["mode"], &shown["kind"]),
        (&json!("required"), &json!("single_select"))
    );
    assert_eq!(
        shown["values"][1],
        json!({"id": mail, "label": "Mail", "children": []})
    );
    let story_form = get(
        &api,
        t,
        &format!(
            "/api/v1/projects/PLAT/issue-form?typeId={}",
            story.as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(by(&story_form["fields"], "key", "application")["mode"], "visible");

    // What breaks the rules is refused, every field named.
    let (status, refused) = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": bug, "summary": "  ", "dueDate": "2026-12-01", "startDate": "2026-12-10"}),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [
            ("dueDate".to_owned(), "before_start".to_owned()),
            (format!("fields.{field}"), "required".to_owned()),
            ("summary".to_owned(), "required".to_owned()),
        ]
    );
    let hostile = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "x", "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]}
    ]}]});
    let (status, refused) = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": story, "summary": "S", "description": hostile, "assigneeId": Uuid::nil(),
                "fields": {Uuid::nil().to_string(): "x"}}),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [
            ("assigneeId".to_owned(), "unknown_member".to_owned()),
            ("description".to_owned(), "invalid".to_owned()),
            (format!("fields.{}", Uuid::nil()), "not_found".to_owned()),
        ]
    );
    assert!(outbox_events(&db, t).await.is_empty(), "nothing created");

    // A story: PLAT-1, in the initial status, reported by the admin.
    let description = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "Replace "},
        {"type": "text", "text": "local accounts", "marks": [{"type": "bold"}]},
        {"type": "text", "text": " <now>"}
    ]}]});
    let response = admin(&api, t, Method::POST, "/api/v1/projects/PLAT/issues")
        .json(&json!({
            "typeId": story,
            "summary": "  Migrate the authentication to OIDC ",
            "description": description,
            "priority": "high",
            "assigneeId": camille,
            "startDate": "2026-12-01",
            "dueDate": "2026-12-10",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["etag"], "\"1\"");
    assert_eq!(response.headers()["location"], "/api/v1/issues/PLAT-1");
    let (status, first) = json(response).await;
    assert_eq!(status, 201, "{first}");
    assert_eq!(
        (&first["key"], &first["summary"], &first["priority"]),
        (
            &json!("PLAT-1"),
            &json!("Migrate the authentication to OIDC"),
            &json!("high")
        )
    );
    assert_eq!(
        (&first["type"]["key"], &first["type"]["label"]),
        (&json!("story"), &json!("Story"))
    );
    assert_eq!(
        (&first["status"]["key"], &first["status"]["category"]),
        (&json!("todo"), &json!("todo"))
    );
    assert_eq!(first["assignee"]["id"], json!(camille));
    assert_eq!(first["reporter"]["id"], json!(api.session(t).member.account_id));
    assert_eq!(first["description"], description);
    assert_eq!(
        first["descriptionHtml"],
        "<p>Replace <strong>local accounts</strong> &lt;now&gt;</p>"
    );
    assert_eq!(
        (&first["startDate"], &first["dueDate"]),
        (&json!("2026-12-01"), &json!("2026-12-10"))
    );
    assert_eq!(first["fields"], json!([]));

    // A bug with its application: PLAT-2, the value shown by its label.
    let (status, second) = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": bug, "summary": "Sign-in loops",
                "fields": {&field: mail, &reviewers: [camille, Uuid::nil()], &team: Uuid::nil(), &related: Uuid::nil()}}),
    )
    .await;
    assert_problem(status, &second, 400, "validation_failed");
    let mut expected = vec![
        (format!("fields.{reviewers}"), "unknown_member".to_owned()),
        (format!("fields.{team}"), "unknown_group".to_owned()),
        (format!("fields.{related}"), "unknown_issue".to_owned()),
    ];
    expected.sort();
    assert_eq!(field_codes(&second), expected);
    let (status, second) = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": bug, "summary": "Sign-in loops",
                "fields": {&field: mail, &reviewers: [camille], &team: group["id"], &related: first["id"]}}),
    )
    .await;
    assert_eq!(status, 201, "{second}");
    assert_eq!(
        (&second["key"], &second["priority"]),
        (&json!("PLAT-2"), &json!("medium"))
    );
    assert_eq!(second["description"], Value::Null);
    let value = &second["fields"][0];
    assert_eq!(
        (&value["key"], &value["value"], &value["display"]),
        (&json!("application"), &mail, &json!("Mail"))
    );
    let value = &second["fields"][1];
    assert_eq!(
        (&value["key"], &value["value"]),
        (&json!("reviewers"), &json!([camille]))
    );
    assert!(
        value["display"].as_str().is_some_and(|name| !name.is_empty()),
        "{value}"
    );
    let shown: Vec<(&Value, &Value)> = second["fields"].as_array().unwrap()[2..]
        .iter()
        .map(|v| (&v["key"], &v["display"]))
        .collect();
    assert_eq!(
        shown,
        [
            (&json!("team"), &json!("Platform team")),
            (&json!("related"), &json!("PLAT-1"))
        ]
    );

    // Read by key and by id.
    let read = get(&api, t, "/api/v1/issues/PLAT-2").await;
    assert_eq!(read, second);
    let by_id = get(
        &api,
        t,
        &format!("/api/v1/issues/{}", first["id"].as_str().unwrap()),
    )
    .await;
    assert_eq!(by_id, first);
    for missing in ["PLAT-3", "NOPE-1", "plat", &Uuid::nil().to_string()] {
        let (status, body) = send(admin(&api, t, Method::GET, &format!("/api/v1/issues/{missing}"))).await;
        assert_problem(status, &body, 404, "not_found");
    }
    let (status, body) = send(admin(
        &api,
        t,
        Method::GET,
        "/api/v1/issues/PLAT-1?expand=comments",
    ))
    .await;
    assert_problem(status, &body, 400, "invalid_query");

    // Each creation wrote its event, in its transaction.
    let events = outbox_events(&db, t).await;
    let kinds: Vec<(&str, String)> = events
        .iter()
        .map(|(kind, id, payload)| {
            assert_eq!(payload["projectId"], json!(plat));
            (kind.as_str(), id.to_string())
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("issue.created", first["id"].as_str().unwrap().to_owned()),
            ("issue.created", second["id"].as_str().unwrap().to_owned()),
        ]
    );

    // My tickets: those reported, newest first, a page at a time.
    let mine = get(&api, t, "/api/v1/me/issues?limit=1").await;
    assert_eq!(mine["items"][0]["key"], "PLAT-2");
    assert_eq!(mine["items"][0]["status"]["label"], "To do");
    let cursor = mine["nextCursor"].as_str().unwrap();
    let rest = get(&api, t, &format!("/api/v1/me/issues?limit=1&cursor={cursor}")).await;
    assert_eq!(rest["items"][0]["key"], "PLAT-1");
    assert_eq!(rest["nextCursor"], Value::Null);
}

#[tokio::test]
async fn a_creation_replayed_with_its_idempotency_key_creates_one_ticket() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    project(&api, t, "Identity platform", "PLAT").await;
    let form = get(&api, t, "/api/v1/projects/PLAT/issue-form").await;
    let body = json!({"typeId": form["typeId"], "summary": "Once"});
    let post = || {
        admin(&api, t, Method::POST, "/api/v1/projects/PLAT/issues")
            .header("idempotency-key", "ticket-1")
            .json(&body)
    };
    let (status, first) = send(post()).await;
    assert_eq!(status, 201, "{first}");
    let (status, again) = send(post()).await;
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["id"], first["id"]);
    let (status, other) = send(
        admin(&api, t, Method::POST, "/api/v1/projects/PLAT/issues")
            .header("idempotency-key", "ticket-1")
            .json(&json!({"typeId": form["typeId"], "summary": "Twice"})),
    )
    .await;
    assert_eq!(status, 422, "{other}");
    let mine = get(&api, t, "/api/v1/me/issues").await;
    assert_eq!(mine["items"].as_array().unwrap().len(), 1);
    assert_eq!(outbox_events(&db, t).await.len(), 1);
}

#[tokio::test]
async fn a_subtask_sits_under_a_ticket_its_type_allows_in_the_same_project() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    project(&api, t, "Identity platform", "PLAT").await;
    project(&api, t, "Mail", "MAIL").await;
    let form = get(&api, t, "/api/v1/projects/PLAT/issue-form").await;
    let id = |key: &str| by(&form["types"], "key", key)["id"].clone();
    let (_, story) = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": id("story"), "summary": "Story"}),
    )
    .await;
    let (_, epic) = create(&api, t, "PLAT", &json!({"typeId": id("epic"), "summary": "Epic"})).await;
    let (_, elsewhere) = create(
        &api,
        t,
        "MAIL",
        &json!({"typeId": id("story"), "summary": "Mail story"}),
    )
    .await;
    assert_eq!(elsewhere["key"], "MAIL-1", "each project counts its own keys");

    for (parent, code) in [
        (Value::Null, "required"),
        (epic["id"].clone(), "not_allowed"),
        (elsewhere["id"].clone(), "other_project"),
        (json!(Uuid::nil()), "not_found"),
    ] {
        let (status, refused) = create(
            &api,
            t,
            "PLAT",
            &json!({"typeId": id("subtask"), "summary": "Sub", "parentId": parent}),
        )
        .await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(
            field_codes(&refused),
            [("parentId".to_owned(), code.to_owned())],
            "{parent}"
        );
    }
    let (status, sub) = create(
        &api,
        t,
        "PLAT",
        &json!({"typeId": id("subtask"), "summary": "Sub", "parentId": story["id"]}),
    )
    .await;
    assert_eq!(status, 201, "{sub}");
    assert_eq!(sub["key"], "PLAT-3");
    assert_eq!(sub["parentId"], story["id"]);
    assert!(sub.get("parent").is_none(), "only when expanded");
    let expanded = get(&api, t, "/api/v1/issues/PLAT-3?expand=parent").await;
    assert_eq!(
        expanded["parent"],
        json!({"id": story["id"], "key": "PLAT-1", "summary": "Story"})
    );
}

#[tokio::test]
async fn tickets_are_created_with_create_ticket_and_hidden_from_other_tenants() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let form = get(&api, a, "/api/v1/projects/PLAT/issue-form").await;
    let body = json!({"typeId": form["typeId"], "summary": "Mine"});
    let (_, ticket) = create(&api, a, "PLAT", &body).await;
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Reader", lea).await;
    let (hugo, contributor) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Contributor", hugo).await;

    // A reader sees the project and its tickets but creates none.
    for (method, path) in [
        (Method::GET, "/api/v1/projects/PLAT/issue-form"),
        (Method::POST, "/api/v1/projects/PLAT/issues"),
    ] {
        let (status, refused) = send(as_member(&api, a, &reader, method, path).json(&body)).await;
        assert_problem(status, &refused, 403, "permission.denied");
    }
    let (status, read) = send(as_member(&api, a, &reader, Method::GET, "/api/v1/issues/PLAT-1")).await;
    assert_eq!((status, &read["id"]), (200, &ticket["id"]));

    // A contributor creates; the ticket is theirs.
    let (status, created) = send(
        as_member(
            &api,
            a,
            &contributor,
            Method::POST,
            "/api/v1/projects/PLAT/issues",
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["reporter"]["id"], json!(hugo));
    let (status, mine) = send(as_member(&api, a, &contributor, Method::GET, "/api/v1/me/issues")).await;
    assert_eq!(status, 200);
    assert_eq!(mine["items"].as_array().unwrap().len(), 1);
    let (status, people) = send(as_member(
        &api,
        a,
        &contributor,
        Method::GET,
        "/api/v1/projects/PLAT/people?search=",
    ))
    .await;
    assert_eq!(status, 200, "{people}");
    assert!(people.as_array().unwrap().iter().any(|p| p["id"] == json!(lea)));
    let (status, group) =
        send(admin(&api, a, Method::POST, "/api/v1/tenant/groups").json(&json!({"name": "Platform team"})))
            .await;
    assert_eq!(status, 201, "{group}");
    let (status, groups) = send(as_member(
        &api,
        a,
        &contributor,
        Method::GET,
        "/api/v1/projects/PLAT/people?kind=group&search=platform",
    ))
    .await;
    assert_eq!(status, 200, "{groups}");
    assert_eq!(groups, json!([{"id": group["id"], "name": "Platform team"}]));

    // Another tenant sees neither the project nor its tickets.
    let id_path = format!("/api/v1/issues/{}", ticket["id"].as_str().unwrap());
    for (method, path) in [
        (Method::GET, "/api/v1/projects/PLAT/issue-form"),
        (Method::POST, "/api/v1/projects/PLAT/issues"),
        (Method::GET, "/api/v1/projects/PLAT/people"),
        (Method::GET, "/api/v1/issues/PLAT-1"),
        (Method::GET, id_path.as_str()),
    ] {
        let (status, hidden) = send(admin(&api, b, method, path).json(&body)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    let theirs = get(&api, b, "/api/v1/me/issues").await;
    assert_eq!(theirs["items"], json!([]));

    // An archived project takes no more tickets; the existing ones stay readable.
    let (status, _) = send(
        admin(&api, a, Method::PUT, &format!("/api/v1/nodes/{plat}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, refused) = create(&api, a, "PLAT", &body).await;
    assert_problem(status, &refused, 409, "node.archived");
    get(&api, a, "/api/v1/issues/PLAT-1").await;
}
