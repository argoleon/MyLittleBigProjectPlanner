//! Integration tests of the account's tenants and of switching between them
//! (L02-US05): the memberships across tenants, the one-time switch link,
//! the target tenant's policy, external accounts and isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use domain::tenancy::Tenancy;
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::sessions::{Proof, SessionLifetime, SessionStage, SessionStore};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb, TestMember};

const MEMBERSHIPS: &str = "/api/v1/me/memberships";
const SWITCH: &str = "/api/v1/me/switch";
const SESSION: &str = "/api/v1/auth/session";

/// A browser session on one tenant's host: its cookie and CSRF token.
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

/// A request of `browser` on `tenant`'s host.
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

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

/// The session store the API reads, to open sessions as other sign-ins would.
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

/// A session of `account_id` in `tenant`, at `stage`, with what it proved.
async fn open(
    api: &TestApi,
    account_id: Uuid,
    tenant: &SeedTenant,
    stage: SessionStage,
    proof: Proof,
) -> Browser {
    let opened = store(api)
        .create_proven(account_id, tenant.id, stage, proof)
        .await
        .unwrap();
    Browser {
        token: opened.token,
        csrf: opened.session.csrf_token,
    }
}

/// Signs in with the password on `tenant`.
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

/// Makes the account of `email` a member of `tenant` too.
async fn join(db: &TestDb, tenant: &SeedTenant, email: &str, role: TenantRole, account_type: AccountType) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let added = infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email,
            display_name: "Test Member",
            password_hash: None,
            account_type,
            tenant_role: role,
            local_account: false,
        },
    )
    .await
    .unwrap();
    assert!(!added.new_account);
    tx.commit().await.unwrap();
}

/// An internal member of alpha (tenant admin) and beta (member), signed in
/// to alpha with the password.
async fn member_of_both(api: &TestApi, db: &TestDb) -> (TestMember, Browser) {
    let member = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    join(
        db,
        &db.tenant_b,
        &member.email,
        TenantRole::Member,
        AccountType::Internal,
    )
    .await;
    let (browser, _) = sign_in(api, &db.tenant_a, &member).await;
    (member, browser)
}

/// Starts a switch from `browser`'s session on `from` to `to`: the link.
async fn start_switch(api: &TestApi, from: &SeedTenant, browser: &Browser, to: &SeedTenant) -> String {
    let (status, body) =
        send(call(api, from, browser, Method::POST, SWITCH).json(&json!({"tenantId": to.id}))).await;
    assert_eq!(status, 200, "{body}");
    body["url"].as_str().unwrap().to_owned()
}

/// Where a browser navigation to `url` (absolute, on a tenant host) lands:
/// the `Location` and the new session cookie, if any.
async fn visit(api: &TestApi, url: &str) -> (String, Option<String>) {
    let rest = url.split_once("://").unwrap().1;
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let res = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{}{path}", api.base))
        .header("host", host)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 303);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    (location, set_cookie(&res, "mlbpp_session"))
}

/// The session of the cookie `token` on `tenant`: its JSON.
async fn session_of(api: &TestApi, tenant: &SeedTenant, token: &str) -> (u16, Value) {
    send(
        api.anonymous(Method::GET, tenant, SESSION)
            .header("cookie", format!("mlbpp_session={token}")),
    )
    .await
}

fn home(tenant: &SeedTenant) -> String {
    format!("https://{}.example.test/", tenant.slug)
}

fn sign_in_error(tenant: &SeedTenant, code: &str) -> String {
    format!("https://{}.example.test/sign-in?error={code}", tenant.slug)
}

