//! Integration tests of attachments (L05-US05) on real PostgreSQL, Valkey,
//! S3 storage (SeaweedFS) and ClamAV: a file announced, uploaded straight
//! to the storage through the presigned URL, scanned by the worker, then
//! downloaded and its thumbnail read; the history and the outbox; the
//! maximum size and the quota; a download before the scan and of an
//! infected file refused; the same file stored once; who adds and removes;
//! tenant isolation.

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
    api.request(method, tenant, path)
}

fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
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

const ATTACHMENTS: &str = "/api/v1/issues/PLAT-1/attachments";

/// Sends the relative presigned `url` to the storage, with the Host the
/// browser of `tenant` sends (the reverse proxy forwards `/<bucket>/` as is).
fn to_storage(tenant: &SeedTenant, method: Method, url: &str) -> reqwest::RequestBuilder {
    let endpoint = std::env::var("TEST_S3_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:58333".to_owned());
    reqwest::Client::new()
        .request(method, format!("{endpoint}{url}"))
        .header("host", format!("{}.example.test", tenant.slug))
}

/// Announces `content` as `name`, uploads it and says so: the attachment.
async fn upload(
    api: &TestApi,
    tenant: &SeedTenant,
    session: Option<&Session>,
    name: &str,
    content: &[u8],
) -> Value {
    let request = |method, path: &str| match session {
        Some(session) => as_member(api, tenant, session, method, path),
        None => admin(api, tenant, method, path),
    };
    let (status, announced) = send(
        request(Method::POST, ATTACHMENTS)
            .json(&json!({"name": name, "size": content.len(), "mediaType": "text/plain"})),
    )
    .await;
    assert_eq!(status, 201, "{announced}");
    let put = to_storage(tenant, Method::PUT, announced["uploadUrl"].as_str().unwrap())
        .body(content.to_vec())
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success(), "{}", put.status());
    let id = announced["attachment"]["id"].as_str().unwrap();
    let (status, uploaded) = send(request(Method::POST, &format!("{ATTACHMENTS}/{id}/uploaded"))).await;
    assert_eq!(status, 200, "{uploaded}");
    uploaded
}

/// Runs the queued jobs (scans, removals) as the worker does.
async fn run_jobs(api: &TestApi, db: &TestDb) {
    let jobs = worker::jobs::Jobs {
        mailer: infra::email::Mailer::new(
            &infra::config::Secret::new(testkit::smtp_url()),
            "Planner <noreply@example.org>",
        )
        .unwrap(),
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
        storage: Some(testkit::storage().await),
        clamav: Some(testkit::clamav_addr()),
        public_url: None,
        events: None,
    };
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
}

/// A request to the API that does not follow redirects: the Location of a
/// download is the presigned URL itself.
fn no_redirect(api: &TestApi, tenant: &SeedTenant, path: &str) -> reqwest::RequestBuilder {
    let session = api.session(tenant);
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{}{path}", api.base))
        .header("host", format!("{}.example.test", tenant.slug))
        .header("cookie", format!("mlbpp_session={}", session.token))
}

