//! Integration tests of the history of tickets (L05-US06) on real
//! PostgreSQL and Valkey: every write (creation, edit, transition, link,
//! parent, move, comments) records who changed what from where, in its
//! transaction; read back newest first, page by page, in the reader's
//! language; deleted comments' content for moderators only; erased authors
//! under their pseudonym; links to tickets out of sight left out; the
//! origin shown; tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use domain::audit::{Action, Origin};
use infra::accounts::TenantRole;
use infra::audit::{Author, Entry};
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

fn by<'a>(items: &'a Value, key: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[key] == value)
        .unwrap_or_else(|| panic!("no {key} = {value} in {items}"))
}

/// The project `code`; its id.
async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, code: &str) -> String {
    let body = json!({"type": "project", "name": name, "key": code});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// Creates a ticket of type `type_key` in the project `code`; it.
async fn create(api: &TestApi, tenant: &SeedTenant, code: &str, type_key: &str, body: Value) -> Value {
    let form = get(api, tenant, &format!("/api/v1/projects/{code}/issue-form")).await;
    let mut body = body;
    body["typeId"] = by(&form["types"], "key", type_key)["id"].clone();
    let (status, created) = send(
        admin(
            api,
            tenant,
            Method::POST,
            &format!("/api/v1/projects/{code}/issues"),
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    created
}

fn doc(text: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
}

fn activity_of(key: &str) -> String {
    format!("/api/v1/issues/{key}/activity")
}

/// Every entry of the history of `key`, as the admin reads it.
async fn activity(api: &TestApi, tenant: &SeedTenant, key: &str) -> Vec<Value> {
    get(api, tenant, &activity_of(key)).await["items"]
        .as_array()
        .unwrap()
        .clone()
}

/// What an entry says, in short: action, field or relation, old and new
/// texts.
fn said(entry: &Value) -> (String, String, Value, Value) {
    let text = |value: &Value| {
        if value.is_null() {
            Value::Null
        } else {
            value["text"].clone()
        }
    };
    let what = if entry["relation"].is_null() {
        entry["field"].as_str().unwrap_or_default()
    } else {
        entry["relation"].as_str().unwrap()
    };
    (
        entry["action"].as_str().unwrap().to_owned(),
        what.to_owned(),
        text(&entry["old"]),
        text(&entry["new"]),
    )
}

fn row(action: &str, what: &str, old: Value, new: Value) -> (String, String, Value, Value) {
    (action.to_owned(), what.to_owned(), old, new)
}

#[tokio::test]
async fn every_write_tells_who_changed_what_and_from_where() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    project(&api, t, "Identity platform", "PLAT").await;
    project(&api, t, "Customers", "CRM").await;
    create(&api, t, "PLAT", "story", json!({"summary": "Migrate"})).await;
    create(&api, t, "PLAT", "story", json!({"summary": "Directory"})).await;
    let assert_count = |count: usize| {
        let api = &api;
        async move { assert_eq!(activity(api, t, "PLAT-1").await.len(), count) }
    };
    let created = activity(&api, t, "PLAT-1").await;
    assert_eq!(created.len(), 1);
    assert_eq!(said(&created[0]), row("created", "", Value::Null, Value::Null));
    assert_eq!(created[0]["actor"]["name"], "Test Member");
    assert_eq!(created[0]["origin"], "interface");

    // An edit: one entry per field changed, sharing one change.
    let (status, edited) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({
                "summary": "Migrate to OIDC",
                "priority": "high",
                "dueDate": "2026-11-30",
                "description": doc("Before"),
            })),
    )
    .await;
    assert_eq!(status, 200, "{edited}");
    // A refused edit records nothing: the history is written in the edit's transaction.
    let (status, refused) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"summary": " ", "priority": "low"})),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_count(5).await;
    let (status, _) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"description": doc("After")})),
    )
    .await;
    assert_eq!(status, 200);

    // A transition, a link, a comment written, edited, deleted.
    let transitions = get(&api, t, "/api/v1/issues/PLAT-1/transitions").await;
    let start = by(&transitions, "name", "Start")["id"].clone();
    let (status, moved) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-1/transitions")
            .header("if-match", "*")
            .json(&json!({"transitionId": start})),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    let (status, _) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-1/links")
            .json(&json!({"relation": "blocks", "issue": "PLAT-2"})),
    )
    .await;
    assert_eq!(status, 201);
    let (status, comment) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-1/comments").json(&json!({"content": doc("Done")})),
    )
    .await;
    assert_eq!(status, 201, "{comment}");
    let comment_path = format!(
        "/api/v1/issues/PLAT-1/comments/{}",
        comment["id"].as_str().unwrap()
    );
    let (status, _) = send(
        admin(&api, t, Method::PUT, &comment_path)
            .header("if-match", "*")
            .json(&json!({"content": doc("Done, deployed")})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(admin(&api, t, Method::DELETE, &comment_path)).await;
    assert_eq!(status, 204);

    let entries = activity(&api, t, "PLAT-1").await;
    // The transition may also assign (a post-function): left aside here.
    let rows: Vec<_> = entries
        .iter()
        .filter(|e| e["field"] != "assignee")
        .map(said)
        .collect();
    let null = Value::Null;
    assert_eq!(
        rows,
        [
            row("comment_deleted", "", null.clone(), null.clone()),
            row("comment_updated", "", null.clone(), null.clone()),
            row("comment_created", "", null.clone(), null.clone()),
            row("link_added", "blocks", null.clone(), json!("PLAT-2")),
            row("updated", "status", json!("To do"), json!("In progress")),
            row("updated", "description", null.clone(), null.clone()),
            row("updated", "summary", json!("Migrate"), json!("Migrate to OIDC")),
            row("updated", "description", null.clone(), null.clone()),
            row("updated", "priority", json!("medium"), json!("high")),
            row("updated", "dueDate", null.clone(), json!("2026-11-30")),
            row("created", "", null.clone(), null.clone()),
        ],
        "{entries:#?}"
    );
    // The transition's entries share one change.
    let status_entry = entries.iter().find(|e| e["field"] == "status").unwrap();
    assert!(
        entries
            .iter()
            .filter(|e| e["changeId"] == status_entry["changeId"])
            .all(|e| e["action"] == "updated")
    );
    let edit: Vec<&Value> = entries
        .iter()
        .filter(|e| e["field"] == "summary" || e["field"] == "priority" || e["field"] == "dueDate")
        .collect();
    assert_eq!(edit.len(), 3);
    assert!(edit.iter().all(|e| e["changeId"] == edit[0]["changeId"]));
    assert_eq!(
        (edit[1]["new"]["kind"].as_str(), edit[2]["new"]["kind"].as_str()),
        (Some("priority"), Some("date"))
    );
    let descriptions: Vec<&Value> = entries.iter().filter(|e| e["field"] == "description").collect();
    assert_eq!(descriptions[0]["old"]["html"], "<p>Before</p>");
    assert_eq!(descriptions[0]["new"]["html"], "<p>After</p>");
    assert_eq!(descriptions[1]["old"], Value::Null);
    assert!(
        entries
            .iter()
            .all(|e| e["actor"]["name"] == "Test Member" && e["origin"] == "interface")
    );
    let deleted = &entries[0];
    assert_eq!(
        deleted["old"]["html"], "<p>Done, deployed</p>",
        "a moderator sees what was deleted"
    );

    // The other ticket tells the link its own way; French labels in French.
    let other = activity(&api, t, "PLAT-2").await;
    assert_eq!(
        said(&other[0]),
        row("link_added", "blocked_by", null.clone(), json!("PLAT-1"))
    );
    let (status, french) = send(
        api.request(Method::GET, t, &activity_of("PLAT-1"))
            .header("accept-language", "fr"),
    )
    .await;
    assert_eq!(status, 200);
    let status_fr = by(&french["items"], "field", "status");
    assert_ne!(status_fr["new"]["text"], "In progress", "{status_fr}");

    // A sub-task's parent changed, then the ticket moved with it.
    let parent = get(&api, t, "/api/v1/issues/PLAT-1").await["id"].clone();
    let sub = create(
        &api,
        t,
        "PLAT",
        "subtask",
        json!({"summary": "Step", "parentId": parent}),
    )
    .await;
    let sub_key = sub["key"].as_str().unwrap().to_owned();
    let (status, reparented) = send(
        admin(&api, t, Method::PUT, &format!("/api/v1/issues/{sub_key}/parent"))
            .header("if-match", "*")
            .json(&json!({"parentId": get(&api, t, "/api/v1/issues/PLAT-2").await["id"]})),
    )
    .await;
    assert_eq!(status, 200, "{reparented}");
    let sub_entries = activity(&api, t, &sub_key).await;
    assert_eq!(
        said(&sub_entries[0]),
        row("updated", "parent", json!("PLAT-1"), json!("PLAT-2"))
    );
    let (status, moved) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-2/move")
            .header("if-match", "*")
            .json(&json!({"projectKey": "CRM", "typeMappings": [], "statusMappings": []})),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    let after_move = activity(&api, t, "CRM-1").await;
    assert_eq!(
        said(&after_move[0]),
        row("moved", "", json!("PLAT-2"), json!("CRM-1"))
    );
    let sub_moved = activity(&api, t, &sub_key).await;
    assert_eq!(
        sub_moved[0]["action"], "moved",
        "the sub-task followed: {sub_moved:#?}"
    );
}

#[tokio::test]
async fn the_history_reads_page_by_page_newest_first() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    project(&api, t, "Identity platform", "PLAT").await;
    create(&api, t, "PLAT", "story", json!({"summary": "S0"})).await;
    for n in 1..=4 {
        let (status, _) = send(
            admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
                .header("if-match", "*")
                .json(&json!({"summary": format!("S{n}")})),
        )
        .await;
        assert_eq!(status, 200);
    }
    let mut seen = Vec::new();
    let mut path = format!("{}?limit=2", activity_of("PLAT-1"));
    loop {
        let page = get(&api, t, &path).await;
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= 2);
        seen.extend(items.iter().map(|e| e["new"]["text"].clone()));
        match page["nextCursor"].as_str() {
            Some(cursor) => path = format!("{}?limit=2&cursor={cursor}", activity_of("PLAT-1")),
            None => break,
        }
    }
    assert_eq!(
        seen,
        [json!("S4"), json!("S3"), json!("S2"), json!("S1"), Value::Null]
    );
    let (status, refused) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("{}?cursor=nope", activity_of("PLAT-1")),
    ))
    .await;
    assert_problem(status, &refused, 400, "invalid_query");
    let (status, refused) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("{}?limit=0", activity_of("PLAT-1")),
    ))
    .await;
    assert_problem(status, &refused, 400, "invalid_query");
}

