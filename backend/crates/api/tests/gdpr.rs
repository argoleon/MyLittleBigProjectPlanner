//! Integration tests of the GDPR rights (L02-US09) on real PostgreSQL and
//! Valkey: export of personal data, erasure by the account itself (with
//! re-authentication), by a tenant admin and by a platform super admin.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::sessions::{Proof, SessionLifetime, SessionStage, SessionStore};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb, TestMember};

/// A browser session: its cookie and CSRF token.
#[derive(Debug, Clone)]
struct Browser {
    token: String,
    csrf: String,
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

fn store(api: &TestApi) -> SessionStore {
    let settings = common::session_settings();
    SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
}

/// A session of `account_id` in `tenant`, opened by a sign-in that proved
/// `proof`.
async fn open(api: &TestApi, account_id: Uuid, tenant: &SeedTenant, proof: Proof) -> Browser {
    let opened = store(api)
        .create_proven(account_id, tenant.id, SessionStage::Complete, proof)
        .await
        .unwrap();
    Browser {
        token: opened.token,
        csrf: opened.session.csrf_token,
    }
}

fn call(
    api: &TestApi,
    tenant: &SeedTenant,
    browser: &Browser,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", browser.token))
        .header("x-csrf-token", &browser.csrf)
}

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

/// Makes the account of `email` a member of `tenant` too.
async fn join(db: &TestDb, tenant: &SeedTenant, email: &str, password_hash: Option<&str>) -> Uuid {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let added = infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email,
            display_name: "Test Member",
            password_hash,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    added.account_id
}

/// A member of alpha and beta, signed in to both.
async fn member_of_both(api: &TestApi, db: &TestDb) -> (TestMember, Browser, Browser) {
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(db, &db.tenant_b, &member.email, None).await;
    let alpha = open(api, member.account_id, &db.tenant_a, Proof::default()).await;
    let beta = open(api, member.account_id, &db.tenant_b, Proof::default()).await;
    (member, alpha, beta)
}

/// Runs the queued jobs (exports, anonymizations) as the worker does.
async fn run_jobs(api: &TestApi, db: &TestDb) {
    let mailer = infra::email::Mailer::new(
        &infra::config::Secret::new(testkit::smtp_url()),
        "Planner <noreply@example.org>",
    )
    .unwrap();
    let jobs = worker::jobs::Jobs {
        mailer,
        exports: infra::exports::ExportStore::new(api.valkey.clone(), api.key_prefix.clone()),
    };
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
}

async fn signs_in(api: &TestApi, tenant: &SeedTenant, email: &str) -> u16 {
    api.anonymous(Method::POST, tenant, "/api/v1/auth/sign-in")
        .json(&json!({"email": email, "password": TEST_PASSWORD}))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn the_member_exports_their_data_through_a_short_lived_download() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, alpha, _) = member_of_both(&api, &db).await;

    let (status, export) = send(call(
        &api,
        &db.tenant_a,
        &alpha,
        Method::POST,
        "/api/v1/me/exports",
    ))
    .await;
    assert_eq!(status, 202, "{export}");
    assert_eq!(export["status"], "pending");
    let id = export["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/me/exports/{id}");
    let (status, body) = send(call(
        &api,
        &db.tenant_a,
        &alpha,
        Method::GET,
        &format!("{path}/file"),
    ))
    .await;
    assert_problem(status, &body, 409, "export.pending");

