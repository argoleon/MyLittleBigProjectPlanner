//! Integration tests of real time (L05-US08) on real PostgreSQL and Valkey,
//! two API instances sharing them and the worker's queue running: a change
//! made through one instance reaches, within seconds, the browsers that
//! follow the ticket on the other; never those who do not see its project,
//! nor another tenant; new notifications reach their recipient only; a
//! browser that comes back resumes where it was, or is told to read
//! everything again; no stream without a session.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{TestApi, json, multi};
use fred::prelude::StreamsInterface;
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

type Session = (String, String);

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (Uuid, Session) {
    let account = db.add_member(tenant, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account, tenant.id)
    .await
    .unwrap();
    (account, (opened.token, opened.session.csrf_token))
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

/// The project PLAT with the story PLAT-1; the project's id.
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
            .json(&json!({"typeId": story, "summary": "OIDC"})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    node["id"].as_str().unwrap().to_owned()
}

/// Runs the queued jobs (dispatches, signals, emails) as the worker does.
async fn run_jobs(api: &TestApi, db: &TestDb) {
    let jobs = worker::jobs::Jobs {
        mailer: infra::email::Mailer::new(
            &infra::config::Secret::new(testkit::smtp_url()),
            "Planner <noreply@example.org>",
        )
        .unwrap(),
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
        storage: None,
        clamav: None,
        public_url: None,
        events: Some(infra::events::EventLog::new(
            api.valkey.clone(),
            api.key_prefix.clone(),
        )),
    };
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
}

/// An event of the stream.
#[derive(Debug, Clone, PartialEq)]
struct Sse {
    id: Option<String>,
    event: String,
    data: Value,
}

/// A browser following `/api/v1/events`.
struct Follower {
    response: reqwest::Response,
    buffer: String,
}

impl Follower {
    async fn open(
        api: &TestApi,
        tenant: &SeedTenant,
        session: &Session,
        last_event_id: Option<&str>,
    ) -> (u16, Option<Self>) {
        let mut request = api
            .anonymous(Method::GET, tenant, "/api/v1/events")
            .header("cookie", format!("mlbpp_session={}", session.0))
            .header("accept", "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        if status != 200 {
            return (status, None);
        }
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        (
            status,
            Some(Self {
                response,
                buffer: String::new(),
            }),
        )
    }

    /// The next event (comments skipped), if one comes within `wait`.
    async fn next(&mut self, wait: Duration) -> Option<Sse> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let (mut id, mut event, mut data) = (None, String::from("message"), String::new());
                for line in block.lines() {
                    if let Some(value) = line.strip_prefix("id:") {
                        id = Some(value.trim().to_owned());
                    } else if let Some(value) = line.strip_prefix("event:") {
                        event = value.trim().to_owned();
                    } else if let Some(value) = line.strip_prefix("data:") {
                        data.push_str(value.trim());
                    }
                }
                if data.is_empty() {
                    continue;
                }
                return Some(Sse {
                    id,
                    event,
                    data: serde_json::from_str(&data).unwrap(),
                });
            }
            let chunk = tokio::time::timeout_at(deadline, self.response.chunk())
                .await
                .ok()?
                .unwrap()?;
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    /// The next event, which must come.
    async fn expect(&mut self) -> Sse {
        self.next(Duration::from_secs(10))
            .await
            .expect("an event within 10 s")
    }
}