#[tokio::test]
async fn the_switcher_lists_my_tenants_with_my_role_in_each() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, browser) = member_of_both(&api, &db).await;

    let (status, list) = send(call(&api, &db.tenant_a, &browser, Method::GET, MEMBERSHIPS)).await;
    assert_eq!(status, 200, "{list}");
    let list = list.as_array().unwrap();
    // Only this account's: not those of the other members of both tenants.
    assert_eq!(list.len(), 2, "{list:?}");
    let alpha = list
        .iter()
        .find(|m| m["tenantId"] == json!(db.tenant_a.id))
        .unwrap();
    let beta = list
        .iter()
        .find(|m| m["tenantId"] == json!(db.tenant_b.id))
        .unwrap();
    assert_eq!(alpha["slug"], db.tenant_a.slug);
    assert_eq!(alpha["name"], db.tenant_a.name);
    assert_eq!(alpha["url"], format!("https://{}.example.test", db.tenant_a.slug));
    assert_eq!(alpha["tenantRole"], "tenant_admin");
    assert_eq!(alpha["accountType"], "internal");
    assert_eq!(alpha["current"], true);
    assert_eq!(beta["tenantRole"], "member");
    assert_eq!(beta["current"], false);
    // By tenant name.
    let names: Vec<&str> = list.iter().map(|m| m["name"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);

    // `/me` describes the account in the current tenant.
    let (status, me) = send(call(&api, &db.tenant_a, &browser, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 200, "{me}");
    assert_eq!(me["id"], json!(member.account_id));
    assert_eq!(me["email"], member.email);
    assert_eq!(me["tenantId"], json!(db.tenant_a.id));
    assert_eq!(me["tenantRole"], "tenant_admin");
    assert_eq!(me["accountType"], "internal");
}

#[tokio::test]
async fn suspended_memberships_are_not_listed_nor_reachable() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, browser) = member_of_both(&api, &db).await;
    let link = start_switch(&api, &db.tenant_a, &browser, &db.tenant_b).await;
    db.exec_as_owner(&format!(
        "UPDATE membership SET status = 'suspended' WHERE account_id = '{}' AND tenant_id = '{}'",
        member.account_id, db.tenant_b.id
    ))
    .await;

    let (_, list) = send(call(&api, &db.tenant_a, &browser, Method::GET, MEMBERSHIPS)).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    // Suspended after the link was issued: no session.
    let (location, cookie) = visit(&api, &link).await;
    assert_eq!(location, sign_in_error(&db.tenant_b, "auth.switch_failed"));
    assert_eq!(cookie, None);
    let (status, body) = send(
        call(&api, &db.tenant_a, &browser, Method::POST, SWITCH).json(&json!({"tenantId": db.tenant_b.id})),
    )
    .await;
    assert_problem(status, &body, 404, "not_found");
}

#[tokio::test]
async fn single_tenant_mode_lists_the_current_tenant_only() {
    let db = TestDb::new().await;
    let api = start(
        &db,
        Tenancy::Single {
            default_slug: db.tenant_a.slug.clone(),
        },
    )
    .await;
    let (_, browser) = member_of_both(&api, &db).await;
    let (status, list) = send(call(&api, &db.tenant_a, &browser, Method::GET, MEMBERSHIPS)).await;
    assert_eq!(status, 200, "{list}");
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["tenantId"], json!(db.tenant_a.id));
    assert_eq!(list[0]["url"], "https://planner.example.test");
    // Another tenant is out of reach of this host.
    let (status, body) = send(
        call(&api, &db.tenant_a, &browser, Method::POST, SWITCH).json(&json!({"tenantId": db.tenant_b.id})),
    )
    .await;
    assert_problem(status, &body, 404, "not_found");
}

