//! Integration tests of sign-in through the tenant's OIDC provider and of
//! the authentication policy (L02-US04), against a real provider: the
//! Keycloak of `deploy/compose.dev.yaml` (realm `mlbpp-test`).

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::{AccountType, NewMember, TenantRole};
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{ALICE, BOB, CAROL, OidcUser, SeedTenant, TEST_PASSWORD, TestDb, TestMember};

const PROVIDERS: &str = "/api/v1/tenant/oidc-providers";
const POLICY: &str = "/api/v1/tenant/auth-policy";

/// A browser that does not follow redirects: each step is checked.
fn browser() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn provider_body() -> Value {
    json!({
        "displayName": "Keycloak",
        "issuer": testkit::oidc_issuer(),
        "clientId": testkit::OIDC_CLIENT_ID,
        "clientSecret": testkit::OIDC_CLIENT_SECRET,
    })
}

/// Configures the test provider in `tenant` (as its admin); its id.
async fn add_provider(api: &TestApi, tenant: &SeedTenant) -> String {
    let (status, provider) = json(
        api.request(Method::POST, tenant, PROVIDERS)
            .json(&provider_body())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 201, "{provider}");
    provider["id"].as_str().unwrap().to_owned()
}

/// Adds `email` to `tenant` as a member of `account_type`, without password.
async fn add_member(db: &TestDb, tenant: &SeedTenant, email: &str, account_type: AccountType) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email,
            display_name: "Alice Martin",
            password_hash: None,
            account_type,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// The end of a sign-in: status, `Location` and session cookie of the callback.
struct Landing {
    status: u16,
    location: String,
    session: Option<String>,
}

/// Calls the API at `url` (absolute, on a tenant host) through the test server.
async fn visit(api: &TestApi, url: &str) -> reqwest::Response {
    let rest = url.split_once("://").unwrap().1;
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    browser()
        .get(format!("{}{path}", api.base))
        .header("host", host)
        .send()
        .await
        .unwrap()
}

fn landing(res: &reqwest::Response) -> Landing {
    Landing {
        status: res.status().as_u16(),
        location: res.headers()["location"].to_str().unwrap().to_owned(),
        session: res
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap())
            .find(|v| v.starts_with("mlbpp_session="))
            .map(|v| v.split(';').next().unwrap()["mlbpp_session=".len()..].to_owned()),
    }
}

/// Leaves `tenant` for the provider: the provider's login page URL.
async fn leave(api: &TestApi, tenant: &SeedTenant, provider: &str) -> String {
    let res = browser()
        .get(format!(
            "{}/api/v1/auth/oidc/{provider}/start?returnUrl=%2Fgantt",
            api.base
        ))
        .header("host", format!("{}.example.test", tenant.slug))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 303);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.starts_with(&testkit::oidc_issuer()), "{location}");
    location
}

/// The whole round trip of `user` through the provider.
async fn sign_in_through_provider(
    api: &TestApi,
    tenant: &SeedTenant,
    provider: &str,
    user: OidcUser,
) -> Landing {
    let login_page = leave(api, tenant, provider).await;
    let back = testkit::provider_login(&login_page, user).await;
    assert!(
        back.starts_with(&format!(
            "https://{}.example.test/api/v1/auth/oidc/callback?",
            tenant.slug
        )),
        "{back}"
    );
    landing(&visit(api, &back).await)
}

fn sign_in_error(tenant: &SeedTenant, code: &str) -> String {
    format!("https://{}.example.test/sign-in?error={code}", tenant.slug)
}

#[tokio::test]
async fn a_member_signs_in_through_the_tenants_provider() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let provider = add_provider(&api, &db.tenant_a).await;
    add_member(&db, &db.tenant_a, ALICE.email, AccountType::Internal).await;

    // The sign-in page lists it.
    let (status, buttons) = json(
        api.anonymous(Method::GET, &db.tenant_a, "/api/v1/auth/providers")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, buttons),
        (200, json!([{"id": provider, "displayName": "Keycloak"}]))
    );

    let landed = sign_in_through_provider(&api, &db.tenant_a, &provider, ALICE).await;
    assert_eq!(landed.status, 303);
    assert_eq!(
        landed.location, "https://alpha.example.test/gantt",
        "back where the member was going"
    );
    let token = landed.session.expect("a session");
    let (status, session) = json(
        api.anonymous(Method::GET, &db.tenant_a, "/api/v1/auth/session")
            .header("cookie", format!("mlbpp_session={token}"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(session["account"]["email"], ALICE.email);
    assert_eq!(session["stage"], "complete");
    // The session records the SSO sign-in, which a tenant switch carries.
    let stored = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: common::session_settings().idle,
            max_age: common::session_settings().max_age,
        },
    )
    .get(&token)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        stored.proof(),
        infra::sessions::Proof {
            second_factor: false,
            sso: true
        }
    );
}

