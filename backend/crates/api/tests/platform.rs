//! Integration tests of the platform endpoints (L02-US01) on real
//! PostgreSQL and Valkey: tenant creation, settings and quotas, deletion with
//! its background purge, and access for platform super admins only.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use domain::tenancy::Tenancy;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const PATH: &str = "/api/v1/platform/tenants";

/// The API, with the signed-in administrator of tenant alpha made platform
/// super admin (the one of beta stays a mere tenant admin).
async fn start_as_super_admin(db: &TestDb, tenancy: Tenancy) -> TestApi {
    let api = start(db, tenancy).await;
    db.make_platform_admin(api.session(&db.tenant_a).member.account_id)
        .await;
    api
}

fn acme() -> Value {
    json!({"slug": "acme", "name": "Acme Industrie", "defaultLocale": "en", "timeZone": "America/New_York"})
}

async fn send(
    api: &TestApi,
    tenant: &SeedTenant,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = api.request(method, tenant, path);
    if let Some(body) = body {
        request = request.json(&body);
    }
    json(request.send().await.unwrap()).await
}

async fn create(api: &TestApi, db: &TestDb, body: Value) -> (u16, Value) {
    send(api, &db.tenant_a, Method::POST, PATH, Some(body)).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

/// Status of `GET /api/v1/tenant` on the host of `slug`.
async fn resolves(api: &TestApi, slug: &str) -> u16 {
    api.client
        .get(format!("{}/api/v1/tenant", api.base))
        .header("host", format!("{slug}.example.test"))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn a_super_admin_creates_a_tenant_reachable_at_its_subdomain() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    assert_eq!(resolves(&api, "acme").await, 404);

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .json(&json!({
            "slug": "acme", "name": "Acme Industrie", "defaultLocale": "en", "timeZone": "America/New_York",
            "defaultCapacityMinutes": [480, 480, 480, 480, 240, 0, 0],
            "overloadTolerancePercent": 10, "overloadMode": "warning"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["etag"], "\"1\"");
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    let (_, tenant) = json(res).await;
    assert_eq!(location, format!("{PATH}/{}", tenant["id"].as_str().unwrap()));
    assert_eq!(tenant["slug"], "acme");
    assert_eq!(tenant["url"], "https://acme.example.test");
    assert_eq!(tenant["defaultLocale"], "en");
    assert_eq!(tenant["timeZone"], "America/New_York");
    assert_eq!(
        tenant["defaultCapacityMinutes"],
        json!([480, 480, 480, 480, 240, 0, 0])
    );
    assert_eq!(tenant["overloadTolerancePercent"], 10);
    assert_eq!(tenant["overloadMode"], "warning");
    assert_eq!(tenant["storageQuotaBytes"], Value::Null);
    assert_eq!(tenant["status"], "active");

    assert_eq!(resolves(&api, "acme").await, 200, "reachable at acme.<domain>");
    let (status, current) = json(
        api.client
            .get(format!("{}/api/v1/tenant", api.base))
            .header("host", "acme.example.test")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((status, current["name"].as_str()), (200, Some("Acme Industrie")));
    assert_eq!(current["timeZone"], "America/New_York");
}

#[tokio::test]
async fn new_tenants_get_the_specified_defaults() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    let (status, tenant) = create(
        &api,
        &db,
        json!({"slug": "acme", "name": "Acme", "defaultLocale": "fr", "timeZone": "Europe/Paris"}),
    )
    .await;
    assert_eq!(status, 201, "{tenant}");
    assert_eq!(
        tenant["defaultCapacityMinutes"],
        json!([420, 420, 420, 420, 420, 0, 0])
    );
    assert_eq!(tenant["overloadTolerancePercent"], 0);
    assert_eq!(tenant["overloadMode"], "blocking");
    assert_eq!(tenant["apiRateLimitPerMinute"], Value::Null);
}

#[tokio::test]
async fn invalid_settings_are_reported_field_by_field() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    let (status, body) = create(
        &api,
        &db,
        json!({
            "slug": "Acme Corp", "name": "", "defaultLocale": "de", "timeZone": "Mars/Olympus_Mons",
            "defaultCapacityMinutes": [420, 420, 1500, 420, 420, 0, 0],
            "overloadTolerancePercent": 150, "overloadMode": "lenient",
            "storageQuotaBytes": 0, "apiRateLimitPerMinute": -5
        }),
    )
    .await;
    assert_problem(status, &body, 400, "validation_failed");
    let mut errors: Vec<String> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| format!("{}:{}", e["field"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    errors.sort();
    assert_eq!(
        errors,
        [
            "apiRateLimitPerMinute:out_of_range",
            "defaultCapacityMinutes.2:out_of_range",
            "defaultLocale:unsupported_locale",
            "name:required",
            "overloadMode:format",
            "overloadTolerancePercent:out_of_range",
            "slug:format",
            "storageQuotaBytes:out_of_range",
            "timeZone:unknown_time_zone",
        ]
    );

    let (status, body) = create(
        &api,
        &db,
        json!({"slug": "beta", "name": "B", "defaultLocale": "fr", "timeZone": "UTC"}),
    )
    .await;
    assert_problem(status, &body, 409, "tenant.slug_taken");
    assert_eq!(body["errors"][0], json!({"field": "slug", "code": "taken"}));

    let (status, body) = create(
        &api,
        &db,
        json!({"slug": "acme", "name": "A", "defaultLocale": "fr", "timeZone": "UTC", "colour": "red"}),
    )
    .await;
    assert_problem(status, &body, 400, "invalid_json");
}

#[tokio::test]
async fn creation_is_idempotent_with_a_key() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    let post = || {
        api.request(Method::POST, &db.tenant_a, PATH)
            .header("idempotency-key", "create-acme")
            .json(&acme())
    };
    let first = post().send().await.unwrap();
    assert_eq!(first.status(), 201);
    let (_, created) = json(first).await;
    let replay = post().send().await.unwrap();
    assert_eq!(replay.status(), 201);
    assert_eq!(replay.headers()["idempotent-replayed"], "true");
    let (_, replayed) = json(replay).await;
    assert_eq!(replayed["id"], created["id"]);
}

#[tokio::test]
async fn single_tenant_mode_only_creates_its_default_tenant() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(
        &db,
        Tenancy::Single {
            default_slug: "alpha".to_owned(),
        },
    )
    .await;
    let (status, body) = create(&api, &db, acme()).await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(
        body["errors"][0],
        json!({"field": "slug", "code": "not_default_slug"})
    );
}

#[tokio::test]
async fn settings_and_quotas_are_updated_with_if_match() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    let (_, tenant) = create(&api, &db, acme()).await;
    let item = format!("{PATH}/{}", tenant["id"].as_str().unwrap());
    let settings = json!({
        "name": "Acme Industrie SA", "defaultLocale": "fr", "timeZone": "Europe/Paris",
        "defaultCapacityMinutes": [420, 420, 420, 420, 210, 0, 0],
        "overloadTolerancePercent": 5, "overloadMode": "blocking",
        "storageQuotaBytes": 10_737_418_240_i64, "apiRateLimitPerMinute": 600
    });

    let missing = api
        .request(Method::PUT, &db.tenant_a, &item)
        .json(&settings)
        .send()
        .await
        .unwrap();
    let (status, body) = json(missing).await;
    assert_problem(status, &body, 428, "precondition_required");

    let res = api
        .request(Method::PUT, &db.tenant_a, &item)
        .header("if-match", "\"1\"")
        .json(&settings)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["etag"], "\"2\"");
    let (_, updated) = json(res).await;
    assert_eq!(updated["name"], "Acme Industrie SA");
    assert_eq!(updated["storageQuotaBytes"], 10_737_418_240_i64);
    assert_eq!(updated["apiRateLimitPerMinute"], 600);
    assert_eq!(updated["slug"], "acme", "the slug never changes");

    // Exposed to the services that enforce them.
    let mut conn = db.app_pool.acquire().await.unwrap();
    let quotas = infra::db::resolve_tenant(&mut conn, "acme")
        .await
        .unwrap()
        .unwrap()
        .quotas;
    assert_eq!(quotas.storage_bytes, Some(10_737_418_240));
    assert_eq!(quotas.api_requests_per_minute, Some(600));

    let stale = api
        .request(Method::PUT, &db.tenant_a, &item)
        .header("if-match", "\"1\"")
        .json(&settings)
        .send()
        .await
        .unwrap();
    let (status, body) = json(stale).await;
    assert_problem(status, &body, 412, "precondition_failed");

    let (status, read) = send(&api, &db.tenant_a, Method::GET, &item, None).await;
    assert_eq!((status, read["version"].as_i64()), (200, Some(2)));
    let (status, page) = send(&api, &db.tenant_a, Method::GET, &format!("{PATH}?limit=2"), None).await;
    assert_eq!(status, 200);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert!(page["nextCursor"].is_string());
}