#[tokio::test]
async fn switching_opens_a_session_in_the_target_tenant_once() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, browser) = member_of_both(&api, &db).await;

    let link = start_switch(&api, &db.tenant_a, &browser, &db.tenant_b).await;
    let prefix = format!(
        "https://{}.example.test/api/v1/auth/switch?token=",
        db.tenant_b.slug
    );
    assert!(link.starts_with(&prefix), "{link}");

    let (location, cookie) = visit(&api, &link).await;
    assert_eq!(location, home(&db.tenant_b));
    let token = cookie.unwrap();
    let (status, session) = session_of(&api, &db.tenant_b, &token).await;
    assert_eq!(status, 200, "{session}");
    assert_eq!(session["stage"], "complete");
    assert_eq!(session["account"]["id"], json!(member.account_id));
    // The role in the target tenant.
    assert_eq!(session["account"]["tenantRole"], "member");

    // The link works once.
    let (location, cookie) = visit(&api, &link).await;
    assert_eq!(location, sign_in_error(&db.tenant_b, "auth.switch_failed"));
    assert_eq!(cookie, None);
    // The session left behind stays open on its own host.
    let (status, _) = session_of(&api, &db.tenant_a, &browser.token).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_switch_link_works_on_its_target_tenant_only() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (_, browser) = member_of_both(&api, &db).await;
    let link = start_switch(&api, &db.tenant_a, &browser, &db.tenant_b).await;

    // The same token on alpha's host.
    let foreign = link.replace(
        &format!("https://{}.", db.tenant_b.slug),
        &format!("https://{}.", db.tenant_a.slug),
    );
    let (location, cookie) = visit(&api, &foreign).await;
    assert_eq!(location, sign_in_error(&db.tenant_a, "auth.switch_failed"));
    assert_eq!(cookie, None);
    // Spent anyway.
    let (location, _) = visit(&api, &link).await;
    assert_eq!(location, sign_in_error(&db.tenant_b, "auth.switch_failed"));

    // Without token, or with an unknown one.
    let (location, _) = visit(
        &api,
        &format!("https://{}.example.test/api/v1/auth/switch", db.tenant_b.slug),
    )
    .await;
    assert_eq!(location, sign_in_error(&db.tenant_b, "auth.switch_failed"));
    let (location, _) = visit(
        &api,
        &format!(
            "https://{}.example.test/api/v1/auth/switch?token=unknown",
            db.tenant_b.slug
        ),
    )
    .await;
    assert_eq!(location, sign_in_error(&db.tenant_b, "auth.switch_failed"));
}

#[tokio::test]
async fn switching_needs_a_membership_and_the_csrf_token() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    // A member of alpha only.
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;

    for target in [db.tenant_b.id, Uuid::nil()] {
        let (status, body) =
            send(call(&api, &db.tenant_a, &browser, Method::POST, SWITCH).json(&json!({"tenantId": target})))
                .await;
        assert_problem(status, &body, 404, "not_found");
    }
    let (status, body) = send(
        api.anonymous(Method::POST, &db.tenant_a, SWITCH)
            .header("cookie", format!("mlbpp_session={}", browser.token))
            .json(&json!({"tenantId": db.tenant_a.id})),
    )
    .await;
    assert_problem(status, &body, 403, "csrf.invalid");
    let (status, body) = send(
        api.anonymous(Method::POST, &db.tenant_a, SWITCH)
            .json(&json!({"tenantId": db.tenant_a.id})),
    )
    .await;
    assert_problem(status, &body, 401, "unauthenticated");
}

#[tokio::test]
async fn a_non_member_cannot_sign_in_to_a_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;

    // The right password, on the host of a tenant the account is not in.
    let (status, body) = send(
        api.anonymous(Method::POST, &db.tenant_b, "/api/v1/auth/sign-in")
            .json(&json!({"email": member.email, "password": TEST_PASSWORD})),
    )
    .await;
    assert_problem(status, &body, 401, "auth.invalid_credentials");
    // Alpha's session on beta's host.
    let (status, body) = session_of(&api, &db.tenant_b, &browser.token).await;
    assert_problem(status, &body, 401, "unauthenticated");
    // A session crafted for beta, which the account does not belong to.
    let crafted = open(
        &api,
        member.account_id,
        &db.tenant_b,
        SessionStage::Complete,
        Proof::default(),
    )
    .await;
    let (status, body) = send(call(&api, &db.tenant_b, &crafted, Method::GET, "/api/v1/me")).await;
    assert_problem(status, &body, 401, "unauthenticated");
}

