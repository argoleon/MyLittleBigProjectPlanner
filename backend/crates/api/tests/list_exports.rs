//! Integration tests of list view exports (L06-US06) on real PostgreSQL,
//! Valkey, S3 storage (SeaweedFS) and SMTP (Mailpit): a small export
//! written at once with the columns and headers asked for, values escaped
//! and unambiguous; a large one written by the worker, its requester
//! notified in the bell and by email; the tickets the member sees only;
//! refusals; a failed run; the file deleted once expired; other members and
//! tenants kept out.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
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

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (TestMember, Session) {
    let member = db.add_member(tenant, TenantRole::Member).await;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(member.account_id, tenant.id)
    .await
    .unwrap();
    (member, (opened.token, opened.session.csrf_token))
}

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: Uuid, label: &str, account: Uuid) {
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

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, key: &str) -> Uuid {
    let (status, node) = send(
        admin(api, tenant, Method::POST, "/api/v1/nodes")
            .json(&json!({"type": "project", "name": name, "key": key})),
    )
    .await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().parse().unwrap()
}

/// `count` tasks `KEY-first…` of `project` in To do, written directly; the
/// summary of each is `summary` followed by its number.
async fn tickets(
    db: &TestDb,
    tenant: &SeedTenant,
    project: Uuid,
    key: &str,
    first: i32,
    count: i32,
    summary: &str,
) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::sqlx::query(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority,
                           created_at)
        SELECT uuidv7(), app_current_tenant(), n.id, s, $2 || '-' || s,
               (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE t.key = 'task'),
               (SELECT st.item_id FROM workflow_status st JOIN project_config_version v ON v.id = st.version_id
                AND v.state = 'published' AND v.config_id = n.config_id WHERE st.key = 'todo'),
               $5 || s, 'high', '2026-11-18T09:12:00.123456Z'::timestamptz
        FROM node n, generate_series($3::int, $3::int + $4::int - 1) AS s
        WHERE n.id = $1
        "#,
    )
    .bind(project)
    .bind(key)
    .bind(first)
    .bind(count)
    .bind(summary)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Runs the queued jobs (exports, dispatches, emails) as the worker does.
async fn run_jobs(api: &TestApi, db: &TestDb) {
    let jobs = worker::jobs::Jobs {
        mailer: infra::email::Mailer::new(
            &infra::config::Secret::new(testkit::smtp_url()),
            "Planner <noreply@example.org>",
        )
        .unwrap(),
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
        storage: Some(testkit::storage().await),
        clamav: None,
        public_url: Some("https://{tenant}.example.test".to_owned()),
        events: None,
    };
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
}

/// The file of the export `id`, downloaded as the browser does: the API's
/// redirect, then the storage.
async fn download(api: &TestApi, tenant: &SeedTenant, id: &str) -> (String, String, Vec<u8>) {
    let session = api.session(tenant);
    let redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{}/api/v1/exports/{id}/file", api.base))
        .header("host", format!("{}.example.test", tenant.slug))
        .header("cookie", format!("mlbpp_session={}", session.token))
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status(), 303);
    let location = redirect.headers()["location"].to_str().unwrap().to_owned();
    let endpoint = std::env::var("TEST_S3_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:58333".to_owned());
    let file = reqwest::Client::new()
        .get(format!("{endpoint}{location}"))
        .header("host", format!("{}.example.test", tenant.slug))
        .send()
        .await
        .unwrap();
    assert_eq!(file.status(), 200);
    let content_type = file.headers()["content-type"].to_str().unwrap().to_owned();
    let disposition = file.headers()["content-disposition"].to_str().unwrap().to_owned();
    (content_type, disposition, file.bytes().await.unwrap().to_vec())
}

fn today() -> String {
    time::OffsetDateTime::now_utc().date().to_string()
}