/// A small PNG image.
fn png() -> Vec<u8> {
    let image = image::RgbImage::from_fn(64, 32, |x, _| image::Rgb([u8::try_from(x * 4).unwrap(), 10, 200]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

async fn objects_of(tenant: &SeedTenant) -> usize {
    testkit::storage()
        .await
        .keys(&format!("{}/", tenant.id))
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_file_goes_to_the_storage_is_scanned_then_downloads_as_an_attachment() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    plat(&api, t).await;

    // Announced: the URL to upload to is on the tenant's own origin.
    let content = b"Recette OIDC: OK".to_vec();
    let (status, announced) = send(
        admin(&api, t, Method::POST, ATTACHMENTS)
            .json(&json!({"name": "C:\\docs\\recette.txt", "size": content.len()})),
    )
    .await;
    assert_eq!(status, 201, "{announced}");
    let url = announced["uploadUrl"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("/{}/{}/uploads/", testkit::TEST_BUCKET, t.id)),
        "a path on the origin the browser uses: {url}"
    );
    assert_eq!(
        (
            &announced["attachment"]["name"],
            &announced["attachment"]["status"]
        ),
        (&json!("recette.txt"), &json!("pending"))
    );
    let id = announced["attachment"]["id"].as_str().unwrap().to_owned();
    let (_, listed) = send(admin(&api, t, Method::GET, ATTACHMENTS)).await;
    assert_eq!(listed, json!([]), "not listed before it is uploaded");
    // Said uploaded before it is: refused.
    let (status, early) = send(admin(
        &api,
        t,
        Method::POST,
        &format!("{ATTACHMENTS}/{id}/uploaded"),
    ))
    .await;
    assert_problem(status, &early, 409, "attachment.not_uploaded");
    let put = to_storage(t, Method::PUT, url)
        .body(content.clone())
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success());
    let (status, uploaded) = send(admin(
        &api,
        t,
        Method::POST,
        &format!("{ATTACHMENTS}/{id}/uploaded"),
    ))
    .await;
    assert_eq!(
        (status, &uploaded["status"]),
        (200, &json!("scanning")),
        "{uploaded}"
    );
    let (status, again) = send(admin(
        &api,
        t,
        Method::POST,
        &format!("{ATTACHMENTS}/{id}/uploaded"),
    ))
    .await;
    assert_eq!(
        (status, &again["status"]),
        (200, &json!("scanning")),
        "said twice: nothing more"
    );

    // Not downloadable while scanning.
    let content_path = format!("{ATTACHMENTS}/{id}/content");
    let (status, waiting) = send(admin(&api, t, Method::GET, &content_path)).await;
    assert_problem(status, &waiting, 409, "attachment.not_scanned");

    run_jobs(&api, &db).await;
    let (_, listed) = send(admin(&api, t, Method::GET, ATTACHMENTS)).await;
    assert_eq!(
        (
            &listed[0]["status"],
            &listed[0]["size"],
            &listed[0]["hasThumbnail"],
            &listed[0]["canDelete"]
        ),
        (
            &json!("clean"),
            &json!(content.len()),
            &json!(false),
            &json!(true)
        )
    );
    assert_eq!(listed[0]["uploadedBy"]["name"], "Test Member");
    let redirected = no_redirect(&api, t, &content_path).send().await.unwrap();
    assert_eq!(redirected.status(), 303);
    assert_eq!(redirected.headers()["cache-control"], "no-store");
    let location = redirected.headers()["location"].to_str().unwrap().to_owned();
    let file = to_storage(t, Method::GET, &location).send().await.unwrap();
    assert_eq!(
        file.headers()["content-type"],
        "application/octet-stream",
        "never rendered"
    );
    assert!(
        file.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"recette.txt\"")
    );
    assert_eq!(file.bytes().await.unwrap().as_ref(), content.as_slice());
    let (status, none) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("{ATTACHMENTS}/{id}/thumbnail"),
    ))
    .await;
    assert_problem(status, &none, 404, "not_found");

    // An image: a thumbnail, served as a PNG.
    let image = upload(&api, t, None, "screen.png", &png()).await;
    run_jobs(&api, &db).await;
    let (_, listed) = send(admin(&api, t, Method::GET, ATTACHMENTS)).await;
    assert_eq!(listed[1]["hasThumbnail"], true);
    let thumbnail = no_redirect(
        &api,
        t,
        &format!("{ATTACHMENTS}/{}/thumbnail", image["id"].as_str().unwrap()),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(thumbnail.status(), 303);
    let png = to_storage(t, Method::GET, thumbnail.headers()["location"].to_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(png.headers()["content-type"], "image/png");
    assert!(png.bytes().await.unwrap().starts_with(b"\x89PNG"));

    // The history and the outbox tell it.
    let (_, activity) = send(admin(&api, t, Method::GET, "/api/v1/issues/PLAT-1/activity")).await;
    assert_eq!(activity["items"][0]["action"], "attachment_added");
    assert_eq!(activity["items"][0]["new"]["text"], "screen.png");
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    let events: i64 =
        infra::sqlx::query_scalar("SELECT count(*) FROM outbox_event WHERE kind = 'attachment.added'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(events, 2);
}

#[tokio::test]
async fn sizes_quotas_and_viruses_are_refused_and_the_same_file_is_stored_once() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    plat(&api, t).await;
    let announce = |body: Value| send(admin(&api, t, Method::POST, ATTACHMENTS).json(&body));

    let (status, large) = announce(json!({"name": "video.mp4", "size": 51 * 1024 * 1024})).await;
    assert_problem(status, &large, 413, "attachment.too_large");
    let (status, empty) = announce(json!({"name": "empty.txt", "size": 0})).await;
    assert_problem(status, &empty, 400, "validation_failed");
    assert_eq!(empty["errors"][0], json!({"field": "size", "code": "empty"}));
    let (status, unnamed) = announce(json!({"name": "dir/", "size": 3})).await;
    assert_problem(status, &unnamed, 400, "validation_failed");
    assert_eq!(unnamed["errors"][0]["field"], "name");

    // The same content twice: one object.
    upload(&api, t, None, "a.txt", b"same content").await;
    upload(&api, t, None, "b.txt", b"same content").await;
    run_jobs(&api, &db).await;
    assert_eq!(objects_of(t).await, 1);
    let (_, listed) = send(admin(&api, t, Method::GET, ATTACHMENTS)).await;
    assert!(listed.as_array().unwrap().iter().all(|a| a["status"] == "clean"));

    // The quota counts that object once: 12 bytes used of 20.
    infra::sqlx::query("UPDATE tenant SET storage_quota_bytes = 20 WHERE id = $1")
        .bind(t.id)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let (status, over) = announce(json!({"name": "c.txt", "size": 9})).await;
    assert_problem(status, &over, 413, "attachment.quota_exceeded");
    let (status, _) = announce(json!({"name": "c.txt", "size": 8})).await;
    assert_eq!(status, 201, "up to the quota");
    infra::sqlx::query("UPDATE tenant SET storage_quota_bytes = NULL WHERE id = $1")
        .bind(t.id)
        .execute(&db.owner_pool)
        .await
        .unwrap();

    // The test virus: infected, removed, never downloadable.
    let virus = upload(&api, t, None, "invoice.exe", &testkit::eicar()).await;
    run_jobs(&api, &db).await;
    let (_, listed) = send(admin(&api, t, Method::GET, ATTACHMENTS)).await;
    let infected = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == virus["id"])
        .unwrap();
    assert_eq!(
        (&infected["status"], &infected["signature"]),
        (&json!("infected"), &json!("Eicar-Test-Signature.UNOFFICIAL"))
    );
    let (status, refused) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("{ATTACHMENTS}/{}/content", virus["id"].as_str().unwrap()),
    ))
    .await;
    assert_problem(status, &refused, 409, "attachment.not_scanned");
    assert_eq!(
        objects_of(t).await,
        1,
        "its content is gone (the pending upload never came)"
    );
}