#[tokio::test]
async fn a_change_on_one_instance_reaches_those_who_see_it_on_the_other() {
    let db = TestDb::new().await;
    let first = common::start(&db, multi()).await;
    let second = common::another_instance(&db, &first).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let project = plat(&first, a).await;
    let (hugo, hugo_session) = member(&db, &first, a).await;
    grant(&db, &first, a, &project, "Contributor", hugo).await;
    let (_, outsider_session) = member(&db, &first, a).await;
    let (_, beta_session) = member(&db, &first, b).await;
    run_jobs(&first, &db).await;

    // Three browsers follow on the second instance.
    let (_, hugo_follows) = Follower::open(&second, a, &hugo_session, None).await;
    let mut hugo_follows = hugo_follows.unwrap();
    let ready = hugo_follows.expect().await;
    assert_eq!(ready.event, "ready");
    let ready_id = ready.id.unwrap();
    let mut outsider = Follower::open(&second, a, &outsider_session, None)
        .await
        .1
        .unwrap();
    assert_eq!(outsider.expect().await.event, "ready");
    let mut beta = Follower::open(&second, b, &beta_session, None).await.1.unwrap();
    assert_eq!(beta.expect().await.event, "ready");

    // The admin starts PLAT-1 through the first instance; the worker dispatches it.
    let (_, transitions) = send(admin(&first, a, Method::GET, "/api/v1/issues/PLAT-1/transitions")).await;
    let start = transitions
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Start")
        .unwrap()["id"]
        .clone();
    let (status, _) = send(
        admin(&first, a, Method::POST, "/api/v1/issues/PLAT-1/transitions")
            .header("if-match", "*")
            .json(&json!({"transitionId": start})),
    )
    .await;
    assert_eq!(status, 200);
    let changed = tokio::time::Instant::now();
    run_jobs(&first, &db).await;
    let signal = hugo_follows.expect().await;
    assert!(changed.elapsed() < Duration::from_secs(5), "within seconds");
    assert_eq!(signal.event, "issue");
    assert_eq!(
        (
            &signal.data["issueKey"],
            &signal.data["kind"],
            &signal.data["actorId"]
        ),
        (
            &json!("PLAT-1"),
            &json!("issue.transitioned"),
            &json!(first.session(a).member.account_id)
        )
    );
    assert!(
        signal.data.get("recipients").is_none() && signal.data.get("projectId").is_none(),
        "a signal, no data"
    );
    let change_id = signal.id.unwrap();

    // The admin mentions Hugo: the ticket changes, and Hugo has a notification.
    let mention = json!({"content": {"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "mention", "attrs": {"id": hugo, "label": "Hugo"}}, {"type": "text", "text": " look"}
    ]}]}});
    let (status, _) =
        send(admin(&first, a, Method::POST, "/api/v1/issues/PLAT-1/comments").json(&mention)).await;
    assert_eq!(status, 201);
    run_jobs(&first, &db).await;
    let mut seen = [hugo_follows.expect().await, hugo_follows.expect().await];
    seen.sort_by(|x, y| x.event.cmp(&y.event));
    assert_eq!(
        seen.iter()
            .map(|e| (e.event.as_str(), e.data["kind"].clone()))
            .collect::<Vec<_>>(),
        [
            ("issue", json!("comment.created")),
            ("notifications", Value::Null)
        ]
    );

    // The others got nothing.
    assert_eq!(
        outsider.next(Duration::from_secs(2)).await,
        None,
        "does not see the project"
    );
    assert_eq!(beta.next(Duration::from_secs(1)).await, None, "another tenant");

    // Hugo comes back after a cut: resumes from the last event he saw.
    drop(hugo_follows);
    let mut back = Follower::open(&first, a, &hugo_session, Some(&ready_id))
        .await
        .1
        .unwrap();
    assert_eq!(back.expect().await.event, "ready");
    let replayed = back.expect().await;
    assert_eq!(
        (replayed.id.as_deref(), replayed.event.as_str()),
        (Some(change_id.as_str()), "issue")
    );
}

#[tokio::test]
async fn a_browser_too_far_behind_reads_everything_again_and_none_follows_without_a_session() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    plat(&api, a).await;
    let log = infra::events::EventLog::new(api.valkey.clone(), api.key_prefix.clone());
    let admin_session = (api.session(a).token.clone(), api.session(a).csrf_token.clone());
    // Something in the log already, so the browser starts after it.
    run_jobs(&api, &db).await;
    let mut first = Follower::open(&api, a, &admin_session, None).await.1.unwrap();
    let old = first.expect().await.id.unwrap();
    drop(first);
    for _ in 0..3 {
        let (status, _) = send(
            admin(&api, a, Method::PATCH, "/api/v1/issues/PLAT-1")
                .header("if-match", "*")
                .json(&json!({"summary": format!("OIDC {}", Uuid::now_v7())})),
        )
        .await;
        assert_eq!(status, 200);
    }
    run_jobs(&api, &db).await;
    // The log was trimmed meanwhile (a busy tenant).
    let _: i64 = api.valkey.xtrim(log.key(a.id), ("MAXLEN", 1)).await.unwrap();
    let mut back = Follower::open(&api, a, &admin_session, Some(&old))
        .await
        .1
        .unwrap();
    assert_eq!(back.expect().await.event, "resync");
    assert_eq!(back.expect().await.event, "issue", "then what is still there");

    // An id that is not one is ignored: the stream starts now.
    let mut odd = Follower::open(&api, a, &admin_session, Some("not-an-id"))
        .await
        .1
        .unwrap();
    assert_eq!(odd.expect().await.event, "ready");

    let (status, _) = Follower::open(&api, a, &("nope".to_owned(), String::new()), None).await;
    assert_eq!(status, 401);
}