#[tokio::test]
async fn external_memberships_get_403_on_internal_endpoints() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    // Internal in alpha, external (request portal) in beta.
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(
        &db,
        &db.tenant_b,
        &member.email,
        TenantRole::Member,
        AccountType::External,
    )
    .await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member).await;

    let (_, list) = send(call(&api, &db.tenant_a, &browser, Method::GET, MEMBERSHIPS)).await;
    let beta = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tenantId"] == json!(db.tenant_b.id))
        .unwrap();
    assert_eq!(beta["accountType"], "external");
    let (status, body) = send(
        call(&api, &db.tenant_a, &browser, Method::POST, SWITCH).json(&json!({"tenantId": db.tenant_b.id})),
    )
    .await;
    assert_problem(status, &body, 403, "forbidden");

    // Even with a session of beta, whatever the role.
    let external = open(
        &api,
        member.account_id,
        &db.tenant_b,
        SessionStage::Complete,
        Proof::default(),
    )
    .await;
    for path in ["/api/v1/me", MEMBERSHIPS, SESSION, "/api/v1/reference-items"] {
        let (status, body) = send(call(&api, &db.tenant_b, &external, Method::GET, path)).await;
        assert_problem(status, &body, 403, "forbidden");
    }
}

#[tokio::test]
async fn nothing_of_the_previous_tenant_is_reachable_after_switching() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (_, browser) = member_of_both(&api, &db).await;
    let (status, created) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/reference-items",
        )
        .json(&json!({
            "code": "ALPHA-ONLY",
            "labelTranslations": {"fr": "Alpha", "en": "Alpha"},
            "quantity": 1
        })),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let item = created["id"].as_str().unwrap().to_owned();

    let link = start_switch(&api, &db.tenant_a, &browser, &db.tenant_b).await;
    let (_, cookie) = visit(&api, &link).await;
    let (_, session) = session_of(&api, &db.tenant_b, cookie.as_deref().unwrap()).await;
    let beta = Browser {
        token: cookie.unwrap(),
        csrf: session["csrfToken"].as_str().unwrap().to_owned(),
    };

    let (status, list) = send(call(
        &api,
        &db.tenant_b,
        &beta,
        Method::GET,
        "/api/v1/reference-items",
    ))
    .await;
    assert_eq!(status, 200, "{list}");
    assert!(!list.to_string().contains("ALPHA-ONLY"), "{list}");
    let (status, _) = send(call(
        &api,
        &db.tenant_b,
        &beta,
        Method::GET,
        &format!("/api/v1/reference-items/{item}"),
    ))
    .await;
    assert_eq!(status, 404);
    // Beta's session does not open alpha either.
    let (status, _) = session_of(&api, &db.tenant_a, &beta.token).await;
    assert_eq!(status, 401);
}

/// Sets beta's policy (as its admin).
async fn set_policy(api: &TestApi, tenant: &SeedTenant, require_mfa: bool, require_sso: bool) {
    let (status, body) = send(
        api.request(Method::PUT, tenant, "/api/v1/tenant/auth-policy")
            .header("if-match", "*")
            .json(&json!({
                "localAccountsRequireMfa": require_mfa,
                "internalAccountsRequireSso": require_sso,
                "portalPasswordEnabled": true,
                "portalMagicLinkEnabled": false
            })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn the_target_tenant_requires_its_own_identity_provider() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, browser) = member_of_both(&api, &db).await;
    let (status, provider) = send(
        api.request(Method::POST, &db.tenant_b, "/api/v1/tenant/oidc-providers")
            .json(&json!({
                "displayName": "Keycloak",
                "issuer": testkit::oidc_issuer(),
                "clientId": testkit::OIDC_CLIENT_ID,
                "clientSecret": testkit::OIDC_CLIENT_SECRET,
            })),
    )
    .await;
    assert_eq!(status, 201, "{provider}");
    set_policy(&api, &db.tenant_b, false, true).await;

    // Even after an SSO sign-in to alpha: beta trusts its own provider.
    let sso = open(
        &api,
        member.account_id,
        &db.tenant_a,
        SessionStage::Complete,
        Proof {
            second_factor: false,
            sso: true,
        },
    )
    .await;
    for from in [&browser, &sso] {
        let link = start_switch(&api, &db.tenant_a, from, &db.tenant_b).await;
        let (location, cookie) = visit(&api, &link).await;
        assert_eq!(location, sign_in_error(&db.tenant_b, "auth.sso_required"));
        assert_eq!(cookie, None);
    }
}

#[tokio::test]
async fn the_target_tenant_makes_members_without_mfa_enroll() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (_, browser) = member_of_both(&api, &db).await;
    set_policy(&api, &db.tenant_b, true, false).await;

    let link = start_switch(&api, &db.tenant_a, &browser, &db.tenant_b).await;
    let (location, cookie) = visit(&api, &link).await;
    // The application sends the session to the enrollment page.
    assert_eq!(location, home(&db.tenant_b));
    let token = cookie.unwrap();
    let (_, session) = session_of(&api, &db.tenant_b, &token).await;
    assert_eq!(session["stage"], "mfa_enrollment");
    // Not signed in to the application yet.
    let (status, body) = send(
        api.anonymous(Method::GET, &db.tenant_b, "/api/v1/me")
            .header("cookie", format!("mlbpp_session={token}")),
    )
    .await;
    assert_problem(status, &body, 401, "unauthenticated");
}