#[tokio::test]
async fn deleting_a_tenant_needs_its_slug_and_purges_it_in_the_background() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    let beta = format!("{PATH}/{}", db.tenant_b.id);
    let delete = |confirm: &str, if_match: &str| {
        api.request(Method::DELETE, &db.tenant_a, &format!("{beta}?confirm={confirm}"))
            .header("if-match", if_match.to_owned())
    };

    let (status, body) = json(delete("bta", "\"1\"").send().await.unwrap()).await;
    assert_problem(status, &body, 400, "tenant.confirmation_mismatch");
    let current = api
        .request(
            Method::DELETE,
            &db.tenant_a,
            &format!("{PATH}/{}?confirm=alpha", db.tenant_a.id),
        )
        .header("if-match", "*");
    let (status, body) = json(current.send().await.unwrap()).await;
    assert_problem(status, &body, 409, "tenant.delete_current");
    assert_eq!(resolves(&api, "beta").await, 200);

    let (status, body) = json(delete("beta", "\"1\"").send().await.unwrap()).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "deleting");
    assert_eq!(
        resolves(&api, "beta").await,
        404,
        "the subdomain no longer resolves"
    );
    // The members of beta are out at once, sessions included.
    let beta_session = api.request(Method::GET, &db.tenant_b, "/api/v1/auth/session");
    assert_eq!(beta_session.send().await.unwrap().status(), 404);
    let (status, _) = json(delete("beta", "*").send().await.unwrap()).await;
    assert_eq!(status, 404, "already being deleted");

    // The worker purges it.
    let mailer = infra::email::Mailer::new(
        &infra::config::Secret::new(testkit::smtp_url()),
        "Planner <noreply@example.org>",
    )
    .unwrap();
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    let jobs = worker::jobs::Jobs {
        mailer,
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
    };
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
    let (status, _) = send(&api, &db.tenant_a, Method::GET, &beta, None).await;
    assert_eq!(status, 404, "nothing left of it");
    let left: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM tenant WHERE id = $1")
        .bind(db.tenant_b.id)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn only_platform_super_admins_reach_tenant_management() {
    let db = TestDb::new().await;
    let api = start_as_super_admin(&db, multi()).await;
    // The administrator of beta is a tenant admin, not a platform super admin.
    let alpha = format!("{PATH}/{}", db.tenant_a.id);
    for (method, path, body) in [
        (Method::GET, PATH.to_owned(), None),
        (Method::POST, PATH.to_owned(), Some(acme())),
        (Method::GET, alpha.clone(), None),
        (
            Method::PUT,
            alpha.clone(),
            Some(json!({"name": "x", "defaultLocale": "fr", "timeZone": "UTC"})),
        ),
        (Method::DELETE, format!("{alpha}?confirm=alpha"), None),
    ] {
        let mut request = api
            .request(method.clone(), &db.tenant_b, &path)
            .header("if-match", "*");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let (status, problem) = json(request.send().await.unwrap()).await;
        assert_problem(status, &problem, 403, "forbidden");
        let anonymous = api.anonymous(method, &db.tenant_b, &path).send().await.unwrap();
        assert_eq!(anonymous.status(), 401, "{path}");
    }
    let tenants: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM tenant")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(tenants, 2, "nothing was created");

    // A super admin still needs the CSRF token.
    let session = api.session(&db.tenant_a);
    let res = api
        .anonymous(Method::POST, &db.tenant_a, PATH)
        .header("cookie", format!("mlbpp_session={}", session.token))
        .json(&acme())
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 403, "csrf.invalid");
}