#[tokio::test]
async fn editors_attach_their_uploaders_and_moderators_remove_and_readers_only_read() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let project = plat(&api, a).await;
    let (hugo, contributor) = member(&db, &api, a).await;
    grant(&db, &api, a, &project, "Contributor", hugo).await;
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &project, "Reader", lea).await;

    let mine = upload(&api, a, Some(&contributor), "mine.txt", b"by Hugo").await;
    let theirs = upload(&api, a, None, "admin.txt", b"by the admin").await;
    run_jobs(&api, &db).await;
    let path = |attachment: &Value| format!("{ATTACHMENTS}/{}", attachment["id"].as_str().unwrap());

    // A reader reads and downloads, attaches and removes nothing.
    let (status, listed) = send(as_member(&api, a, &reader, Method::GET, ATTACHMENTS)).await;
    assert_eq!((status, listed.as_array().unwrap().len()), (200, 2));
    assert!(listed.as_array().unwrap().iter().all(|a| a["canDelete"] == false));
    let (status, refused) = send(
        as_member(&api, a, &reader, Method::POST, ATTACHMENTS).json(&json!({"name": "x.txt", "size": 1})),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    let (status, refused) = send(as_member(&api, a, &reader, Method::DELETE, &path(&mine))).await;
    assert_problem(status, &refused, 403, "permission.denied");

    // Hugo removes his own, not the admin's; nor completes another's upload.
    let (status, refused) = send(as_member(&api, a, &contributor, Method::DELETE, &path(&theirs))).await;
    assert_problem(status, &refused, 403, "permission.denied");
    let (status, refused) = send(as_member(
        &api,
        a,
        &contributor,
        Method::POST,
        &format!("{}/uploaded", path(&theirs)),
    ))
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    let (status, _) = send(as_member(&api, a, &contributor, Method::DELETE, &path(&mine))).await;
    assert_eq!(status, 204);
    // The admin moderates.
    let (status, _) = send(admin(&api, a, Method::DELETE, &path(&theirs))).await;
    assert_eq!(status, 204);
    run_jobs(&api, &db).await;
    assert_eq!(objects_of(a).await, 0, "their content left the storage");
    let (_, activity) = send(admin(&api, a, Method::GET, "/api/v1/issues/PLAT-1/activity")).await;
    let actions: Vec<&str> = activity["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        &actions[..4],
        [
            "attachment_removed",
            "attachment_removed",
            "attachment_added",
            "attachment_added"
        ]
    );
    let _ = hugo;

    // Another tenant sees nothing.
    let id = theirs["id"].as_str().unwrap();
    for (method, path) in [
        (Method::GET, ATTACHMENTS.to_owned()),
        (Method::POST, ATTACHMENTS.to_owned()),
        (Method::GET, format!("{ATTACHMENTS}/{id}/content")),
        (Method::DELETE, format!("{ATTACHMENTS}/{id}")),
    ] {
        let (status, hidden) =
            send(admin(&api, b, method, &path).json(&json!({"name": "x", "size": 1}))).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
}