#[tokio::test]
async fn a_small_export_is_written_at_once_with_the_columns_asked_for() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    tickets(&db, t, plat, "PLAT", 1, 1, "Plain ").await;
    tickets(&db, t, plat, "PLAT", 2, 1, "=SUM(A1), \"quoted\" ").await;
    tickets(&db, t, plat, "PLAT", 3, 1, "-1 accès ").await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    infra::sqlx::query("UPDATE issue SET due_date = '2026-11-20' WHERE key_number = 1")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (status, export) = send(
        api.request(Method::POST, t, "/api/v1/exports")
            .header("accept-language", "fr")
            .json(&json!({
                "query": "project = PLAT ORDER BY key ASC",
                "columns": ["key", "summary", "status", "priority", "assignee", "due", "created"],
                "name": "search"
            })),
    )
    .await;
    assert_eq!(status, 201, "{export}");
    assert_eq!(
        (
            &export["status"],
            &export["rows"],
            &export["fileName"],
            &export["format"]
        ),
        (
            &json!("ready"),
            &json!(3),
            &json!(format!("search-{}.csv", today())),
            &json!("csv")
        )
    );
    let id = export["id"].as_str().unwrap();
    assert_eq!(export["downloadPath"], format!("/api/v1/exports/{id}/file"));

    let (content_type, disposition, bytes) = download(&api, t, id).await;
    assert_eq!(content_type, "text/csv; charset=utf-8");
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(
        disposition.contains(&format!("search-{}.csv", today())),
        "{disposition}"
    );
    assert_eq!(bytes[..3], [0xEF, 0xBB, 0xBF], "UTF-8 with a byte order mark");
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(
        text,
        "\u{feff}Clé,Titre,Statut,Priorité,Responsable,Échéance,Créé le\r\n\
         PLAT-1,Plain 1,À faire,Haute,,2026-11-20,2026-11-18T09:12:00Z\r\n\
         PLAT-2,\"'=SUM(A1), \"\"quoted\"\" 2\",À faire,Haute,,,2026-11-18T09:12:00Z\r\n\
         PLAT-3,'-1 accès 3,À faire,Haute,,,2026-11-18T09:12:00Z\r\n"
    );
    // Listed and read back by its requester.
    let (status, list) = send(admin(&api, t, Method::GET, "/api/v1/exports")).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["items"][0]["id"], id);
    let (status, read) = send(admin(&api, t, Method::GET, &format!("/api/v1/exports/{id}"))).await;
    assert_eq!((status, &read["status"]), (200, &json!("ready")));
}

#[tokio::test]
async fn a_large_export_runs_in_the_background_and_notifies_its_requester() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let rows = i32::try_from(domain::export::SYNC_MAX_ROWS).unwrap() + 1;
    tickets(&db, t, plat, "PLAT", 1, rows, "Ticket ").await;
    let (lucas, session) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Reader", lucas.account_id).await;
    // Told by email too.
    let (status, preferences) = send(
        as_member(
            &api,
            t,
            &session,
            Method::PUT,
            "/api/v1/me/notification-preferences",
        )
        .json(&json!({
            "events": [{"event": "export_ready", "inApp": true, "email": true}],
            "dailyDigest": true
        })),
    )
    .await;
    assert_eq!(status, 200, "{preferences}");
    let export_choice = preferences["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["event"] == "export_ready")
        .unwrap()
        .clone();
    assert_eq!(
        export_choice,
        json!({"event": "export_ready", "inApp": true, "email": true})
    );

    let (status, export) = send(
        as_member(&api, t, &session, Method::POST, "/api/v1/exports").json(&json!({
            "query": "project = PLAT",
            "columns": ["key", "summary"],
            "name": "my-tickets"
        })),
    )
    .await;
    assert_eq!(status, 202, "{export}");
    assert_eq!(
        (&export["status"], &export["rows"], &export["downloadPath"]),
        (&json!("queued"), &Value::Null, &Value::Null)
    );
    let id = export["id"].as_str().unwrap().to_owned();
    let (status, early) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/exports/{id}/file"),
    ))
    .await;
    assert_problem(status, &early, 409, "export.not_ready");

    run_jobs(&api, &db).await;
    let (status, ready) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/exports/{id}"),
    ))
    .await;
    assert_eq!(status, 200, "{ready}");
    assert_eq!(
        (&ready["status"], &ready["rows"]),
        (&json!("ready"), &json!(rows))
    );
    assert!(ready["sizeBytes"].as_i64().unwrap() > 10_000);

    // In the bell: no ticket, the export.
    let (status, notifications) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        "/api/v1/me/notifications",
    ))
    .await;
    assert_eq!(status, 200, "{notifications}");
    let notice = &notifications["items"][0];
    assert_eq!(
        (
            &notice["event"],
            &notice["issueId"],
            &notice["issueKey"],
            &notice["read"]
        ),
        (&json!("export_ready"), &Value::Null, &Value::Null, &json!(false))
    );
    assert_eq!(
        notice["export"],
        json!({"id": id, "fileName": format!("my-tickets-{}.csv", today()), "rows": rows})
    );
    // By email at once, despite the daily digest, in the member's language
    // (the organization's: French).
    let email = testkit::caught_email(&lucas.email).await;
    assert_eq!(
        email.subject,
        format!("Votre export my-tickets-{}.csv est prêt · {}", today(), t.name)
    );
    assert!(
        email
            .text
            .contains(&format!("https://{}.example.test/exports/{id}", t.slug)),
        "{}",
        email.text
    );
}