#[tokio::test]
async fn an_invalid_used_or_foreign_state_is_rejected() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let provider = add_provider(&api, &db.tenant_a).await;
    add_provider(&api, &db.tenant_b).await;
    add_member(&db, &db.tenant_a, ALICE.email, AccountType::Internal).await;
    add_member(&db, &db.tenant_b, ALICE.email, AccountType::Internal).await;

    // A state this API never issued.
    let forged = landing(
        &visit(
            &api,
            "https://alpha.example.test/api/v1/auth/oidc/callback?state=forged&code=x",
        )
        .await,
    );
    assert_eq!(forged.location, sign_in_error(&db.tenant_a, "auth.oidc_failed"));
    assert!(forged.session.is_none());

    // A genuine answer, brought to another tenant: refused, and spent.
    let login_page = leave(&api, &db.tenant_a, &provider).await;
    let back = testkit::provider_login(&login_page, ALICE).await;
    let foreign = landing(&visit(&api, &back.replace("alpha.example.test", "beta.example.test")).await);
    assert_eq!(foreign.location, sign_in_error(&db.tenant_b, "auth.oidc_failed"));
    assert!(foreign.session.is_none());
    let replayed = landing(&visit(&api, &back).await);
    assert_eq!(
        replayed.location,
        sign_in_error(&db.tenant_a, "auth.oidc_failed"),
        "a state works once"
    );

    // A fresh one works, once.
    let login_page = leave(&api, &db.tenant_a, &provider).await;
    let back = testkit::provider_login(&login_page, ALICE).await;
    assert!(landing(&visit(&api, &back).await).session.is_some());
    let again = landing(&visit(&api, &back).await);
    assert_eq!(again.location, sign_in_error(&db.tenant_a, "auth.oidc_failed"));
    assert!(again.session.is_none());
}

#[tokio::test]
async fn only_a_verified_email_of_an_active_internal_member_opens_a_session() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let provider = add_provider(&api, &db.tenant_a).await;
    add_member(&db, &db.tenant_a, BOB.email, AccountType::Internal).await;
    // Alice is only an external member: the portal, not the application.
    add_member(&db, &db.tenant_a, ALICE.email, AccountType::External).await;

    for (user, code) in [
        (BOB, "auth.oidc_email_unverified"),
        (CAROL, "auth.oidc_no_member"),
        (ALICE, "auth.oidc_no_member"),
    ] {
        let landed = sign_in_through_provider(&api, &db.tenant_a, &provider, user).await;
        assert_eq!(
            landed.location,
            sign_in_error(&db.tenant_a, code),
            "{}",
            user.username
        );
        assert!(landed.session.is_none());
    }
}

