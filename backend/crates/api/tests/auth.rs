//! Integration tests of local authentication (L02-US02), on real
//! PostgreSQL, Valkey and Mailpit: sign-in, sessions, CSRF, rate limits and
//! password reset.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::{AccountType, TenantRole};
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb};

const SIGN_IN: &str = "/api/v1/auth/sign-in";
const SIGN_OUT: &str = "/api/v1/auth/sign-out";
const SESSION: &str = "/api/v1/auth/session";
const RESET: &str = "/api/v1/auth/password-reset";
const CONFIRM: &str = "/api/v1/auth/password-reset/confirm";
const ITEMS: &str = "/api/v1/reference-items";

async fn sign_in(api: &TestApi, tenant: &SeedTenant, email: &str, password: &str) -> reqwest::Response {
    sign_in_from(api, tenant, email, password, "198.51.100.1").await
}

async fn sign_in_from(
    api: &TestApi,
    tenant: &SeedTenant,
    email: &str,
    password: &str,
    ip: &str,
) -> reqwest::Response {
    api.anonymous(Method::POST, tenant, SIGN_IN)
        .header("x-forwarded-for", ip)
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
}

/// The `Set-Cookie` header setting `name`.
fn set_cookie(res: &reqwest::Response, name: &str) -> String {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|v| v.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no {name} cookie"))
}

/// The value of a `Set-Cookie` header.
fn cookie_value(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned()
}

/// A session opened through the sign-in endpoint: (cookie token, CSRF token).
async fn signed_in(api: &TestApi, tenant: &SeedTenant, email: &str, password: &str) -> (String, String) {
    let res = sign_in(api, tenant, email, password).await;
    assert_eq!(res.status(), 200);
    let token = cookie_value(&set_cookie(&res, "mlbpp_session"));
    let (_, body) = json(res).await;
    (token, body["csrfToken"].as_str().unwrap().to_owned())
}

fn with_session(builder: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    builder.header("cookie", format!("mlbpp_session={token}"))
}

/// Runs `sql` in a transaction of `tenant`, as the application role.
async fn exec_in_tenant(db: &TestDb, tenant: &SeedTenant, sql: &str) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let done = infra::sqlx::raw_sql(infra::sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(done.rows_affected() > 0, "{sql} changed nothing");
    tx.commit().await.unwrap();
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

#[tokio::test]
async fn sign_in_opens_a_session_with_host_only_http_only_cookies() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::ConfigurationAdmin).await;

    // The email is matched whatever its case and spaces.
    let res = sign_in(
        &api,
        &db.tenant_a,
        &format!(" {} ", member.email.to_uppercase()),
        TEST_PASSWORD,
    )
    .await;
    assert_eq!(res.status(), 200);
    let session_cookie = set_cookie(&res, "mlbpp_session");
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/", "Max-Age=3600"] {
        assert!(session_cookie.contains(attribute), "{session_cookie}");
    }
    assert!(!session_cookie.contains("Domain"), "host-only: {session_cookie}");
    let csrf_cookie = set_cookie(&res, "mlbpp_csrf");
    assert!(
        !csrf_cookie.contains("HttpOnly"),
        "the front end reads it: {csrf_cookie}"
    );
    assert!(csrf_cookie.contains("Secure") && !csrf_cookie.contains("Domain"));

    let token = cookie_value(&session_cookie);
    let (_, body) = json(res).await;
    assert_eq!(body["account"]["id"], member.account_id.to_string());
    assert_eq!(body["account"]["email"], member.email.as_str());
    assert_eq!(body["account"]["displayName"], "Test Member");
    assert_eq!(body["account"]["tenantRole"], "configuration_admin");
    assert_eq!(body["csrfToken"], cookie_value(&csrf_cookie));
    assert!(body["expiresAt"].is_string());
    assert!(body.get("password").is_none() && body.get("passwordHash").is_none());

    let res = with_session(api.anonymous(Method::GET, &db.tenant_a, SESSION), &token)
        .send()
        .await
        .unwrap();
    let (status, current) = json(res).await;
    assert_eq!(status, 200);
    assert_eq!(current["account"], body["account"]);
    assert_eq!(current["csrfToken"], body["csrfToken"]);
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_email_get_the_same_answer() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let suspended = db.add_member(&db.tenant_a, TenantRole::Member).await;
    exec_in_tenant(
        &db,
        &db.tenant_a,
        &format!(
            "UPDATE membership SET status = 'suspended' WHERE account_id = '{}'",
            suspended.account_id
        ),
    )
    .await;
    let external = db
        .add_member_with(&db.tenant_a, TenantRole::Member, AccountType::External)
        .await;
    let elsewhere = db.add_member(&db.tenant_b, TenantRole::Member).await;

    let mut answers = Vec::new();
    for (email, password) in [
        (member.email.as_str(), "wrong password, long enough"),
        ("nobody@alpha.test", TEST_PASSWORD),
        ("not an email", TEST_PASSWORD),
        (suspended.email.as_str(), TEST_PASSWORD),
        (external.email.as_str(), TEST_PASSWORD),
        (elsewhere.email.as_str(), TEST_PASSWORD),
    ] {
        let res = sign_in(&api, &db.tenant_a, email, password).await;
        assert!(res.headers().get("set-cookie").is_none());
        let (status, mut body) = json(res).await;
        assert_problem(status, &body, 401, "auth.invalid_credentials");
        body.as_object_mut().unwrap().remove("instance");
        answers.push(body);
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]), "{answers:?}");

    let res = api
        .anonymous(Method::POST, &db.tenant_a, SIGN_IN)
        .json(&json!({"email": "", "password": ""}))
        .send()
        .await
        .unwrap();
    let (status, body) = json(res).await;
    assert_problem(status, &body, 400, "validation_failed");
}