#[tokio::test]
async fn deleted_comments_show_to_moderators_only_and_erased_authors_under_their_pseudonym() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    create(&api, t, "PLAT", "story", json!({"summary": "Migrate"})).await;
    let (hugo, contributor) = member(&db, &api, t).await;
    grant(&db, &api, t, &plat, "Contributor", hugo).await;
    let (status, comment) = send(
        as_member(
            &api,
            t,
            &contributor,
            Method::POST,
            "/api/v1/issues/PLAT-1/comments",
        )
        .json(&json!({"content": doc("Mine")})),
    )
    .await;
    assert_eq!(status, 201, "{comment}");
    let (status, _) = send(as_member(
        &api,
        t,
        &contributor,
        Method::DELETE,
        &format!(
            "/api/v1/issues/PLAT-1/comments/{}",
            comment["id"].as_str().unwrap()
        ),
    ))
    .await;
    assert_eq!(status, 204);

    // Its author sees that it was deleted, not what it said; a moderator does.
    let (status, theirs) = send(as_member(
        &api,
        t,
        &contributor,
        Method::GET,
        &activity_of("PLAT-1"),
    ))
    .await;
    assert_eq!(status, 200, "{theirs}");
    assert_eq!(theirs["items"][0]["action"], "comment_deleted");
    assert_eq!(theirs["items"][0]["old"], Value::Null);
    let entries = activity(&api, t, "PLAT-1").await;
    assert_eq!(entries[0]["old"]["html"], "<p>Mine</p>");
    assert_eq!(entries[0]["actor"]["id"], json!(hugo));

    // Erased from the tenant: "Deleted user #N" wherever they acted.
    let (status, body) = send(
        api.request(Method::POST, t, &format!("/api/v1/tenant/members/{hugo}/erasure"))
            .header("if-match", "*")
            .json(&json!({})),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    let number: i64 = infra::sqlx::query_scalar(
        "SELECT pseudonym_number FROM erased_member WHERE tenant_id = $1 AND account_id = $2",
    )
    .bind(t.id)
    .bind(hugo)
    .fetch_one(&db.owner_pool)
    .await
    .unwrap();
    let entries = activity(&api, t, "PLAT-1").await;
    for entry in &entries[..2] {
        assert_eq!(
            entry["actor"]["name"],
            format!("Deleted user #{number}"),
            "{entry}"
        );
    }
    let comments = get(&api, t, "/api/v1/issues/PLAT-1/comments").await;
    assert_eq!(comments[0]["author"]["name"], format!("Deleted user #{number}"));
}

