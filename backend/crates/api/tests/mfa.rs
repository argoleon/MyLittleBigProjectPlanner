//! Integration tests of two-step verification (L02-US03) on real
//! PostgreSQL and Valkey: enrollment, sign-in with a TOTP or backup code,
//! rate limiting, and enrollment forced by the tenant's policy.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::TenantRole;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb, TestMember};

const ITEMS: &str = "/api/v1/reference-items";

/// A browser session: its cookie and CSRF token.
#[derive(Debug, Clone)]
struct Browser {
    token: String,
    csrf: String,
}

fn set_cookie(res: &reqwest::Response, name: &str) -> Option<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap().split_once('=').unwrap().1.to_owned())
}

/// Signs in with the password: the session and its JSON.
async fn sign_in(api: &TestApi, tenant: &SeedTenant, member: &TestMember) -> (Browser, Value) {
    let res = api
        .anonymous(Method::POST, tenant, "/api/v1/auth/sign-in")
        .json(&json!({"email": member.email, "password": TEST_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let token = set_cookie(&res, "mlbpp_session").unwrap();
    let (_, body) = json(res).await;
    let csrf = body["csrfToken"].as_str().unwrap().to_owned();
    (Browser { token, csrf }, body)
}

/// A request of `browser` (cookie and CSRF token).
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

fn now() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap()
}

/// The code the authenticator app shows `offset_secs` from now.
fn code(secret: &str, offset_secs: i64) -> String {
    infra::mfa::code_for_base32(secret, now().saturating_add_signed(offset_secs)).unwrap()
}

/// Enables two-step verification for `browser`'s member: the base32 secret
/// and the backup codes.
async fn enable(api: &TestApi, tenant: &SeedTenant, browser: &Browser) -> (String, Vec<String>) {
    let (status, setup) = send(call(
        api,
        tenant,
        browser,
        Method::POST,
        "/api/v1/account/mfa/setup",
    ))
    .await;
    assert_eq!(status, 200, "{setup}");
    let secret = setup["secret"].as_str().unwrap().to_owned();
    let (status, enabled) = send(
        call(api, tenant, browser, Method::POST, "/api/v1/account/mfa/confirm")
            .json(&json!({"code": code(&secret, 0)})),
    )
    .await;
    assert_eq!(status, 200, "{enabled}");
    let codes = enabled["backupCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect();
    (secret, codes)
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

#[tokio::test]
async fn enabling_mfa_shows_a_qr_code_and_eight_backup_codes_once() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;

    let (status, state) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::GET,
        "/api/v1/account/mfa",
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        state,
        json!({"enabled": false, "backupCodesLeft": 0, "requiredByPolicy": false})
    );

    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/account/mfa/confirm",
        )
        .json(&json!({"code": "123456"})),
    )
    .await;
    assert_problem(status, &body, 409, "mfa.not_started");

    let (status, setup) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::POST,
        "/api/v1/account/mfa/setup",
    ))
    .await;
    assert_eq!(status, 200);
    let secret = setup["secret"].as_str().unwrap().to_owned();
    assert!(setup["otpauthUri"].as_str().unwrap().contains(&secret));
    assert!(
        setup["qrCodeSvg"].as_str().unwrap().starts_with("<?xml")
            || setup["qrCodeSvg"].as_str().unwrap().contains("<svg")
    );

    // A wrong code does not enable it.
    let wrong = if code(&secret, 0) == "000000" {
        "111111"
    } else {
        "000000"
    };
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/account/mfa/confirm",
        )
        .json(&json!({"code": wrong})),
    )
    .await;
    assert_problem(status, &body, 401, "auth.invalid_code");

    let (status, enabled) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/account/mfa/confirm",
        )
        .json(&json!({"code": code(&secret, 0)})),
    )
    .await;
    assert_eq!(status, 200, "{enabled}");
    let codes = enabled["backupCodes"].as_array().unwrap();
    assert_eq!(codes.len(), 8);
    assert!(codes.iter().all(|c| c.as_str().unwrap().len() == 11), "{codes:?}");
    assert_eq!(enabled["session"]["stage"], "complete");

    let (_, state) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::GET,
        "/api/v1/account/mfa",
    ))
    .await;
    assert_eq!(
        state,
        json!({"enabled": true, "backupCodesLeft": 8, "requiredByPolicy": false})
    );
    // Shown once: a new setup is refused while it is enabled.
    let (status, body) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::POST,
        "/api/v1/account/mfa/setup",
    ))
    .await;
    assert_problem(status, &body, 409, "mfa.already_enabled");
}