#[tokio::test]
async fn a_disabled_or_unknown_provider_cannot_be_used() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let provider = add_provider(&api, &db.tenant_a).await;
    let mut body = provider_body();
    body["enabled"] = json!(false);
    let (status, _) = json(
        api.request(Method::PUT, &db.tenant_a, &format!("{PROVIDERS}/{provider}"))
            .header("if-match", "*")
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    for id in [provider.as_str(), "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8b"] {
        let res = browser()
            .get(format!("{}/api/v1/auth/oidc/{id}/start", api.base))
            .header("host", "alpha.example.test")
            .send()
            .await
            .unwrap();
        assert_eq!(
            landing(&res).location,
            sign_in_error(&db.tenant_a, "auth.oidc_failed")
        );
    }
    let (_, buttons) = json(
        api.anonymous(Method::GET, &db.tenant_a, "/api/v1/auth/providers")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(buttons, json!([]), "disabled providers have no button");
}

async fn password_sign_in(api: &TestApi, tenant: &SeedTenant, member: &TestMember) -> (u16, Value) {
    json(
        api.anonymous(Method::POST, tenant, "/api/v1/auth/sign-in")
            .json(&json!({"email": member.email, "password": TEST_PASSWORD}))
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn set_policy(api: &TestApi, tenant: &SeedTenant, require_sso: bool) {
    let (status, body) = json(
        api.request(Method::PUT, tenant, POLICY)
            .header("if-match", "*")
            .json(&json!({
                "localAccountsRequireMfa": false,
                "internalAccountsRequireSso": require_sso,
                "portalPasswordEnabled": true,
                "portalMagicLinkEnabled": false
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn the_sso_rule_refuses_passwords_of_internal_members_except_local_accounts() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let internal = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let contractor = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &contractor.email,
            display_name: "Thomas Leroy",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: true,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    set_policy(&api, &db.tenant_a, true).await;
    // No provider yet: nobody is locked out.
    assert_eq!(password_sign_in(&api, &db.tenant_a, &internal).await.0, 200);

    let provider = add_provider(&api, &db.tenant_a).await;
    let (status, body) = password_sign_in(&api, &db.tenant_a, &internal).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("auth.sso_required")),
        "{body}"
    );
    assert_eq!(
        password_sign_in(&api, &db.tenant_a, &contractor).await.0,
        200,
        "a local account"
    );
    // A wrong password still says so: the rule reveals nothing to strangers.
    let (status, body) = json(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/sign-in")
            .json(&json!({"email": internal.email, "password": "wrong password, long"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("auth.invalid_credentials"))
    );

    // The policy reads back, and lifting it lets passwords in again.
    let (status, policy) = json(
        api.request(Method::GET, &db.tenant_a, POLICY)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(policy["internalAccountsRequireSso"], true);
    assert_eq!(policy["portalMagicLinkEnabled"], false);
    set_policy(&api, &db.tenant_a, false).await;
    assert_eq!(password_sign_in(&api, &db.tenant_a, &internal).await.0, 200);
    let _ = provider;
}

#[tokio::test]
async fn tenant_admins_manage_providers_whose_secret_never_comes_back() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let res = api
        .request(Method::POST, &db.tenant_a, PROVIDERS)
        .json(&provider_body())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["etag"], "\"1\"");
    let (_, created) = json(res).await;
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        created["redirectUri"],
        "https://alpha.example.test/api/v1/auth/oidc/callback"
    );
    assert_eq!(created["emailClaim"], "email");
    assert_eq!(created["enabled"], true);
    assert!(created.get("clientSecret").is_none(), "{created}");

    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    let stored: Vec<u8> = infra::sqlx::query_scalar("SELECT client_secret_ciphertext FROM oidc_provider")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_ne!(
        stored.as_slice(),
        testkit::OIDC_CLIENT_SECRET.as_bytes(),
        "encrypted at rest"
    );

    let (status, body) = json(
        api.request(Method::POST, &db.tenant_a, PROVIDERS)
            .json(&provider_body())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("oidc_provider.duplicate"))
    );

    // Renamed without its secret: the stored one still signs members in.
    let mut renamed = provider_body();
    renamed["displayName"] = json!("Corporate SSO");
    renamed.as_object_mut().unwrap().remove("clientSecret");
    let (status, updated) = json(
        api.request(Method::PUT, &db.tenant_a, &format!("{PROVIDERS}/{id}"))
            .header("if-match", "\"1\"")
            .json(&renamed)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, updated["displayName"].as_str()),
        (200, Some("Corporate SSO"))
    );
    add_member(&db, &db.tenant_a, ALICE.email, AccountType::Internal).await;
    assert!(
        sign_in_through_provider(&api, &db.tenant_a, &id, ALICE)
            .await
            .session
            .is_some()
    );

    let (status, check) = json(
        api.request(Method::POST, &db.tenant_a, &format!("{PROVIDERS}/{id}/test"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(check["issuer"], testkit::oidc_issuer());

    let (status, list) = json(
        api.request(Method::GET, &db.tenant_a, PROVIDERS)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((status, list.as_array().unwrap().len()), (200, 1));
    let (status, _) = json(
        api.request(Method::GET, &db.tenant_b, PROVIDERS)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = json(
        api.request(Method::GET, &db.tenant_b, &format!("{PROVIDERS}/{id}"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 404, "another tenant's provider does not exist for beta");

    let (status, _) = json(
        api.request(Method::DELETE, &db.tenant_a, &format!("{PROVIDERS}/{id}"))
            .header("if-match", "\"2\"")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn an_unreachable_provider_is_reported_by_the_test() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let mut body = provider_body();
    body["issuer"] = json!("http://127.0.0.1:1/realms/none");
    let (_, created) = json(
        api.request(Method::POST, &db.tenant_a, PROVIDERS)
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let id = created["id"].as_str().unwrap();
    let (status, problem) = json(
        api.request(Method::POST, &db.tenant_a, &format!("{PROVIDERS}/{id}/test"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, problem["code"].as_str()),
        (502, Some("oidc_provider.unreachable"))
    );
    assert!(problem["detail"].is_string());
}

#[tokio::test]
async fn only_tenant_admins_reach_the_authentication_settings() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::ConfigurationAdmin).await;
    let res = api
        .anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/sign-in")
        .json(&json!({"email": member.email, "password": TEST_PASSWORD}))
        .send()
        .await
        .unwrap();
    let cookie = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned())
        .collect::<Vec<_>>()
        .join("; ");
    let (_, session) = json(res).await;
    let csrf = session["csrfToken"].as_str().unwrap();
    for (method, path) in [
        (Method::GET, POLICY.to_owned()),
        (Method::GET, PROVIDERS.to_owned()),
        (Method::POST, PROVIDERS.to_owned()),
    ] {
        let (status, body) = json(
            api.anonymous(method, &db.tenant_a, &path)
                .header("cookie", &cookie)
                .header("x-csrf-token", csrf)
                .json(&provider_body())
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (403, Some("forbidden")),
            "{path}"
        );
    }
}