    run_jobs(&api, &db).await;
    let (status, ready) = send(call(&api, &db.tenant_a, &alpha, Method::GET, &path)).await;
    assert_eq!((status, ready["status"].as_str()), (200, Some("ready")));
    let res = call(&api, &db.tenant_a, &alpha, Method::GET, &format!("{path}/file"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "application/json");
    assert!(
        res.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"personal-data-")
    );
    assert_eq!(res.headers()["cache-control"], "no-store");
    let (_, data) = json(res).await;
    assert_eq!(data["account"]["email"], member.email.as_str());
    let tenants: Vec<&str> = data["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["tenant"]["slug"].as_str().unwrap())
        .collect();
    assert_eq!(tenants.len(), 2, "{tenants:?}");

    // Another account knows nothing of it.
    let (status, _) = send(api.request(Method::GET, &db.tenant_a, &path)).await;
    assert_eq!(status, 404);
    let (status, _) = send(api.request(Method::GET, &db.tenant_a, &format!("{path}/file"))).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn the_member_erases_their_account_with_their_password_and_email() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, alpha, beta) = member_of_both(&api, &db).await;
    let (status, me) = send(call(&api, &db.tenant_a, &alpha, Method::GET, "/api/v1/me")).await;
    assert_eq!((status, me["hasPassword"].as_bool()), (200, Some(true)));
    let erase =
        |body: Value| call(&api, &db.tenant_a, &alpha, Method::POST, "/api/v1/me/erasure").json(&body);
    let codes = |body: &Value| -> Vec<(String, String)> {
        body["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["field"].as_str().unwrap().to_owned(),
                    e["code"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };

    let (status, body) = send(erase(json!({"confirmation": "someone@else.test"}))).await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(
        codes(&body),
        [
            ("confirmation".to_owned(), "mismatch".to_owned()),
            ("password".to_owned(), "required".to_owned())
        ]
    );
    let (status, body) = send(erase(
        json!({"confirmation": member.email, "password": "not mine"}),
    ))
    .await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(codes(&body), [("password".to_owned(), "incorrect".to_owned())]);
    let (status, _) = send(call(&api, &db.tenant_a, &alpha, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 200, "nothing erased yet");

    let res = erase(json!({"confirmation": member.email.to_uppercase(), "password": TEST_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let cleared: Vec<&str> = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert!(
        cleared
            .iter()
            .any(|c| c.starts_with("mlbpp_session=;") && c.contains("Max-Age=0")),
        "{cleared:?}"
    );

    for (tenant, browser) in [(&db.tenant_a, &alpha), (&db.tenant_b, &beta)] {
        let (status, body) = send(call(&api, tenant, browser, Method::GET, "/api/v1/me")).await;
        assert_problem(status, &body, 401, "unauthenticated");
    }
    assert_eq!(signs_in(&api, &db.tenant_a, &member.email).await, 401);
    // The admins no longer list them.
    let (_, members) = send(api.request(Method::GET, &db.tenant_a, "/api/v1/tenant/members?limit=200")).await;
    assert!(!members.to_string().contains(&member.email));
}

#[tokio::test]
async fn an_account_without_password_needs_a_recent_sign_in_through_its_organization() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let email = format!("sso-{}@alpha.test", Uuid::now_v7().simple());
    let account = join(&db, &db.tenant_a, &email, None).await;
    let body = json!({"confirmation": email});

    let not_sso = open(&api, account, &db.tenant_a, Proof::default()).await;
    let (_, me) = send(call(&api, &db.tenant_a, &not_sso, Method::GET, "/api/v1/me")).await;
    assert_eq!(me["hasPassword"], false);
    let (status, problem) =
        send(call(&api, &db.tenant_a, &not_sso, Method::POST, "/api/v1/me/erasure").json(&body)).await;
    assert_problem(status, &problem, 403, "account.reauthentication_required");

    // Just signed in through the identity provider: enough.
    let sso = open(
        &api,
        account,
        &db.tenant_a,
        Proof {
            second_factor: false,
            sso: true,
        },
    )
    .await;
    let (status, problem) =
        send(call(&api, &db.tenant_a, &sso, Method::POST, "/api/v1/me/erasure").json(&body)).await;
    assert_eq!(status, 204, "{problem}");
}

#[tokio::test]
async fn the_last_admin_of_a_tenant_cannot_erase_their_account() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    // The seeded admin of beta is the only one there.
    let admin = api.session(&db.tenant_b).clone();
    let browser = Browser {
        token: admin.token.clone(),
        csrf: admin.csrf_token.clone(),
    };
    let (status, body) = send(
        call(&api, &db.tenant_b, &browser, Method::POST, "/api/v1/me/erasure")
            .json(&json!({"confirmation": admin.member.email, "password": TEST_PASSWORD})),
    )
    .await;
    assert_problem(status, &body, 409, "member.last_admin");
}

#[tokio::test]
async fn a_tenant_admin_erases_a_member_from_their_tenant_only() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, alpha, beta) = member_of_both(&api, &db).await;
    let path = format!("/api/v1/tenant/members/{}/erasure", member.account_id);

    // Members do not erase each other.
    let (status, body) = send(
        call(&api, &db.tenant_a, &alpha, Method::POST, &path)
            .header("if-match", "*")
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &body, 403, "forbidden");
    let (status, _) = send(api.request(Method::POST, &db.tenant_a, &path).json(&json!({}))).await;
    assert_eq!(status, 428, "If-Match needed");

    let (status, body) = send(
        api.request(Method::POST, &db.tenant_a, &path)
            .header("if-match", "*")
            .json(&json!({"purgeComments": true})),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    let (status, _) = send(call(&api, &db.tenant_a, &alpha, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 401, "signed out of alpha");
    let (status, me) = send(call(&api, &db.tenant_b, &beta, Method::GET, "/api/v1/me")).await;
    assert_eq!(
        (status, me["email"].as_str()),
        (200, Some(member.email.as_str())),
        "beta and the account untouched"
    );
    let (status, _) = send(api.request(
        Method::GET,
        &db.tenant_a,
        &format!("/api/v1/tenant/members/{}", member.account_id),
    ))
    .await;
    assert_eq!(status, 404);
    let erased: (i64, bool) = infra::sqlx::query_as(
        "SELECT pseudonym_number, comments_purged FROM erased_member WHERE tenant_id = $1 AND account_id = $2",
    )
    .bind(db.tenant_a.id)
    .bind(member.account_id)
    .fetch_one(&db.owner_pool)
    .await
    .unwrap();
    assert!(erased.0 > 0 && erased.1, "{erased:?}");
    run_jobs(&api, &db).await;

    // The only admin of beta stays.
    let admin_b = api.session(&db.tenant_b).member.account_id;
    let (status, body) = send(
        api.request(
            Method::POST,
            &db.tenant_b,
            &format!("/api/v1/tenant/members/{admin_b}/erasure"),
        )
        .header("if-match", "*")
        .json(&json!({})),
    )
    .await;
    assert_problem(status, &body, 409, "member.last_admin");
}

#[tokio::test]
async fn a_platform_super_admin_erases_a_global_account() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, alpha, beta) = member_of_both(&api, &db).await;
    let path = |account: Uuid| format!("/api/v1/platform/accounts/{account}/erasure");

    let (status, body) = send(
        api.request(Method::POST, &db.tenant_a, &path(member.account_id))
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &body, 403, "forbidden");

    let admin = api.session(&db.tenant_a).member.account_id;
    db.make_platform_admin(admin).await;
    let (status, body) = send(
        api.request(Method::POST, &db.tenant_a, &path(member.account_id))
            .json(&json!({})),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    for (tenant, browser) in [(&db.tenant_a, &alpha), (&db.tenant_b, &beta)] {
        let (status, _) = send(call(&api, tenant, browser, Method::GET, "/api/v1/me")).await;
        assert_eq!(status, 401);
    }
    let (status, _) = send(
        api.request(Method::POST, &db.tenant_a, &path(member.account_id))
            .json(&json!({})),
    )
    .await;
    assert_eq!(status, 404, "erased already");
    let (status, body) = send(
        api.request(Method::POST, &db.tenant_a, &path(admin))
            .json(&json!({})),
    )
    .await;
    assert_problem(status, &body, 409, "account.platform_admin");
    run_jobs(&api, &db).await;
}