#[tokio::test]
async fn five_failures_on_an_email_block_its_sign_in_for_a_while() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let other = db.add_member(&db.tenant_a, TenantRole::Member).await;

    for _ in 0..5 {
        let res = sign_in(&api, &db.tenant_a, &member.email, "wrong password, long enough").await;
        assert_eq!(res.status(), 401);
    }
    // Even the right password is refused now, from any address.
    let res = sign_in_from(&api, &db.tenant_a, &member.email, TEST_PASSWORD, "203.0.113.50").await;
    assert_eq!(res.status(), 429);
    let retry: u64 = res.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=900).contains(&retry), "{retry}");
    let (status, body) = json(res).await;
    assert_problem(status, &body, 429, "too_many_requests");

    // Unknown emails are limited the same way: no enumeration.
    for _ in 0..5 {
        sign_in(&api, &db.tenant_a, "nobody@alpha.test", TEST_PASSWORD).await;
    }
    let res = sign_in(&api, &db.tenant_a, "nobody@alpha.test", TEST_PASSWORD).await;
    assert_eq!(res.status(), 429);

    // Other emails are not affected.
    assert_eq!(
        sign_in(&api, &db.tenant_a, &other.email, TEST_PASSWORD)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn a_success_clears_the_failures_of_the_email() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    for _ in 0..4 {
        sign_in(&api, &db.tenant_a, &member.email, "wrong password, long enough").await;
    }
    assert_eq!(
        sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD)
            .await
            .status(),
        200
    );
    for _ in 0..4 {
        sign_in(&api, &db.tenant_a, &member.email, "wrong password, long enough").await;
    }
    assert_eq!(
        sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn many_failures_from_one_address_block_it() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    for i in 0..30 {
        let res = sign_in_from(
            &api,
            &db.tenant_a,
            &format!("guess{i}@alpha.test"),
            "x",
            "192.0.2.66",
        )
        .await;
        assert_eq!(res.status(), 401);
    }
    let res = sign_in_from(&api, &db.tenant_a, &member.email, TEST_PASSWORD, "192.0.2.66").await;
    assert_eq!(res.status(), 429);
    let res = sign_in_from(&api, &db.tenant_a, &member.email, TEST_PASSWORD, "192.0.2.67").await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn protected_endpoints_need_a_session_of_their_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    for path in [ITEMS, SESSION] {
        let (status, body) = json(
            api.anonymous(Method::GET, &db.tenant_a, path)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_problem(status, &body, 401, "unauthenticated");
    }
    let forged = with_session(api.anonymous(Method::GET, &db.tenant_a, ITEMS), "forged-token");
    assert_eq!(forged.send().await.unwrap().status(), 401);

    // The session of tenant A is refused on tenant B.
    let token_a = &api.session(&db.tenant_a).token;
    let res = with_session(api.anonymous(Method::GET, &db.tenant_b, ITEMS), token_a)
        .send()
        .await
        .unwrap();
    let (status, body) = json(res).await;
    assert_problem(status, &body, 401, "unauthenticated");

    // Public endpoints answer without a session.
    assert_eq!(
        api.anonymous(Method::GET, &db.tenant_a, "/api/v1/tenant")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        api.anonymous(Method::GET, &db.tenant_a, "/api/v1/ping")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // An unknown tenant is still a 404, session or not.
    let res = api
        .client
        .get(format!("{}{ITEMS}", api.base))
        .header("host", "gamma.example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn a_member_suspended_after_sign_in_loses_access() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (token, _) = signed_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
    exec_in_tenant(
        &db,
        &db.tenant_a,
        &format!(
            "UPDATE membership SET status = 'suspended' WHERE account_id = '{}'",
            member.account_id
        ),
    )
    .await;
    let res = with_session(api.anonymous(Method::GET, &db.tenant_a, SESSION), &token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn state_changing_requests_need_the_csrf_token_of_the_session() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let session = api.session(&db.tenant_a).clone();
    let item = json!({"code": "CSRF-1", "labelTranslations": {"fr": "a", "en": "a"}, "quantity": 1});

    let missing = with_session(api.anonymous(Method::POST, &db.tenant_a, ITEMS), &session.token).json(&item);
    let (status, body) = json(missing.send().await.unwrap()).await;
    assert_problem(status, &body, 403, "csrf.invalid");

    let other_csrf = &api.session(&db.tenant_b).csrf_token;
    let wrong = with_session(api.anonymous(Method::POST, &db.tenant_a, ITEMS), &session.token)
        .header("x-csrf-token", other_csrf)
        .json(&item);
    let (status, body) = json(wrong.send().await.unwrap()).await;
    assert_problem(status, &body, 403, "csrf.invalid");

    let count: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM reference_item")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was created");

    // Reads need no CSRF token; writes with it succeed.
    let read = with_session(api.anonymous(Method::GET, &db.tenant_a, ITEMS), &session.token);
    assert_eq!(read.send().await.unwrap().status(), 200);
    let res = api
        .request(Method::POST, &db.tenant_a, ITEMS)
        .json(&item)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn after_sign_out_the_cookie_no_longer_works() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (token, csrf) = signed_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
    let (other_token, _) = signed_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;

    let without_csrf = with_session(api.anonymous(Method::POST, &db.tenant_a, SIGN_OUT), &token);
    assert_eq!(without_csrf.send().await.unwrap().status(), 403);

    let res = with_session(api.anonymous(Method::POST, &db.tenant_a, SIGN_OUT), &token)
        .header("x-csrf-token", &csrf)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let cleared = set_cookie(&res, "mlbpp_session");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    assert!(set_cookie(&res, "mlbpp_csrf").contains("Max-Age=0"));

    let replayed = with_session(api.anonymous(Method::GET, &db.tenant_a, SESSION), &token);
    let (status, body) = json(replayed.send().await.unwrap()).await;
    assert_problem(status, &body, 401, "unauthenticated");
    let other = with_session(api.anonymous(Method::GET, &db.tenant_a, SESSION), &other_token);
    assert_eq!(
        other.send().await.unwrap().status(),
        200,
        "other sessions stay open"
    );
}

#[tokio::test]
async fn signing_in_again_replaces_the_session_of_the_cookie() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (first, _) = signed_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;

    let res = with_session(api.anonymous(Method::POST, &db.tenant_a, SIGN_IN), &first)
        .json(&json!({"email": member.email, "password": TEST_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let second = cookie_value(&set_cookie(&res, "mlbpp_session"));
    assert_ne!(second, first);
    let old = with_session(api.anonymous(Method::GET, &db.tenant_a, SESSION), &first);
    assert_eq!(old.send().await.unwrap().status(), 401);
}

// --- Password reset ----------------------------------------------------------

async fn request_reset(api: &TestApi, tenant: &SeedTenant, email: &str) -> u16 {
    api.anonymous(Method::POST, tenant, RESET)
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn confirm(api: &TestApi, tenant: &SeedTenant, token: &str, password: &str) -> (u16, Value) {
    json(
        api.anonymous(Method::POST, tenant, CONFIRM)
            .json(&json!({ "token": token, "password": password }))
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn queued_jobs(db: &TestDb) -> i64 {
    infra::sqlx::query_scalar("SELECT count(*) FROM job")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

/// Runs the queued jobs with the worker's handlers (emails to Mailpit).
async fn run_jobs(db: &TestDb) {
    let mailer = infra::email::Mailer::new(
        &infra::config::Secret::new(testkit::smtp_url()),
        "Planner <noreply@example.org>",
    )
    .unwrap();
    let runner = worker::jobs::JobRunner::new(db.app_pool.clone());
    let jobs = worker::jobs::Jobs { mailer };
    while runner.next_batch(&jobs).await == worker::Batch::Processed {}
}

/// The token of the reset link emailed to `email`.
async fn emailed_token(email: &str) -> (String, testkit::CaughtEmail) {
    let caught = testkit::caught_email(email).await;
    let link_start = caught
        .text
        .find("https://alpha.example.test/reset-password?token=")
        .expect("link");
    let token: String = caught.text[link_start..]
        .split_once("token=")
        .unwrap()
        .1
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (token, caught)
}

#[tokio::test]
async fn a_reset_link_sets_a_new_password_and_ends_every_session() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    // Signed in in both tenants: the reset ends both sessions.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_b.id)
        .await
        .unwrap();
    infra::accounts::add_member(
        &mut tx,
        &infra::accounts::NewMember {
            email: &member.email,
            display_name: "Test Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (session_a, _) = signed_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
    let (session_b, _) = signed_in(&api, &db.tenant_b, &member.email, TEST_PASSWORD).await;

    assert_eq!(
        request_reset(&api, &db.tenant_a, &member.email.to_uppercase()).await,
        202
    );
    run_jobs(&db).await;
    let (token, caught) = emailed_token(&member.email).await;
    assert!(caught.subject.contains("Alpha Corp"), "{}", caught.subject);
    assert!(caught.text.contains("60"), "the lifetime is stated");

    // A weak password is refused, and the link still works afterwards.
    let (status, body) = confirm(&api, &db.tenant_a, &token, "short").await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(body["errors"][0]["code"], "too_short");

    let (status, _) = confirm(&api, &db.tenant_a, &token, "a brand new passphrase").await;
    assert_eq!(status, 204);
    for (tenant, token) in [(&db.tenant_a, &session_a), (&db.tenant_b, &session_b)] {
        let res = with_session(api.anonymous(Method::GET, tenant, SESSION), token);
        assert_eq!(res.send().await.unwrap().status(), 401, "{}", tenant.slug);
    }
    assert_eq!(
        sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD)
            .await
            .status(),
        401
    );
    assert_eq!(
        sign_in(&api, &db.tenant_a, &member.email, "a brand new passphrase")
            .await
            .status(),
        200
    );

    // The link works once.
    let (status, body) = confirm(&api, &db.tenant_a, &token, "yet another passphrase").await;
    assert_problem(status, &body, 400, "auth.reset_token_invalid");
}

#[tokio::test]
async fn a_reset_request_answers_the_same_for_unknown_emails_and_sends_nothing() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let elsewhere = db.add_member(&db.tenant_b, TenantRole::Member).await;
    let external = db
        .add_member_with(&db.tenant_a, TenantRole::Member, AccountType::External)
        .await;

    for email in [
        "nobody@alpha.test",
        "not an email",
        elsewhere.email.as_str(),
        external.email.as_str(),
    ] {
        assert_eq!(request_reset(&api, &db.tenant_a, email).await, 202, "{email}");
    }
    assert_eq!(queued_jobs(&db).await, 0);
}

#[tokio::test]
async fn reset_emails_to_one_address_are_limited() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    for _ in 0..5 {
        assert_eq!(request_reset(&api, &db.tenant_a, &member.email).await, 202);
    }
    assert_eq!(queued_jobs(&db).await, 3);
}

#[tokio::test]
async fn expired_unknown_and_foreign_reset_tokens_are_refused() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;

    assert_eq!(request_reset(&api, &db.tenant_a, &member.email).await, 202);
    run_jobs(&db).await;
    let (token, _) = emailed_token(&member.email).await;

    let (status, body) = confirm(&api, &db.tenant_b, &token, "a brand new passphrase").await;
    assert_problem(status, &body, 400, "auth.reset_token_invalid");
    let (status, _) = confirm(&api, &db.tenant_a, "unknown-token", "a brand new passphrase").await;
    assert_eq!(status, 400);

    exec_in_tenant(
        &db,
        &db.tenant_a,
        "UPDATE password_reset SET expires_at = now() - interval '1 second'",
    )
    .await;
    let (status, body) = confirm(&api, &db.tenant_a, &token, "a brand new passphrase").await;
    assert_problem(status, &body, 400, "auth.reset_token_invalid");
    assert_eq!(
        sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn a_new_password_revokes_the_other_reset_links() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    assert_eq!(request_reset(&api, &db.tenant_a, &member.email).await, 202);
    run_jobs(&db).await;
    let (first, _) = emailed_token(&member.email).await;
    // Mailpit search returns the newest first: delete the first email.
    testkit::delete_caught_emails(&member.email).await;
    assert_eq!(request_reset(&api, &db.tenant_a, &member.email).await, 202);
    run_jobs(&db).await;
    let (second, _) = emailed_token(&member.email).await;
    assert_ne!(first, second);

    assert_eq!(
        confirm(&api, &db.tenant_a, &second, "a brand new passphrase")
            .await
            .0,
        204
    );
    let (status, body) = confirm(&api, &db.tenant_a, &first, "another new passphrase").await;
    assert_problem(status, &body, 400, "auth.reset_token_invalid");
}
