//! Integration tests of comments (L05-US04) on real PostgreSQL and Valkey:
//! a comment with a mention and ticket references rendered safely, its
//! event; refused content; the author edits and deletes, moderators delete
//! only, the placeholder; the version read; `comment` needed; tenant
//! isolation; archived projects.

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

/// The project PLAT with two stories, PLAT-1 and PLAT-2; its id.
async fn plat(api: &TestApi, tenant: &SeedTenant) -> String {
    let body = json!({"type": "project", "name": "Identity platform", "key": "PLAT"});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    let form = get(api, tenant, "/api/v1/projects/PLAT/issue-form").await;
    let story = form["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "story")
        .unwrap()["id"]
        .clone();
    for summary in ["Migrate to OIDC", "Directory scoping"] {
        let (status, created) = send(
            admin(api, tenant, Method::POST, "/api/v1/projects/PLAT/issues")
                .json(&json!({"typeId": story, "summary": summary})),
        )
        .await;
        assert_eq!(status, 201, "{created}");
    }
    node["id"].as_str().unwrap().to_owned()
}

fn doc(content: Value) -> Value {
    let mut paragraph = serde_json::Map::new();
    paragraph.insert("type".to_owned(), "paragraph".into());
    paragraph.insert("content".to_owned(), content);
    json!({"type": "doc", "content": [paragraph]})
}

fn said(text: &str) -> Value {
    json!({"content": doc(json!([{"type": "text", "text": text}]))})
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

const COMMENTS: &str = "/api/v1/issues/PLAT-1/comments";

#[tokio::test]
async fn a_comment_mentions_a_member_and_links_tickets_safely() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let project = plat(&api, t).await;
    let (camille, _) = member(&db, &api, t).await;
    let (hugo, contributor) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Contributor", hugo).await;

    let content = doc(json!([
        {"type": "mention", "attrs": {"id": camille.to_string(), "label": "Camille"}},
        {"type": "text", "text": " fixed by PLAT-2 <script>alert(1)</script>"},
    ]));
    let (status, written) =
        send(as_member(&api, t, &contributor, Method::POST, COMMENTS).json(&json!({"content": content})))
            .await;
    assert_eq!(status, 201, "{written}");
    assert_eq!(
        written["contentHtml"],
        format!(
            "<p><span class=\"mention\" data-id=\"{camille}\">@Camille</span> fixed by \
             <a class=\"issue-ref\" href=\"/issues/PLAT-2\">PLAT-2</a> &lt;script&gt;alert(1)&lt;/script&gt;</p>"
        )
    );
    assert_eq!(
        (
            &written["visibility"],
            &written["author"]["id"],
            &written["version"]
        ),
        (&json!("internal"), &json!(hugo), &json!(1))
    );
    assert_eq!(
        (
            &written["canEdit"],
            &written["canDelete"],
            &written["byTransition"]
        ),
        (&json!(true), &json!(true), &json!(false))
    );
    let created = events(&db, t, "comment.created").await;
    assert_eq!(
        (
            &created[0]["mentions"],
            &created[0]["issueRefs"],
            &created[0]["visibility"]
        ),
        (&json!([camille]), &json!(["PLAT-2"]), &json!("internal"))
    );

    // Public, for the portal; the list, oldest first, as the admin reads it.
    let (status, public) = send(admin(&api, t, Method::POST, COMMENTS).json(
        &json!({"content": doc(json!([{"type": "text", "text": "Deployed"}])), "visibility": "public"}),
    ))
    .await;
    assert_eq!(status, 201, "{public}");
    let listed = get(&api, t, COMMENTS).await;
    let seen: Vec<(&Value, &Value, &Value)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (&c["id"], &c["visibility"], &c["canEdit"]))
        .collect();
    // The admin moderates the first, edits only their own.
    assert_eq!(
        seen,
        [
            (&written["id"], &json!("internal"), &json!(false)),
            (&public["id"], &json!("public"), &json!(true))
        ]
    );
    assert_eq!(listed[0]["canDelete"], true);

    // Refused: empty, a mention of no member, a node outside the allow-list.
    for (body, code) in [
        (said("   "), "required"),
        (
            json!({"content": doc(json!([{"type": "mention", "attrs": {"id": Uuid::nil().to_string(), "label": "x"}}]))}),
            "unknown_member",
        ),
        (
            json!({"content": doc(json!([{"type": "image", "attrs": {"src": "x"}}]))}),
            "invalid",
        ),
    ] {
        let (status, refused) = send(admin(&api, t, Method::POST, COMMENTS).json(&body)).await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(
            refused["errors"],
            json!([{"field": "content", "code": code}]),
            "{body}"
        );
    }
}