#[tokio::test]
async fn a_second_factor_is_asked_unless_the_session_left_checked_it() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (member, browser) = member_of_both(&api, &db).await;
    // Enables two-step verification (global to the account).
    let (status, setup) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::POST,
        "/api/v1/account/mfa/setup",
    ))
    .await;
    assert_eq!(status, 200, "{setup}");
    let secret = setup["secret"].as_str().unwrap();
    let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap();
    let code = infra::mfa::code_for_base32(secret, now).unwrap();
    let (status, enabled) = send(
        call(
            &api,
            &db.tenant_a,
            &browser,
            Method::POST,
            "/api/v1/account/mfa/confirm",
        )
        .json(&json!({"code": code})),
    )
    .await;
    assert_eq!(status, 200, "{enabled}");
    let backup_code = enabled["backupCodes"][0].as_str().unwrap().to_owned();

    // A session that checked no second factor (e.g. through alpha's SSO).
    let unproven = open(
        &api,
        member.account_id,
        &db.tenant_a,
        SessionStage::Complete,
        Proof {
            second_factor: false,
            sso: true,
        },
    )
    .await;
    let link = start_switch(&api, &db.tenant_a, &unproven, &db.tenant_b).await;
    let (_, cookie) = visit(&api, &link).await;
    let (_, session) = session_of(&api, &db.tenant_b, &cookie.unwrap()).await;
    assert_eq!(session["stage"], "mfa_challenge");

    // Password then a code on alpha: the switch is complete.
    let (challenged, body) = sign_in(&api, &db.tenant_a, &member).await;
    assert_eq!(body["stage"], "mfa_challenge");
    let res = call(
        &api,
        &db.tenant_a,
        &challenged,
        Method::POST,
        "/api/v1/auth/sign-in/verify",
    )
    .json(&json!({"code": backup_code}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let token = set_cookie(&res, "mlbpp_session").unwrap();
    let (_, verified) = json(res).await;
    let proven = Browser {
        token,
        csrf: verified["csrfToken"].as_str().unwrap().to_owned(),
    };
    let link = start_switch(&api, &db.tenant_a, &proven, &db.tenant_b).await;
    let (location, cookie) = visit(&api, &link).await;
    assert_eq!(location, home(&db.tenant_b));
    let beta = cookie.unwrap();
    let (_, session) = session_of(&api, &db.tenant_b, &beta).await;
    assert_eq!(session["stage"], "complete");

    // And from beta onwards, the proof carries over.
    let beta = Browser {
        token: beta,
        csrf: session["csrfToken"].as_str().unwrap().to_owned(),
    };
    let link = start_switch(&api, &db.tenant_b, &beta, &db.tenant_a).await;
    let (_, cookie) = visit(&api, &link).await;
    let (_, session) = session_of(&api, &db.tenant_a, &cookie.unwrap()).await;
    assert_eq!(session["stage"], "complete");
}
