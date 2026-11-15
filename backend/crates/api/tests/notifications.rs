//! Integration tests of notifications (L05-US07) on real PostgreSQL,
//! Valkey and Mailpit, the worker's queue running: watchers (automatic and
//! chosen); an assignment, a mention, a comment and a status change
//! notifying whom the configuration's rules name, never the author nor who
//! does not see the ticket, in the application and by email as preferences
//! say; the notifications of two organizations read together, filtered,
//! marked read; preferences; the daily digest; isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb, TestMember};

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

async fn open_session(api: &TestApi, tenant: &SeedTenant, account_id: Uuid) -> Session {
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
    (opened.token, opened.session.csrf_token)
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (TestMember, Session) {
    let member = db.add_member(tenant, TenantRole::Member).await;
    let session = open_session(api, tenant, member.account_id).await;
    (member, session)
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

/// The project PLAT with the story PLAT-1 reported by the admin; the
/// project's id.
async fn plat(api: &TestApi, tenant: &SeedTenant) -> String {
    let body = json!({"type": "project", "name": "Identity platform", "key": "PLAT"});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    let (_, form) = send(admin(
        api,
        tenant,
        Method::GET,
        "/api/v1/projects/PLAT/issue-form",
    ))
    .await;
    let story = form["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "story")
        .unwrap()["id"]
        .clone();
    let (status, created) = send(
        admin(api, tenant, Method::POST, "/api/v1/projects/PLAT/issues")
            .json(&json!({"typeId": story, "summary": "Migrate to OIDC"})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    node["id"].as_str().unwrap().to_owned()
}

/// Runs the queued jobs (dispatches, emails) as the worker does.
async fn run_jobs(api: &TestApi, db: &TestDb) {
    let jobs = jobs(api);
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
}

fn jobs(api: &TestApi) -> worker::jobs::Jobs {
    worker::jobs::Jobs {
        mailer: mailer(),
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
        storage: None,
        clamav: None,
        public_url: Some("https://{tenant}.example.test".to_owned()),
    }
}

fn mailer() -> infra::email::Mailer {
    infra::email::Mailer::new(
        &infra::config::Secret::new(testkit::smtp_url()),
        "Planner <noreply@example.org>",
    )
    .unwrap()
}

fn mention(account: Uuid, name: &str, text: &str) -> Value {
    json!({"content": {"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "mention", "attrs": {"id": account, "label": name}},
        {"type": "text", "text": text}
    ]}]}})
}

/// The notifications of `session`'s member, as listed.
async fn notifications_of(api: &TestApi, tenant: &SeedTenant, session: &Session) -> Vec<Value> {
    let (status, page) = send(as_member(
        api,
        tenant,
        session,
        Method::GET,
        "/api/v1/me/notifications",
    ))
    .await;
    assert_eq!(status, 200, "{page}");
    page["items"].as_array().unwrap().clone()
}

fn events(items: &[Value]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|n| {
            (
                n["event"].as_str().unwrap().to_owned(),
                n["issueKey"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The email jobs queued to `to` (before the worker runs them).
async fn queued_emails(db: &TestDb, to: &str) -> i64 {
    infra::sqlx::query_scalar("SELECT count(*) FROM job WHERE kind = 'email.send' AND payload ->> 'to' = $1")
        .bind(to)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn changes_notify_whom_the_rules_name_in_the_app_and_by_email() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let project = plat(&api, t).await;
    let (hugo, hugo_session) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Contributor", hugo.account_id).await;
    let (camille, camille_session) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Reader", camille.account_id).await;
    // Somebody who does not see the project.
    let (outsider, outsider_session) = member(&db, &api, t).await;
    run_jobs(&api, &db).await;

    // The admin, who reported PLAT-1, watches it; assigning Hugo makes him watch it too.
    let (status, watchers) = send(admin(&api, t, Method::GET, "/api/v1/issues/PLAT-1/watchers")).await;
    assert_eq!((status, &watchers), (200, &json!({"count": 1, "watching": true})));
    let (status, _) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"assigneeId": hugo.account_id})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        queued_emails(&db, &hugo.email).await,
        0,
        "written by the worker, not the request"
    );
    run_jobs(&api, &db).await;
    let mine = notifications_of(&api, t, &hugo_session).await;
    assert_eq!(events(&mine), [("assigned".to_owned(), "PLAT-1".to_owned())]);
    assert_eq!(
        (
            &mine[0]["actorName"],
            &mine[0]["issueSummary"],
            &mine[0]["read"],
            &mine[0]["tenant"]["slug"]
        ),
        (
            &json!("Test Member"),
            &json!("Migrate to OIDC"),
            &json!(false),
            &json!(t.slug)
        )
    );
    let email = testkit::caught_email(&hugo.email).await;
    assert_eq!(
        email.subject,
        format!(
            "Test Member vous a assigné PLAT-1 · {}",
            tenant_name(&db, t).await
        )
    );
    assert!(
        email
            .text
            .contains(&format!("https://{}.example.test/issues/PLAT-1", t.slug)),
        "{}",
        email.text
    );
    assert!(
        notifications_of(&api, t, &api_admin_session(&api, t))
            .await
            .is_empty(),
        "never the author"
    );

    // Hugo comments mentioning Camille, and the outsider who does not see the project.
    let (status, _) = send(
        as_member(
            &api,
            t,
            &hugo_session,
            Method::POST,
            "/api/v1/issues/PLAT-1/comments",
        )
        .json(&mention(camille.account_id, "Camille", " can you check?")),
    )
    .await;
    assert_eq!(status, 201);
    let (status, refused) = send(
        as_member(
            &api,
            t,
            &hugo_session,
            Method::POST,
            "/api/v1/issues/PLAT-1/comments",
        )
        .json(&mention(outsider.account_id, "Outsider", " and you?")),
    )
    .await;
    assert_eq!(status, 201, "{refused}");
    run_jobs(&api, &db).await;
    let camille_sees = notifications_of(&api, t, &camille_session).await;
    assert_eq!(
        events(&camille_sees),
        [("mentioned".to_owned(), "PLAT-1".to_owned())]
    );
    assert_eq!(camille_sees[0]["excerpt"], "@Camille can you check?");
    assert!(
        notifications_of(&api, t, &outsider_session).await.is_empty(),
        "does not see the ticket"
    );
    let admin_sees = notifications_of(&api, t, &api_admin_session(&api, t)).await;
    assert_eq!(
        events(&admin_sees),
        [
            ("commented".to_owned(), "PLAT-1".to_owned()),
            ("commented".to_owned(), "PLAT-1".to_owned())
        ],
        "the reporter, a watcher, is told of comments"
    );
    let (_, watchers) = send(as_member(
        &api,
        t,
        &hugo_session,
        Method::GET,
        "/api/v1/issues/PLAT-1/watchers",
    ))
    .await;
    assert_eq!(watchers["watching"], true, "commenting, Hugo watches it");

    // A status change: the reporter and the assignee; Hugo chose no email for it.
    let (status, _) = send(
        as_member(
            &api,
            t,
            &hugo_session,
            Method::PUT,
            "/api/v1/me/notification-preferences",
        )
        .json(&json!({
            "events": [{"event": "status_changed", "inApp": true, "email": false}],
            "dailyDigest": false
        })),
    )
    .await;
    assert_eq!(status, 200);
    let (_, transitions) = send(admin(&api, t, Method::GET, "/api/v1/issues/PLAT-1/transitions")).await;
    let start = transitions
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Start")
        .unwrap()["id"]
        .clone();
    testkit::delete_caught_emails(&hugo.email).await;
    let (status, _) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-1/transitions")
            .header("if-match", "*")
            .json(&json!({"transitionId": start})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(queued_emails(&db, &hugo.email).await, 0);
    run_jobs(&api, &db).await;
    let mine = notifications_of(&api, t, &hugo_session).await;
    assert_eq!(mine[0]["event"], "status_changed");
    assert_eq!(mine[0]["status"], "In progress");
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    let emailed: Option<bool> = infra::sqlx::query_scalar(
        "SELECT emailed_at IS NOT NULL FROM notification WHERE account_id = $1 AND event = 'status_changed'",
    )
    .bind(hugo.account_id)
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    assert_eq!(emailed, Some(false), "only in the application");
}

async fn tenant_name(db: &TestDb, tenant: &SeedTenant) -> String {
    infra::sqlx::query_scalar("SELECT name FROM tenant WHERE id = $1")
        .bind(tenant.id)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

fn api_admin_session(api: &TestApi, tenant: &SeedTenant) -> Session {
    let session = api.session(tenant);
    (session.token.clone(), session.csrf_token.clone())
}

#[tokio::test]
async fn a_member_reads_the_notifications_of_both_organizations_and_marks_them_read() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let alpha = plat(&api, a).await;
    let beta = plat(&api, b).await;
    // Lucas belongs to both.
    let (lucas, in_alpha) = member(&db, &api, a).await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &lucas.email,
            display_name: "Test Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let in_beta = open_session(&api, b, lucas.account_id).await;
    grant(&db, &api, a, &alpha, "Contributor", lucas.account_id).await;
    grant(&db, &api, b, &beta, "Contributor", lucas.account_id).await;
    for tenant in [a, b] {
        let (status, _) = send(
            admin(&api, tenant, Method::PATCH, "/api/v1/issues/PLAT-1")
                .header("if-match", "*")
                .json(&json!({"assigneeId": lucas.account_id})),
        )
        .await;
        assert_eq!(status, 200);
    }
    run_jobs(&api, &db).await;

    // Both organizations, from either; one only when filtered.
    let all = notifications_of(&api, a, &in_alpha).await;
    let mut tenants: Vec<&str> = all
        .iter()
        .map(|n| n["tenant"]["slug"].as_str().unwrap())
        .collect();
    tenants.sort_unstable();
    let mut expected = vec![a.slug.as_str(), b.slug.as_str()];
    expected.sort_unstable();
    assert_eq!(tenants, expected);
    assert_eq!(notifications_of(&api, b, &in_beta).await.len(), 2);
    let path = format!("/api/v1/me/notifications?tenantId={}", b.id);
    let (_, filtered) = send(as_member(&api, a, &in_alpha, Method::GET, &path)).await;
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["tenant"]["id"], json!(b.id));
    let (_, unread) = send(as_member(
        &api,
        a,
        &in_alpha,
        Method::GET,
        "/api/v1/me/notifications/unread",
    ))
    .await;
    assert_eq!(unread["total"], 2);

    // Marked read: one, then all; it stays read. Another member's ids change nothing.
    let first = all[0]["id"].clone();
    let (status, left) = send(
        as_member(&api, a, &in_alpha, Method::POST, "/api/v1/me/notifications/read")
            .json(&json!({"ids": [first]})),
    )
    .await;
    assert_eq!((status, &left["total"]), (200, &json!(1)));
    let (status, left) = send(
        admin(&api, a, Method::POST, "/api/v1/me/notifications/read").json(&json!({"ids": [all[1]["id"]]})),
    )
    .await;
    assert_eq!(
        (status, &left["total"]),
        (200, &json!(0)),
        "the admin has none of their own"
    );
    let (_, unread) = send(as_member(
        &api,
        a,
        &in_alpha,
        Method::GET,
        "/api/v1/me/notifications/unread",
    ))
    .await;
    assert_eq!(unread["total"], 1, "the admin could not mark Lucas's");
    let (_, left) =
        send(as_member(&api, b, &in_beta, Method::POST, "/api/v1/me/notifications/read").json(&json!({})))
            .await;
    assert_eq!(left["total"], 0);
    let reread = notifications_of(&api, a, &in_alpha).await;
    assert!(reread.iter().all(|n| n["read"] == true));
    let (_, page) = send(as_member(
        &api,
        a,
        &in_alpha,
        Method::GET,
        "/api/v1/me/notifications?unread=true",
    ))
    .await;
    assert_eq!(page["items"], json!([]));

    // Paging.
    let (_, first_page) = send(as_member(
        &api,
        a,
        &in_alpha,
        Method::GET,
        "/api/v1/me/notifications?limit=1",
    ))
    .await;
    let cursor = first_page["nextCursor"].as_str().unwrap();
    let (_, second) = send(as_member(
        &api,
        a,
        &in_alpha,
        Method::GET,
        &format!("/api/v1/me/notifications?limit=1&cursor={cursor}"),
    ))
    .await;
    assert_ne!(first_page["items"][0]["id"], second["items"][0]["id"]);
    assert_eq!(second["nextCursor"], Value::Null);

    // Once removed from beta, beta's notifications are no longer listed.
    infra::sqlx::query("UPDATE membership SET status = 'suspended' WHERE tenant_id = $1 AND account_id = $2")
        .bind(b.id)
        .bind(lucas.account_id)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let left = notifications_of(&api, a, &in_alpha).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["tenant"]["slug"], a.slug.as_str());
}

#[tokio::test]
async fn preferences_have_defaults_apply_everywhere_and_the_digest_gathers_the_emails() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let project = plat(&api, t).await;
    let (lucas, session) = member(&db, &api, t).await;
    grant(&db, &api, t, &project, "Contributor", lucas.account_id).await;

    let (status, defaults) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        "/api/v1/me/notification-preferences",
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        defaults,
        json!({
            "events": [
                {"event": "created", "inApp": true, "email": true},
                {"event": "assigned", "inApp": true, "email": true},
                {"event": "status_changed", "inApp": true, "email": false},
                {"event": "commented", "inApp": true, "email": false},
                {"event": "mentioned", "inApp": true, "email": true},
            ],
            "dailyDigest": false
        })
    );
    let (status, refused) = send(
        as_member(&api, t, &session, Method::PUT, "/api/v1/me/notification-preferences").json(&json!({
            "events": [{"event": "assigned", "inApp": true, "email": true}, {"event": "assigned", "inApp": false, "email": false}],
            "dailyDigest": false
        })),
    )
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    let (status, chosen) = send(
        as_member(
            &api,
            t,
            &session,
            Method::PUT,
            "/api/v1/me/notification-preferences",
        )
        .json(
            &json!({"events": [{"event": "commented", "inApp": false, "email": false}], "dailyDigest": true}),
        ),
    )
    .await;
    assert_eq!(status, 200, "{chosen}");
    assert_eq!(chosen["dailyDigest"], true);
    assert_eq!(
        chosen["events"][1],
        json!({"event": "assigned", "inApp": true, "email": true}),
        "kept"
    );

    // With the digest: notified in the application, no email yet.
    let (status, _) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"assigneeId": lucas.account_id})),
    )
    .await;
    assert_eq!(status, 200);
    // A comment on a ticket Lucas now watches: he chose nothing for comments.
    let (status, _) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-1/comments")
            .json(&json!({"content": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Done"}]}]}})),
    )
    .await;
    assert_eq!(status, 201);
    run_jobs(&api, &db).await;
    assert_eq!(
        events(&notifications_of(&api, t, &session).await),
        [("assigned".to_owned(), "PLAT-1".to_owned())]
    );
    assert_eq!(queued_emails(&db, &lucas.email).await, 0);

    // The digest, once due: one email, then nothing more.
    let sent = worker::send_due_digests_where(
        &db.app_pool,
        &mailer(),
        Some("https://{tenant}.example.test"),
        &|_| true,
    )
    .await
    .unwrap();
    assert_eq!(sent, 1);
    let digest = testkit::caught_email(&lucas.email).await;
    assert!(digest.subject.starts_with("Votre journée"), "{}", digest.subject);
    assert!(
        digest.text.contains("Test Member vous a assigné PLAT-1"),
        "{}",
        digest.text
    );
    let again = worker::send_due_digests_where(&db.app_pool, &mailer(), None, &|_| true)
        .await
        .unwrap();
    assert_eq!(again, 0, "everything went");
    let candidates = {
        let mut conn = db.app_pool.acquire().await.unwrap();
        infra::notifications::digest_candidates(&mut conn).await.unwrap()
    };
    assert!(
        candidates.iter().all(|c| c.account_id != lucas.account_id),
        "nothing waits any more"
    );
}

#[tokio::test]
async fn watching_is_chosen_and_stays_in_its_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let project = plat(&api, a).await;
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &project, "Reader", lea.account_id).await;
    let path = "/api/v1/issues/PLAT-1/watch";
    let (status, watched) = send(as_member(&api, a, &reader, Method::PUT, path)).await;
    assert_eq!((status, &watched), (200, &json!({"count": 2, "watching": true})));
    let (status, again) = send(as_member(&api, a, &reader, Method::PUT, path)).await;
    assert_eq!(
        (status, &again),
        (200, &json!({"count": 2, "watching": true})),
        "once"
    );
    let (status, left) = send(as_member(&api, a, &reader, Method::DELETE, path)).await;
    assert_eq!((status, &left), (200, &json!({"count": 1, "watching": false})));
    let (outsider, outsider_session) = member(&db, &api, a).await;
    let _ = outsider;
    let (status, hidden) = send(as_member(&api, a, &outsider_session, Method::PUT, path)).await;
    assert_problem(status, &hidden, 404, "not_found");
    for method in [Method::GET, Method::PUT] {
        let path = if method == Method::GET {
            "/api/v1/issues/PLAT-1/watchers"
        } else {
            path
        };
        let (status, hidden) = send(admin(&api, b, method, path)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
}