#[tokio::test]
async fn with_mfa_the_password_alone_opens_no_session() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;
    let (secret, backup_codes) = enable(&api, &db.tenant_a, &browser).await;

    let (pending, body) = sign_in(&api, &db.tenant_a, &member).await;
    assert_eq!(body["stage"], "mfa_challenge");
    let (status, _) = send(call(&api, &db.tenant_a, &pending, Method::GET, ITEMS)).await;
    assert_eq!(status, 401, "nothing is reachable before the second factor");
    let (status, current) = send(call(
        &api,
        &db.tenant_a,
        &pending,
        Method::GET,
        "/api/v1/auth/session",
    ))
    .await;
    assert_eq!((status, current["stage"].as_str()), (200, Some("mfa_challenge")));
    let (status, _) = send(call(
        &api,
        &db.tenant_a,
        &pending,
        Method::POST,
        "/api/v1/account/mfa/setup",
    ))
    .await;
    assert_eq!(status, 401);

    // A TOTP code (the next one: the current step was used to enable it).
    let totp = code(&secret, 30);
    let res = call(
        &api,
        &db.tenant_a,
        &pending,
        Method::POST,
        "/api/v1/auth/sign-in/verify",
    )
    .json(&json!({"code": totp}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let complete = Browser {
        token: set_cookie(&res, "mlbpp_session").unwrap(),
        csrf: set_cookie(&res, "mlbpp_csrf").unwrap(),
    };
    let (_, body) = json(res).await;
    assert_eq!(body["stage"], "complete");
    assert_ne!(complete.token, pending.token, "a new token once verified");
    let (status, _) = send(call(&api, &db.tenant_a, &complete, Method::GET, ITEMS)).await;
    assert_eq!(status, 200);
    let (status, _) = send(call(
        &api,
        &db.tenant_a,
        &pending,
        Method::GET,
        "/api/v1/auth/session",
    ))
    .await;
    assert_eq!(status, 401, "the pending token is gone");
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &complete,
            Method::POST,
            "/api/v1/auth/sign-in/verify",
        )
        .json(&json!({"code": "123456"})),
    )
    .await;
    assert_problem(status, &body, 409, "conflict");

    // The same TOTP code a second time is refused.
    let (again, _) = sign_in(&api, &db.tenant_a, &member).await;
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &again,
            Method::POST,
            "/api/v1/auth/sign-in/verify",
        )
        .json(&json!({"code": totp})),
    )
    .await;
    assert_problem(status, &body, 401, "auth.invalid_code");

    // A backup code works once, typed in any case and with spaces.
    let typed = backup_codes[0].to_uppercase().replace('-', " ");
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &again,
            Method::POST,
            "/api/v1/auth/sign-in/verify",
        )
        .json(&json!({"code": typed})),
    )
    .await;
    assert_eq!(
        (status, body["stage"].as_str()),
        (200, Some("complete")),
        "{body}"
    );
    let (third, _) = sign_in(&api, &db.tenant_a, &member).await;
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &third,
            Method::POST,
            "/api/v1/auth/sign-in/verify",
        )
        .json(&json!({"code": backup_codes[0]})),
    )
    .await;
    assert_problem(status, &body, 401, "auth.invalid_code");
}

