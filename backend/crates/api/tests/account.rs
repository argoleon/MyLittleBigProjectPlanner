//! Integration tests of the signed-in member's own account (L02-US08) on
//! real PostgreSQL and Valkey: preferences, password change and sessions.
//! (Two-step verification, refused when the policy requires it, is tested
//! in `mfa.rs`.)

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::sessions::{SessionLifetime, SessionStore};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb, TestMember};

const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 \
                      (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
const NEW_PASSWORD: &str = "a much longer passphrase";

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

/// A session of `account_id` in `tenant`, as a sign-in opens it.
async fn open(api: &TestApi, account_id: Uuid, tenant: &SeedTenant) -> Browser {
    let opened = store(api).create(account_id, tenant.id).await.unwrap();
    Browser {
        token: opened.token,
        csrf: opened.session.csrf_token,
    }
}

/// A request of `browser` on `tenant`, from a Firefox on Linux.
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
        .header("user-agent", FIREFOX)
}

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

/// Makes the account of `email` a member of `tenant` too.
async fn join(db: &TestDb, tenant: &SeedTenant, email: &str) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email,
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
}

/// An internal member of alpha and beta.
async fn member_of_both(db: &TestDb) -> TestMember {
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(db, &db.tenant_b, &member.email).await;
    member
}

async fn signs_in(api: &TestApi, tenant: &SeedTenant, email: &str, password: &str) -> u16 {
    api.anonymous(Method::POST, tenant, "/api/v1/auth/sign-in")
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn preferences_are_checked_and_apply_in_every_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = member_of_both(&db).await;
    let alpha = open(&api, member.account_id, &db.tenant_a).await;
    let beta = open(&api, member.account_id, &db.tenant_b).await;

    let (status, me) = send(call(&api, &db.tenant_a, &alpha, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 200);
    assert_eq!(
        (me["locale"].clone(), me["timeZone"].clone()),
        (Value::Null, Value::Null),
        "tenant defaults"
    );

    let (status, saved) = send(
        call(&api, &db.tenant_a, &alpha, Method::PUT, "/api/v1/me/preferences")
            .json(&json!({"locale": "en", "timeZone": "America/New_York"})),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved, json!({"locale": "en", "timeZone": "America/New_York"}));
    for (tenant, browser) in [(&db.tenant_a, &alpha), (&db.tenant_b, &beta)] {
        let (_, me) = send(call(&api, tenant, browser, Method::GET, "/api/v1/me")).await;
        assert_eq!(
            (me["locale"].as_str(), me["timeZone"].as_str()),
            (Some("en"), Some("America/New_York"))
        );
    }

    // Unsupported language, a time zone PostgreSQL does not know: both said.
    let (status, body) = send(
        call(&api, &db.tenant_a, &alpha, Method::PUT, "/api/v1/me/preferences")
            .json(&json!({"locale": "de", "timeZone": "Mars/Olympus_Mons"})),
    )
    .await;
    assert_problem(status, &body, 400, "validation_failed");
    let errors: Vec<(&str, &str)> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["field"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        errors,
        [
            ("locale", "unsupported_locale"),
            ("timeZone", "unknown_time_zone")
        ]
    );
    let (status, _) = send(
        call(&api, &db.tenant_a, &alpha, Method::PUT, "/api/v1/me/preferences")
            .json(&json!({"theme": "dark"})),
    )
    .await;
    assert_eq!(status, 400, "unknown field");

    // Back to the tenant's defaults.
    let (status, _) =
        send(call(&api, &db.tenant_b, &beta, Method::PUT, "/api/v1/me/preferences").json(&json!({}))).await;
    assert_eq!(status, 200);
    let (_, me) = send(call(&api, &db.tenant_a, &alpha, Method::GET, "/api/v1/me")).await;
    assert_eq!(
        (me["locale"].clone(), me["timeZone"].clone()),
        (Value::Null, Value::Null)
    );

    // A change needs the CSRF token.
    let (status, body) = send(
        api.anonymous(Method::PUT, &db.tenant_a, "/api/v1/me/preferences")
            .header("cookie", format!("mlbpp_session={}", alpha.token))
            .json(&json!({"locale": "fr"})),
    )
    .await;
    assert_problem(status, &body, 403, "csrf.invalid");
}

#[tokio::test]
async fn changing_the_password_ends_the_other_sessions_in_every_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = member_of_both(&db).await;
    let this = open(&api, member.account_id, &db.tenant_a).await;
    let other = open(&api, member.account_id, &db.tenant_a).await;
    let beta = open(&api, member.account_id, &db.tenant_b).await;
    let change = |current: &str, new: &str| {
        call(&api, &db.tenant_a, &this, Method::POST, "/api/v1/me/password")
            .json(&json!({"currentPassword": current, "newPassword": new}))
    };

    let (status, body) = send(change("not my password", NEW_PASSWORD)).await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(
        body["errors"],
        json!([{"field": "currentPassword", "code": "incorrect"}])
    );
    let (status, body) = send(change(TEST_PASSWORD, "short")).await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(
        body["errors"],
        json!([{"field": "newPassword", "code": "too_short"}])
    );
    let (status, _) = send(call(&api, &db.tenant_a, &other, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 200, "nothing changed yet");

    let (status, body) = send(change(TEST_PASSWORD, NEW_PASSWORD)).await;
    assert_eq!(status, 204, "{body}");
    let (status, _) = send(call(&api, &db.tenant_a, &this, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 200, "this session stays open");
    for (tenant, browser) in [(&db.tenant_a, &other), (&db.tenant_b, &beta)] {
        let (status, body) = send(call(&api, tenant, browser, Method::GET, "/api/v1/me")).await;
        assert_problem(status, &body, 401, "unauthenticated");
    }
    assert_eq!(
        signs_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await,
        401
    );
    assert_eq!(
        signs_in(&api, &db.tenant_a, &member.email, NEW_PASSWORD).await,
        200
    );
}

#[tokio::test]
async fn incorrect_current_passwords_are_limited() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let browser = open(&api, member.account_id, &db.tenant_a).await;
    let change = |current: &str| {
        call(&api, &db.tenant_a, &browser, Method::POST, "/api/v1/me/password")
            .json(&json!({"currentPassword": current, "newPassword": NEW_PASSWORD}))
    };
    for _ in 0..domain::account::MAX_FAILED_SIGN_INS_PER_EMAIL {
        let (status, _) = send(change("guess")).await;
        assert_eq!(status, 400);
    }
    // Even the right one now waits.
    let res = change(TEST_PASSWORD).send().await.unwrap();
    assert!(res.headers().contains_key("retry-after"));
    let (status, body) = json(res).await;
    assert_problem(status, &body, 429, "too_many_requests");
}

#[tokio::test]
async fn an_account_without_password_cannot_change_it() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    // Signs in through the organization's identity provider only.
    let email = format!("sso-{}@alpha.test", Uuid::now_v7().simple());
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    let added = infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &email,
            display_name: "SSO Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let browser = open(&api, added.account_id, &db.tenant_a).await;
    let (status, body) = send(
        call(&api, &db.tenant_a, &browser, Method::POST, "/api/v1/me/password")
            .json(&json!({"currentPassword": "", "newPassword": NEW_PASSWORD})),
    )
    .await;
    assert_problem(status, &body, 409, "account.no_password");
}