#[tokio::test]
async fn the_author_edits_and_deletes_moderators_only_delete() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let project = plat(&api, t).await;
    let (camille, _) = member(&db, &api, t).await;
    let (hugo, author) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Contributor", hugo).await;
    let (lea, other) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Contributor", lea).await;
    let (pia, moderator) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Node admin", pia).await;
    let (status, written) =
        send(as_member(&api, t, &author, Method::POST, COMMENTS).json(&said("First draft"))).await;
    assert_eq!(status, 201);
    let path = format!("{COMMENTS}/{}", written["id"].as_str().unwrap());
    let edit = json!({
        "content": doc(json!([
            {"type": "text", "text": "Reviewed with "},
            {"type": "mention", "attrs": {"id": camille.to_string(), "label": "Camille"}}
        ])),
        "visibility": "public",
    });

    // Someone else, even a moderator, edits nothing.
    for session in [&other, &moderator] {
        let (status, refused) = send(
            as_member(&api, t, session, Method::PUT, &path)
                .header("if-match", "*")
                .json(&edit),
        )
        .await;
        assert_problem(status, &refused, 403, "permission.denied");
    }
    let (status, refused) = send(as_member(&api, t, &author, Method::PUT, &path).json(&edit)).await;
    assert_problem(status, &refused, 428, "precondition_required");
    let (status, edited) = send(
        as_member(&api, t, &author, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&edit),
    )
    .await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(
        (&edited["version"], &edited["visibility"]),
        (&json!(2), &json!("public"))
    );
    assert!(edited["editedAt"].is_string());
    let (status, stale) = send(
        as_member(&api, t, &author, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&edit),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let updated = &events(&db, t, "comment.updated").await[0];
    assert_eq!(
        (
            &updated["before"],
            &updated["text"],
            &updated["addedMentions"],
            &updated["formerVisibility"]
        ),
        (
            &json!("First draft"),
            &json!("Reviewed with @Camille"),
            &json!([camille]),
            &json!("internal")
        )
    );

    // Another member deletes nothing; a moderator deletes: a placeholder stays.
    let (status, refused) = send(as_member(&api, t, &other, Method::DELETE, &path)).await;
    assert_problem(status, &refused, 403, "permission.denied");
    let response = as_member(&api, t, &moderator, Method::DELETE, &path)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let listed = get(&api, t, COMMENTS).await;
    assert_eq!(
        (
            &listed[0]["deleted"],
            &listed[0]["content"],
            &listed[0]["contentHtml"],
            &listed[0]["author"]["id"]
        ),
        (&json!(true), &Value::Null, &Value::Null, &json!(hugo))
    );
    assert_eq!(
        (&listed[0]["canEdit"], &listed[0]["canDelete"]),
        (&json!(false), &json!(false))
    );
    assert_eq!(
        events(&db, t, "comment.deleted").await[0]["before"],
        "Reviewed with @Camille"
    );
    for method in [Method::PUT, Method::DELETE] {
        let (status, gone) = send(
            as_member(&api, t, &author, method, &path)
                .header("if-match", "*")
                .json(&edit),
        )
        .await;
        assert_problem(status, &gone, 404, "not_found");
    }

    // The author deletes their own.
    let (_, second) = send(as_member(&api, t, &author, Method::POST, COMMENTS).json(&said("Second"))).await;
    let response = as_member(
        &api,
        t,
        &author,
        Method::DELETE,
        &format!("{COMMENTS}/{}", second["id"].as_str().unwrap()),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), 204);
}

#[tokio::test]
async fn comments_need_comment_and_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let project = plat(&api, a).await;
    let (_, written) = send(admin(&api, a, Method::POST, COMMENTS).json(&said("Mine"))).await;
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &project, "Reader", lea).await;

    // A reader reads, writes nothing.
    let (status, listed) = send(as_member(&api, a, &reader, Method::GET, COMMENTS)).await;
    assert_eq!((status, listed.as_array().unwrap().len()), (200, 1));
    assert_eq!(listed[0]["canEdit"], false);
    let (status, refused) =
        send(as_member(&api, a, &reader, Method::POST, COMMENTS).json(&said("Me too"))).await;
    assert_problem(status, &refused, 403, "permission.denied");

    // Another tenant sees nothing, by key or id; a comment of another ticket is not this one's.
    let path = format!("{COMMENTS}/{}", written["id"].as_str().unwrap());
    for (method, path) in [
        (Method::GET, COMMENTS.to_owned()),
        (Method::POST, COMMENTS.to_owned()),
        (Method::DELETE, path.clone()),
    ] {
        let (status, hidden) = send(
            admin(&api, b, method, &path)
                .header("if-match", "*")
                .json(&said("x")),
        )
        .await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    let elsewhere = format!(
        "/api/v1/issues/PLAT-2/comments/{}",
        written["id"].as_str().unwrap()
    );
    let (status, hidden) = send(admin(&api, a, Method::DELETE, &elsewhere)).await;
    assert_problem(status, &hidden, 404, "not_found");

    // An archived project takes no more comments.
    let (status, _) = send(
        admin(
            &api,
            a,
            Method::PUT,
            &format!("/api/v1/nodes/{project}/archiving"),
        )
        .header("if-match", "*")
        .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, archived) = send(admin(&api, a, Method::POST, COMMENTS).json(&said("Late"))).await;
    assert_problem(status, &archived, 409, "node.archived");
}