#[tokio::test]
async fn repeated_wrong_codes_are_rate_limited() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;
    let (secret, _) = enable(&api, &db.tenant_a, &browser).await;
    let (pending, _) = sign_in(&api, &db.tenant_a, &member).await;
    let right = code(&secret, 30);
    let wrong = if right == "000000" { "111111" } else { "000000" };

    for _ in 0..5 {
        let (status, _) = send(
            call(
                &api,
                &db.tenant_a,
                &pending,
                Method::POST,
                "/api/v1/auth/sign-in/verify",
            )
            .json(&json!({"code": wrong})),
        )
        .await;
        assert_eq!(status, 401);
    }
    let res = call(
        &api,
        &db.tenant_a,
        &pending,
        Method::POST,
        "/api/v1/auth/sign-in/verify",
    )
    .json(&json!({"code": right}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 429, "even the right code, until the window ends");
    assert!(res.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn the_tenant_policy_forces_members_without_mfa_to_enroll() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;

    // Only a tenant admin sets the policy.
    let policy = json!({"localAccountsRequireMfa": true});
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::PUT,
            "/api/v1/tenant/auth-policy",
        )
        .header("if-match", "*")
        .json(&policy),
    )
    .await;
    assert_problem(status, &body, 403, "forbidden");
    let res = api
        .request(Method::GET, &db.tenant_a, "/api/v1/tenant/auth-policy")
        .send()
        .await
        .unwrap();
    let etag = res.headers()["etag"].to_str().unwrap().to_owned();
    let (_, current) = json(res).await;
    assert_eq!(current, json!({"localAccountsRequireMfa": false}));
    let (status, _) = send(
        api.request(Method::PUT, &db.tenant_a, "/api/v1/tenant/auth-policy")
            .json(&policy),
    )
    .await;
    assert_eq!(status, 428);
    let (status, updated) = send(
        api.request(Method::PUT, &db.tenant_a, "/api/v1/tenant/auth-policy")
            .header("if-match", etag)
            .json(&policy),
    )
    .await;
    assert_eq!((status, updated), (200, policy));

    // The member's next sign-in can only enroll.
    let (enrolling, body) = sign_in(&api, &db.tenant_a, &member).await;
    assert_eq!(body["stage"], "mfa_enrollment");
    let (status, _) = send(call(&api, &db.tenant_a, &enrolling, Method::GET, ITEMS)).await;
    assert_eq!(status, 401);
    let (status, state) = send(call(
        &api,
        &db.tenant_a,
        &enrolling,
        Method::GET,
        "/api/v1/account/mfa",
    ))
    .await;
    assert_eq!((status, state["requiredByPolicy"].as_bool()), (200, Some(true)));
    let (_, setup) = send(call(
        &api,
        &db.tenant_a,
        &enrolling,
        Method::POST,
        "/api/v1/account/mfa/setup",
    ))
    .await;
    let secret = setup["secret"].as_str().unwrap().to_owned();
    let res = call(
        &api,
        &db.tenant_a,
        &enrolling,
        Method::POST,
        "/api/v1/account/mfa/confirm",
    )
    .json(&json!({"code": code(&secret, 0)}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let complete = Browser {
        token: set_cookie(&res, "mlbpp_session").unwrap(),
        csrf: set_cookie(&res, "mlbpp_csrf").unwrap(),
    };
    let (_, enabled) = json(res).await;
    assert_eq!(enabled["session"]["stage"], "complete");
    assert_eq!(enabled["backupCodes"].as_array().unwrap().len(), 8);
    let (status, _) = send(call(&api, &db.tenant_a, &complete, Method::GET, ITEMS)).await;
    assert_eq!(status, 200, "enrolled: the application opens");

    // While the policy holds, it cannot be disabled.
    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &complete,
            Method::POST,
            "/api/v1/account/mfa/disable",
        )
        .json(&json!({"code": code(&secret, 30)})),
    )
    .await;
    assert_problem(status, &body, 409, "mfa.required_by_policy");
}

#[tokio::test]
async fn mfa_is_disabled_with_a_code_and_sign_in_is_direct_again() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;
    let (_, backup_codes) = enable(&api, &db.tenant_a, &browser).await;

    let (status, body) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/account/mfa/disable",
        )
        .json(&json!({"code": "zzzzz-zzzzz"})),
    )
    .await;
    assert_problem(status, &body, 401, "auth.invalid_code");
    let (status, _) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/account/mfa/disable",
        )
        .json(&json!({"code": backup_codes[1]})),
    )
    .await;
    assert_eq!(status, 204);
    let (_, body) = sign_in(&api, &db.tenant_a, &member).await;
    assert_eq!(body["stage"], "complete");
}