#[tokio::test]
async fn sessions_are_listed_with_their_device_and_revoked_one_by_one() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = member_of_both(&db).await;
    let this = open(&api, member.account_id, &db.tenant_a).await;
    let phone = open(&api, member.account_id, &db.tenant_b).await;
    // The phone's request records its device.
    let res = api
        .anonymous(Method::GET, &db.tenant_b, "/api/v1/me")
        .header("cookie", format!("mlbpp_session={}", phone.token))
        .header("user-agent", IPHONE)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let (status, sessions) = send(call(
        &api,
        &db.tenant_a,
        &this,
        Method::GET,
        "/api/v1/me/sessions",
    ))
    .await;
    assert_eq!(status, 200, "{sessions}");
    let sessions = sessions.as_array().unwrap().clone();
    assert_eq!(sessions.len(), 2, "only this account's sessions");
    let (current, other) = (&sessions[0], &sessions[1]);
    assert_eq!(current["current"], true);
    assert_eq!(current["tenantName"], db.tenant_a.name.as_str());
    assert_eq!(
        (current["browser"].as_str(), current["os"].as_str()),
        (Some("Firefox"), Some("Linux"))
    );
    assert_eq!(other["current"], false);
    assert_eq!(other["tenantId"], db.tenant_b.id.to_string());
    assert_eq!(
        (other["browser"].as_str(), other["os"].as_str()),
        (Some("Safari"), Some("iOS"))
    );
    for field in ["createdAt", "lastSeenAt", "expiresAt"] {
        assert!(current[field].is_string(), "{field}");
    }
    let id = |session: &Value| session["id"].as_str().unwrap().to_owned();

    // Not this one: signing out does that.
    let (status, body) = send(call(
        &api,
        &db.tenant_a,
        &this,
        Method::DELETE,
        &format!("/api/v1/me/sessions/{}", id(current)),
    ))
    .await;
    assert_problem(status, &body, 409, "session.current");

    // Another account's session is not found, and stays open.
    let admin = api.session(&db.tenant_a).clone();
    let (_, admin_sessions) = send(api.request(Method::GET, &db.tenant_a, "/api/v1/me/sessions")).await;
    let admin_id = id(&admin_sessions[0]);
    let (status, _) = send(call(
        &api,
        &db.tenant_a,
        &this,
        Method::DELETE,
        &format!("/api/v1/me/sessions/{admin_id}"),
    ))
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(api.request(Method::GET, &db.tenant_a, "/api/v1/me")).await;
    assert_eq!(status, 200, "{} still signed in", admin.member.email);

    // The phone's session is refused from its next request on.
    let (status, _) = send(call(
        &api,
        &db.tenant_a,
        &this,
        Method::DELETE,
        &format!("/api/v1/me/sessions/{}", id(other)),
    ))
    .await;
    assert_eq!(status, 204);
    let (status, body) = send(call(&api, &db.tenant_b, &phone, Method::GET, "/api/v1/me")).await;
    assert_problem(status, &body, 401, "unauthenticated");
    let (_, sessions) = send(call(
        &api,
        &db.tenant_a,
        &this,
        Method::GET,
        "/api/v1/me/sessions",
    ))
    .await;
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    let (status, _) = send(call(
        &api,
        &db.tenant_a,
        &this,
        Method::DELETE,
        &format!("/api/v1/me/sessions/{}", id(other)),
    ))
    .await;
    assert_eq!(status, 404, "already ended");
}