#[tokio::test]
async fn an_export_holds_the_tickets_its_member_sees_and_stays_theirs() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let ops = project(&api, t, "Operations", "OPS").await;
    tickets(&db, t, plat, "PLAT", 1, 2, "Ticket ").await;
    tickets(&db, t, ops, "OPS", 1, 2, "Ticket ").await;
    let (lucas, session) = member(&db, &api, t).await;
    grant(&db, &api, t, plat, "Reader", lucas.account_id).await;
    let (status, export) = send(
        as_member(&api, t, &session, Method::POST, "/api/v1/exports")
            .json(&json!({"query": "ORDER BY key ASC", "columns": ["key"]})),
    )
    .await;
    assert_eq!(status, 201, "{export}");
    assert_eq!(export["rows"], 2);
    assert_eq!(export["fileName"], format!("export-{}.csv", today()));
    let id = export["id"].as_str().unwrap().to_owned();

    // Another member, the admin included, and another tenant: no such export.
    let (_, other) = member(&db, &api, t).await;
    for path in [
        format!("/api/v1/exports/{id}"),
        format!("/api/v1/exports/{id}/file"),
    ] {
        let (status, hidden) = send(as_member(&api, t, &other, Method::GET, &path)).await;
        assert_problem(status, &hidden, 404, "not_found");
        let (status, hidden) = send(admin(&api, t, Method::GET, &path)).await;
        assert_problem(status, &hidden, 404, "not_found");
        let (status, hidden) = send(admin(&api, &db.tenant_b, Method::GET, &path)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    let (status, list) = send(as_member(&api, t, &other, Method::GET, "/api/v1/exports")).await;
    assert_eq!((status, &list["items"]), (200, &json!([])));
}

#[tokio::test]
async fn an_export_is_refused_when_it_cannot_run() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let refused = |body: Value| send(admin(&api, t, Method::POST, "/api/v1/exports").json(&body));
    let (status, problem) = refused(json!({"query": "project = ", "columns": ["key"]})).await;
    assert_problem(status, &problem, 400, "query.invalid");
    assert_eq!(problem["query"]["position"], 10);
    let (status, problem) = refused(json!({"query": "", "columns": ["key", "points"]})).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(
        problem["errors"],
        json!([{"field": "columns.1", "code": "unknown_field"}])
    );
    let (status, problem) = refused(json!({"query": "", "columns": []})).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(
        problem["errors"],
        json!([{"field": "columns", "code": "required"}])
    );
    // More tickets than an export holds.
    let too_many = i32::try_from(domain::export::MAX_ROWS).unwrap() + 1;
    tickets(&db, t, plat, "PLAT", 1, too_many, "Ticket ").await;
    let (status, problem) = refused(json!({"query": "project = PLAT", "columns": ["key"]})).await;
    assert_problem(status, &problem, 422, "export.too_large");
    assert_eq!(
        problem["detail"],
        format!("the query finds {too_many} tickets; an export holds 100000 at most")
    );
}

#[tokio::test]
async fn a_background_export_whose_query_no_longer_runs_fails() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let rows = i32::try_from(domain::export::SYNC_MAX_ROWS).unwrap() + 1;
    tickets(&db, t, plat, "PLAT", 1, rows, "Ticket ").await;
    let (status, export) = send(
        admin(&api, t, Method::POST, "/api/v1/exports")
            .json(&json!({"query": "project = PLAT", "columns": ["key"]})),
    )
    .await;
    assert_eq!(status, 202, "{export}");
    let id: Uuid = export["id"].as_str().unwrap().parse().unwrap();
    // A field it names deleted since it was asked.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    infra::sqlx::query("UPDATE list_export SET query = 'legacy_points = 3' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    run_jobs(&api, &db).await;
    let (status, failed) = send(admin(&api, t, Method::GET, &format!("/api/v1/exports/{id}"))).await;
    assert_eq!(status, 200, "{failed}");
    assert_eq!(
        (&failed["status"], &failed["failure"], &failed["downloadPath"]),
        (&json!("failed"), &json!("query.invalid"), &Value::Null)
    );
}