#[tokio::test]
async fn the_history_shows_the_origin_and_hides_what_the_reader_cannot_see() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let secret = project(&api, a, "Mergers", "MRG").await;
    let ticket = create(&api, a, "PLAT", "story", json!({"summary": "Mine"})).await;
    create(&api, a, "MRG", "story", json!({"summary": "Secret"})).await;
    let (status, _) = send(
        admin(
            &api,
            a,
            Method::PUT,
            &format!("/api/v1/nodes/{secret}/confidentiality"),
        )
        .header("if-match", "*")
        .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(
        admin(&api, a, Method::POST, "/api/v1/issues/PLAT-1/links")
            .json(&json!({"relation": "relates_to", "issue": "MRG-1"})),
    )
    .await;
    assert_eq!(status, 201);
    // A change through MCP (L16 will write them; recorded here as it will).
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let author = Author {
        actor_id: Some(api.session(a).member.account_id),
        origin: Origin::Mcp,
    };
    let id = Uuid::parse_str(ticket["id"].as_str().unwrap()).unwrap();
    infra::audit::record(
        &mut tx,
        id,
        author,
        &[Entry::field("summary", json!("Mine"), json!("Ours"))],
    )
    .await
    .unwrap();
    infra::audit::record(
        &mut tx,
        id,
        Author {
            actor_id: None,
            origin: Origin::System,
        },
        &[Entry::action(Action::Moved)],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let entries = activity(&api, a, "PLAT-1").await;
    assert_eq!(
        (&entries[0]["origin"], &entries[0]["actor"]),
        (&json!("system"), &Value::Null)
    );
    assert_eq!(entries[1]["origin"], "mcp");
    assert_eq!(entries[1]["actor"]["name"], "Test Member");
    assert_eq!(
        said(&entries[2]),
        row("link_added", "relates_to", Value::Null, json!("MRG-1"))
    );

    // A reader of PLAT only: the link to the confidential project is left out.
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Reader", lea).await;
    let (status, read) = send(as_member(&api, a, &reader, Method::GET, &activity_of("PLAT-1"))).await;
    assert_eq!(status, 200, "{read}");
    let actions: Vec<&str> = read["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["moved", "updated", "created"]);
    let (status, hidden) = send(as_member(&api, a, &reader, Method::GET, &activity_of("MRG-1"))).await;
    assert_problem(status, &hidden, 404, "not_found");

    // Another tenant sees nothing, by key or id.
    for key in ["PLAT-1", ticket["id"].as_str().unwrap()] {
        let (status, hidden) = send(admin(&api, b, Method::GET, &activity_of(key))).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
}