#[tokio::test]
async fn an_expired_export_goes_with_its_file() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    tickets(&db, t, plat, "PLAT", 1, 2, "Ticket ").await;
    let (status, export) =
        send(admin(&api, t, Method::POST, "/api/v1/exports").json(&json!({"query": "", "columns": ["key"]})))
            .await;
    assert_eq!(status, 201, "{export}");
    let id: Uuid = export["id"].as_str().unwrap().parse().unwrap();
    let key = infra::list_exports::object_key(t.id, id, "csv");
    let storage = testkit::storage().await;
    assert!(storage.size_of(&key).await.unwrap().is_some());
    // A day later: hidden at once, then deleted by its job.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    infra::sqlx::query("UPDATE list_export SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let due: i64 = infra::sqlx::query_scalar(
        "WITH due AS (UPDATE job SET run_at = now() WHERE kind = 'export.purge' AND payload ->> 'exportId' = $1 \
         RETURNING 1) SELECT count(*) FROM due",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(due, 1, "one purge per export, queued with it");
    let (status, gone) = send(admin(&api, t, Method::GET, &format!("/api/v1/exports/{id}"))).await;
    assert_problem(status, &gone, 404, "not_found");
    run_jobs(&api, &db).await;
    assert_eq!(storage.size_of(&key).await.unwrap(), None);
    let left: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM list_export WHERE id = $1")
        .bind(id)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// The cells of the first sheet of an `.xlsx` file.
fn sheet_of(bytes: Vec<u8>) -> Vec<Vec<calamine::Data>> {
    use calamine::Reader;
    let mut book: calamine::Xlsx<_> = calamine::open_workbook_from_rs(std::io::Cursor::new(bytes)).unwrap();
    let range = book.worksheet_range_at(0).unwrap().unwrap();
    range.rows().map(<[calamine::Data]>::to_vec).collect()
}

#[tokio::test]
async fn an_excel_export_has_typed_cells_at_once_or_in_the_background() {
    use calamine::Data;

    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    tickets(&db, t, plat, "PLAT", 1, 2, "=Ticket ").await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    infra::sqlx::query("UPDATE issue SET due_date = '2026-11-20' WHERE key_number = 1")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (status, export) = send(
        api.request(Method::POST, t, "/api/v1/exports")
            .header("accept-language", "en")
            .json(&json!({
                "query": "project = PLAT ORDER BY key ASC",
                "columns": ["key", "summary", "due", "created"],
                "format": "xlsx",
                "name": "search"
            })),
    )
    .await;
    assert_eq!(status, 201, "{export}");
    assert_eq!(
        (&export["format"], &export["fileName"]),
        (&json!("xlsx"), &json!(format!("search-{}.xlsx", today())))
    );
    let (content_type, disposition, bytes) = download(&api, t, export["id"].as_str().unwrap()).await;
    assert_eq!(
        content_type,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    assert!(disposition.contains(".xlsx"), "{disposition}");
    let sheet = sheet_of(bytes);
    assert_eq!(
        sheet[0],
        ["Key", "Title", "Due date", "Created (UTC)"].map(|h| Data::String(h.to_owned()))
    );
    assert_eq!(sheet[1][0], Data::String("PLAT-1".to_owned()));
    // A text starting like a formula stays a text, as written.
    assert_eq!(sheet[1][1], Data::String("=Ticket 1".to_owned()));
    let (Data::DateTime(due), Data::DateTime(created)) = (&sheet[1][2], &sheet[1][3]) else {
        panic!("dates expected: {:?}", sheet[1]);
    };
    assert!(
        (due.as_f64() - 46_346.0).abs() < 1e-9,
        "2026-11-20: {}",
        due.as_f64()
    );
    let at_09_12 = 46_344.0 + (9.0 * 3600.0 + 12.0 * 60.0) / 86_400.0;
    assert!(
        (created.as_f64() - at_09_12).abs() < 1e-6,
        "2026-11-18 09:12 UTC: {}",
        created.as_f64()
    );
    assert_eq!(sheet[2][2], Data::Empty);

    // Beyond 1,000 tickets: in the background, the same cells.
    let rows = i32::try_from(domain::export::SYNC_MAX_ROWS).unwrap() + 1;
    tickets(&db, t, plat, "PLAT", 3, rows - 2, "Ticket ").await;
    let (status, queued) = send(admin(&api, t, Method::POST, "/api/v1/exports").json(&json!({
        "query": "project = PLAT ORDER BY key ASC",
        "columns": ["key"],
        "format": "xlsx"
    })))
    .await;
    assert_eq!(status, 202, "{queued}");
    run_jobs(&api, &db).await;
    let id = queued["id"].as_str().unwrap();
    let (status, ready) = send(admin(&api, t, Method::GET, &format!("/api/v1/exports/{id}"))).await;
    assert_eq!(
        (status, &ready["status"], &ready["rows"]),
        (200, &json!("ready"), &json!(rows))
    );
    let (_, _, bytes) = download(&api, t, id).await;
    let sheet = sheet_of(bytes);
    assert_eq!(
        sheet.len(),
        usize::try_from(rows).unwrap() + 1,
        "the headers, then a row per ticket"
    );
    assert_eq!(sheet[1001][0], Data::String("PLAT-1001".to_owned()));
}
