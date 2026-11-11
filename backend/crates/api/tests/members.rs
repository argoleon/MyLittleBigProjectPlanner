//! Integration tests of the administration of members and invitations
//! (L02-US06): invite, accept (new or existing account), change access,
//! remove with session revocation, permissions and cross-tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb};

const MEMBERS: &str = "/api/v1/tenant/members";
const INVITATIONS: &str = "/api/v1/tenant/invitations";

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

/// Signs `member` in to `tenant` with the password.
async fn sign_in(api: &TestApi, tenant: &SeedTenant, email: &str, password: &str) -> (Browser, Value) {
    let res = api
        .anonymous(Method::POST, tenant, "/api/v1/auth/sign-in")
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let token = set_cookie(&res, "mlbpp_session").unwrap();
    let (_, body) = json(res).await;
    let csrf = body["csrfToken"].as_str().unwrap().to_owned();
    (Browser { token, csrf }, body)
}

/// A unique email for an invitation.
fn invitee(name: &str) -> String {
    format!("{name}-{}@client.example", Uuid::now_v7().simple())
}

/// Invites `email` to `tenant` as its admin.
async fn invite(
    api: &TestApi,
    tenant: &SeedTenant,
    email: &str,
    account_type: &str,
    role: &str,
) -> (u16, Value) {
    send(
        api.request(Method::POST, tenant, INVITATIONS)
            .json(&json!({"email": email, "accountType": account_type, "tenantRole": role})),
    )
    .await
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

/// The token of the invitation link emailed to `email`, and the email.
async fn emailed_token(tenant: &SeedTenant, email: &str) -> (String, testkit::CaughtEmail) {
    let caught = testkit::caught_email(email).await;
    let prefix = format!("https://{}.example.test/accept-invitation?token=", tenant.slug);
    let start = caught.text.find(&prefix).expect("link");
    let token: String = caught.text[start + prefix.len()..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (token, caught)
}

/// Invites `email` and returns the token of the emailed link.
async fn invited(
    api: &TestApi,
    db: &TestDb,
    tenant: &SeedTenant,
    email: &str,
    account_type: &str,
    role: &str,
) -> String {
    let (status, body) = invite(api, tenant, email, account_type, role).await;
    assert_eq!(status, 201, "{body}");
    run_jobs(db).await;
    emailed_token(tenant, email).await.0
}

async fn accept(api: &TestApi, tenant: &SeedTenant, body: Value) -> (u16, Value, Option<String>) {
    let res = api
        .anonymous(Method::POST, tenant, "/api/v1/auth/invitation/accept")
        .json(&body)
        .send()
        .await
        .unwrap();
    let cookie = set_cookie(&res, "mlbpp_session");
    let (status, body) = json(res).await;
    (status, body, cookie)
}

async fn members_of(api: &TestApi, tenant: &SeedTenant, query: &str) -> Vec<Value> {
    let (status, page) = send(api.request(Method::GET, tenant, &format!("{MEMBERS}{query}"))).await;
    assert_eq!(status, 200, "{page}");
    page["items"].as_array().unwrap().clone()
}

async fn invitations_of(api: &TestApi, tenant: &SeedTenant) -> Vec<Value> {
    let (status, page) = send(api.request(Method::GET, tenant, INVITATIONS)).await;
    assert_eq!(status, 200, "{page}");
    page["items"].as_array().unwrap().clone()
}

/// Adds the account of `email` to `tenant` too.
async fn join(db: &TestDb, tenant: &SeedTenant, email: &str, role: TenantRole) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email,
            display_name: "Test Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: role,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_invitation_is_emailed_and_listed_until_accepted() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let email = invitee("p.durand");

    let res = api
        .request(Method::POST, &db.tenant_a, INVITATIONS)
        .json(&json!({"email": format!("  {}  ", email.to_uppercase()), "accountType": "external", "tenantRole": "member"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    assert_eq!(res.headers()["etag"], "\"1\"");
    let (_, created) = json(res).await;
    assert_eq!(created["email"], email);
    assert_eq!(created["accountType"], "external");
    assert_eq!(created["tenantRole"], "member");
    assert_eq!(created["status"], "pending");
    assert_eq!(created["invitedBy"], "Test Member");
    assert_eq!(
        location,
        format!("{INVITATIONS}/{}", created["id"].as_str().unwrap())
    );

    // "Invitation sent": listed with the invitations, not with the members.
    let listed = invitations_of(&api, &db.tenant_a).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], created["id"]);
    assert!(
        !members_of(&api, &db.tenant_a, "")
            .await
            .iter()
            .any(|m| m["email"] == email)
    );
    assert!(invitations_of(&api, &db.tenant_b).await.is_empty());

    // Sent by the worker, in the tenant's language.
    run_jobs(&db).await;
    let (token, caught) = emailed_token(&db.tenant_a, &email).await;
    assert_eq!(
        caught.subject,
        format!("Test Member vous invite à rejoindre {}", db.tenant_a.name)
    );
    assert!(caught.text.contains("portail de demandes"), "{}", caught.text);

    let (status, preview) = send(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/invitation")
            .json(&json!({"token": token})),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    assert_eq!(preview["email"], email);
    assert_eq!(preview["tenantName"], db.tenant_a.name);
    assert_eq!(preview["invitedBy"], "Test Member");
    assert_eq!(preview["accountType"], "external");
    assert_eq!(preview["newAccount"], true);
}

#[tokio::test]
async fn accepting_creates_the_account_and_signs_an_internal_member_in() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let email = invitee("camille");
    let token = invited(&api, &db, &db.tenant_a, &email, "internal", "configuration_admin").await;

    // A new account needs a name and a password; the link survives the mistake.
    let (status, body, cookie) =
        accept(&api, &db.tenant_a, json!({"token": token, "password": "short"})).await;
    assert_problem(status, &body, 400, "validation_failed");
    let fields: Vec<&str> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["displayName", "password"]);
    assert_eq!(cookie, None);

    let (status, accepted, cookie) = accept(
        &api,
        &db.tenant_a,
        json!({"token": token, "displayName": " Camille Martin ", "password": "a long enough passphrase"}),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["accountType"], "internal");
    assert_eq!(accepted["ssoRequired"], false);
    assert_eq!(accepted["session"]["stage"], "complete");
    assert_eq!(accepted["session"]["account"]["displayName"], "Camille Martin");
    assert_eq!(
        accepted["session"]["account"]["tenantRole"],
        "configuration_admin"
    );
    let browser = Browser {
        token: cookie.unwrap(),
        csrf: accepted["session"]["csrfToken"].as_str().unwrap().to_owned(),
    };
    let (status, _) = send(call(&api, &db.tenant_a, &browser, Method::GET, "/api/v1/me")).await;
    assert_eq!(status, 200);

    // Once.
    let (status, body, _) = accept(
        &api,
        &db.tenant_a,
        json!({"token": token, "displayName": "Someone", "password": "a long enough passphrase"}),
    )
    .await;
    assert_problem(status, &body, 400, "invitation.invalid");
    // Now a member, no longer an invitation.
    assert!(invitations_of(&api, &db.tenant_a).await.is_empty());
    let member = members_of(&api, &db.tenant_a, "")
        .await
        .into_iter()
        .find(|m| m["email"] == email)
        .unwrap();
    assert_eq!(member["tenantRole"], "configuration_admin");
    assert_eq!(member["status"], "active");
    // The password signs in.
    sign_in(&api, &db.tenant_a, &email, "a long enough passphrase").await;
}

#[tokio::test]
async fn an_existing_account_joins_with_its_password_unchanged() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let beta = db.add_member(&db.tenant_b, TenantRole::Member).await;
    let token = invited(&api, &db, &db.tenant_a, &beta.email, "internal", "member").await;

    let (_, preview) = send(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/invitation")
            .json(&json!({"token": token})),
    )
    .await;
    assert_eq!(preview["newAccount"], false);
    let (status, accepted, cookie) = accept(&api, &db.tenant_a, json!({"token": token})).await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["session"]["account"]["id"], json!(beta.account_id));
    assert!(cookie.is_some());
    // Same account, same password, in both tenants.
    sign_in(&api, &db.tenant_a, &beta.email, TEST_PASSWORD).await;
    sign_in(&api, &db.tenant_b, &beta.email, TEST_PASSWORD).await;
}

#[tokio::test]
async fn an_external_invitee_joins_without_a_session_of_the_application() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let email = invitee("p.durand");
    let token = invited(&api, &db, &db.tenant_a, &email, "external", "member").await;
    let (status, accepted, cookie) = accept(
        &api,
        &db.tenant_a,
        json!({"token": token, "displayName": "Paul Durand", "password": "a long enough passphrase"}),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["accountType"], "external");
    assert_eq!(accepted["session"], Value::Null);
    assert_eq!(cookie, None);
    let member = members_of(&api, &db.tenant_a, "?accountType=external").await;
    assert_eq!(member.len(), 1);
    assert_eq!(member[0]["email"], email);
}

#[tokio::test]
async fn accepting_follows_the_tenant_policy() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let (status, body) = send(
        api.request(Method::PUT, &db.tenant_a, "/api/v1/tenant/auth-policy")
            .header("if-match", "*")
            .json(&json!({
                "localAccountsRequireMfa": true,
                "internalAccountsRequireSso": false,
                "portalPasswordEnabled": true,
                "portalMagicLinkEnabled": false
            })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let email = invitee("ines");
    let token = invited(&api, &db, &db.tenant_a, &email, "internal", "member").await;
    let (status, accepted, cookie) = accept(
        &api,
        &db.tenant_a,
        json!({"token": token, "displayName": "Inès Moreau", "password": "a long enough passphrase"}),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["session"]["stage"], "mfa_enrollment");
    assert!(cookie.is_some());

    // With SSO required: the member signs in through the provider.
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
    let (status, body) = send(
        api.request(Method::PUT, &db.tenant_b, "/api/v1/tenant/auth-policy")
            .header("if-match", "*")
            .json(&json!({
                "localAccountsRequireMfa": false,
                "internalAccountsRequireSso": true,
                "portalPasswordEnabled": true,
                "portalMagicLinkEnabled": false
            })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let email = invitee("marion");
    let token = invited(&api, &db, &db.tenant_b, &email, "internal", "member").await;
    let (status, accepted, cookie) = accept(
        &api,
        &db.tenant_b,
        json!({"token": token, "displayName": "Marion Garnier", "password": "a long enough passphrase"}),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["session"], Value::Null);
    assert_eq!(accepted["ssoRequired"], true);
    assert_eq!(cookie, None);
}

#[tokio::test]
async fn invitations_are_checked_sent_again_and_revoked() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let (status, body) = invite(&api, &db.tenant_a, "not an email", "external", "tenant_admin").await;
    assert_problem(status, &body, 400, "validation_failed");
    let errors: Vec<(&str, &str)> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["field"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        errors,
        [("email", "email"), ("tenantRole", "external_member_only")]
    );

    // A member already: nothing to invite.
    let admin = &api.session(&db.tenant_a).member;
    let (status, body) = invite(&api, &db.tenant_a, &admin.email, "internal", "member").await;
    assert_problem(status, &body, 409, "invitation.already_member");

    let email = invitee("thomas");
    let first = invited(&api, &db, &db.tenant_a, &email, "internal", "member").await;
    let (status, body) = invite(&api, &db.tenant_a, &email, "internal", "member").await;
    assert_problem(status, &body, 409, "invitation.pending");

    // Expired: listed as such, its link no longer works.
    // Row-level security applies to the owner too: in alpha's context.
    db.exec_as_owner(&format!(
        "SELECT set_config('app.tenant_id', '{}', true); \
         UPDATE invitation SET expires_at = now() - interval '1 second' WHERE email = '{email}'",
        db.tenant_a.id
    ))
    .await;
    let listed = invitations_of(&api, &db.tenant_a).await;
    assert_eq!(listed[0]["status"], "expired");
    let id = listed[0]["id"].as_str().unwrap().to_owned();
    let (status, body) = send(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/invitation")
            .json(&json!({"token": first})),
    )
    .await;
    assert_problem(status, &body, 400, "invitation.invalid");

    // Sent again: a new link, valid again.
    testkit::delete_caught_emails(&email).await;
    let res = api
        .request(Method::POST, &db.tenant_a, &format!("{INVITATIONS}/{id}/resend"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let etag = res.headers()["etag"].to_str().unwrap().to_owned();
    let (_, resent) = json(res).await;
    assert_eq!(resent["status"], "pending");
    run_jobs(&db).await;
    let (second, _) = emailed_token(&db.tenant_a, &email).await;
    assert_ne!(second, first);

    // Revoked: the link stops working, once and for all.
    let (status, body) =
        send(api.request(Method::DELETE, &db.tenant_a, &format!("{INVITATIONS}/{id}"))).await;
    assert_problem(status, &body, 428, "precondition_required");
    let (status, body) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{INVITATIONS}/{id}"))
            .header("if-match", "\"1\""),
    )
    .await;
    assert_problem(status, &body, 412, "precondition_failed");
    let (status, _) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{INVITATIONS}/{id}"))
            .header("if-match", etag),
    )
    .await;
    assert_eq!(status, 204);
    let (status, body, _) = accept(
        &api,
        &db.tenant_a,
        json!({"token": second, "displayName": "Thomas", "password": "a long enough passphrase"}),
    )
    .await;
    assert_problem(status, &body, 400, "invitation.invalid");
    let (status, body) =
        send(api.request(Method::POST, &db.tenant_a, &format!("{INVITATIONS}/{id}/resend"))).await;
    assert_problem(status, &body, 409, "invitation.closed");
    assert!(invitations_of(&api, &db.tenant_a).await.is_empty());
    // Invite again after a revocation.
    let (status, body) = invite(&api, &db.tenant_a, &email, "internal", "member").await;
    assert_eq!(status, 201, "{body}");
}

#[tokio::test]
async fn an_invitation_belongs_to_its_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let email = invitee("leclerc");
    let token = invited(&api, &db, &db.tenant_b, &email, "internal", "member").await;
    let id = invitations_of(&api, &db.tenant_b).await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Beta's token on alpha's host.
    let (status, body) = send(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/invitation")
            .json(&json!({"token": token})),
    )
    .await;
    assert_problem(status, &body, 400, "invitation.invalid");
    // Beta's invitation through alpha.
    let (status, _) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{INVITATIONS}/{id}"))
            .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) =
        send(api.request(Method::POST, &db.tenant_a, &format!("{INVITATIONS}/{id}/resend"))).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn only_tenant_admins_manage_members() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    for role in [TenantRole::ConfigurationAdmin, TenantRole::Member] {
        let member = db.add_member(&db.tenant_a, role).await;
        let (browser, _) = sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
        let target = format!("{MEMBERS}/{}", member.account_id);
        let requests = [
            call(&api, &db.tenant_a, &browser, Method::GET, MEMBERS),
            call(&api, &db.tenant_a, &browser, Method::GET, &target),
            call(&api, &db.tenant_a, &browser, Method::PUT, &target)
                .header("if-match", "*")
                .json(
                    &json!({"accountType": "internal", "tenantRole": "tenant_admin", "localAccount": false}),
                ),
            call(&api, &db.tenant_a, &browser, Method::DELETE, &target).header("if-match", "*"),
            call(&api, &db.tenant_a, &browser, Method::GET, INVITATIONS),
            call(&api, &db.tenant_a, &browser, Method::POST, INVITATIONS)
                .json(&json!({"email": invitee("x"), "accountType": "internal", "tenantRole": "member"})),
            call(&api, &db.tenant_a, &browser, Method::GET, "/api/v1/tenant/groups"),
            call(
                &api,
                &db.tenant_a,
                &browser,
                Method::POST,
                "/api/v1/tenant/groups",
            )
            .json(&json!({"name": "Platform"})),
        ];
        for request in requests {
            let (status, body) = send(request).await;
            assert_problem(status, &body, 403, "forbidden");
        }
    }
    let (status, body) = send(api.anonymous(Method::GET, &db.tenant_a, MEMBERS)).await;
    assert_problem(status, &body, 401, "unauthenticated");
}

#[tokio::test]
async fn members_are_listed_searched_filtered_and_paged() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let internal = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let external = db
        .add_member_with(&db.tenant_a, TenantRole::Member, AccountType::External)
        .await;
    db.exec_as_owner(&format!(
        "UPDATE account SET display_name = 'Inès Moreau' WHERE id = '{}'",
        internal.account_id
    ))
    .await;

    let all = members_of(&api, &db.tenant_a, "").await;
    // The seeded admin, then the two members, in the order they joined.
    assert_eq!(all.len(), 3);
    assert_eq!(all[1]["accountId"], json!(internal.account_id));
    assert_eq!(all[1]["displayName"], "Inès Moreau");
    assert_eq!(all[1]["accountType"], "internal");
    assert_eq!(all[1]["tenantRole"], "member");
    assert_eq!(all[1]["mfaEnabled"], false);
    assert_eq!(all[1]["localAccount"], false);
    assert_eq!(all[1]["groups"], json!([]));
    assert_eq!(all[1]["version"], 1);
    assert_eq!(all[2]["accountType"], "external");

    let found = members_of(&api, &db.tenant_a, "?search=in%C3%A8s%20MOR").await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["accountId"], json!(internal.account_id));
    let by_email = members_of(&api, &db.tenant_a, &format!("?search={}", &external.email[..20])).await;
    assert_eq!(by_email.len(), 1);
    assert_eq!(by_email[0]["accountId"], json!(external.account_id));
    // Wildcards are plain characters.
    assert!(members_of(&api, &db.tenant_a, "?search=%25").await.is_empty());
    let externals = members_of(&api, &db.tenant_a, "?accountType=external").await;
    assert_eq!(externals.len(), 1);
    assert_eq!(
        members_of(&api, &db.tenant_a, "?accountType=internal")
            .await
            .len(),
        2
    );
    let (status, body) =
        send(api.request(Method::GET, &db.tenant_a, &format!("{MEMBERS}?accountType=robot"))).await;
    assert_problem(status, &body, 400, "invalid_query");

    // Pages of one.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let query = match &cursor {
            Some(c) => format!("?limit=1&cursor={c}"),
            None => "?limit=1".to_owned(),
        };
        let (_, page) = send(api.request(Method::GET, &db.tenant_a, &format!("{MEMBERS}{query}"))).await;
        seen.push(page["items"][0]["accountId"].clone());
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    let ids: Vec<Value> = all.iter().map(|m| m["accountId"].clone()).collect();
    assert_eq!(seen, ids);

    // Beta's admin sees beta's members only.
    let beta = members_of(&api, &db.tenant_b, "").await;
    assert_eq!(beta.len(), 1);
    let (status, _) = send(api.request(
        Method::GET,
        &db.tenant_b,
        &format!("{MEMBERS}/{}", internal.account_id),
    ))
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_new_role_applies_to_the_next_request() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
    let target = format!("{MEMBERS}/{}", member.account_id);
    let body = json!({"accountType": "internal", "tenantRole": "configuration_admin", "localAccount": true});

    let (status, problem) = send(api.request(Method::PUT, &db.tenant_a, &target).json(&body)).await;
    assert_problem(status, &problem, 428, "precondition_required");
    let (status, problem) = send(
        api.request(Method::PUT, &db.tenant_a, &target)
            .header("if-match", "\"7\"")
            .json(&body),
    )
    .await;
    assert_problem(status, &problem, 412, "precondition_failed");
    let (status, problem) = send(
        api.request(Method::PUT, &db.tenant_a, &target)
            .header("if-match", "\"1\"")
            .json(&json!({"accountType": "external", "tenantRole": "tenant_admin", "localAccount": false})),
    )
    .await;
    assert_problem(status, &problem, 400, "validation_failed");

    let res = api
        .request(Method::PUT, &db.tenant_a, &target)
        .header("if-match", "\"1\"")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["etag"], "\"2\"");
    let (_, updated) = json(res).await;
    assert_eq!(updated["tenantRole"], "configuration_admin");
    assert_eq!(updated["localAccount"], true);

    // Their next request: a configuration admin, who manages no member.
    let (_, session) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::GET,
        "/api/v1/auth/session",
    ))
    .await;
    assert_eq!(session["account"]["tenantRole"], "configuration_admin");
    let (status, problem) = send(call(&api, &db.tenant_a, &browser, Method::GET, MEMBERS)).await;
    assert_problem(status, &problem, 403, "forbidden");
    // Unknown member.
    let (status, _) = send(
        api.request(
            Method::PUT,
            &db.tenant_a,
            &format!("{MEMBERS}/{}", Uuid::now_v7()),
        )
        .header("if-match", "*")
        .json(&body),
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn the_last_tenant_admin_stays_one() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let admin = api.session(&db.tenant_a).member.clone();
    let target = format!("{MEMBERS}/{}", admin.account_id);
    let demote = json!({"accountType": "internal", "tenantRole": "member", "localAccount": false});

    let (status, body) = send(
        api.request(Method::PUT, &db.tenant_a, &target)
            .header("if-match", "*")
            .json(&demote),
    )
    .await;
    assert_problem(status, &body, 409, "member.last_admin");
    let (status, body) = send(
        api.request(Method::DELETE, &db.tenant_a, &target)
            .header("if-match", "*"),
    )
    .await;
    assert_problem(status, &body, 409, "member.last_admin");

    // With another admin, they can step down.
    db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    let (status, body) = send(
        api.request(Method::PUT, &db.tenant_a, &target)
            .header("if-match", "*")
            .json(&demote),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn a_removed_member_loses_this_tenant_only() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(&db, &db.tenant_b, &member.email, TenantRole::Member).await;
    let (alpha, _) = sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
    let (beta, _) = sign_in(&api, &db.tenant_b, &member.email, TEST_PASSWORD).await;

    let (status, group) = send(
        api.request(Method::POST, &db.tenant_a, "/api/v1/tenant/groups")
            .json(&json!({"name": "Platform"})),
    )
    .await;
    assert_eq!(status, 201, "{group}");
    let group = group["id"].as_str().unwrap().to_owned();
    let (status, _) = send(api.request(
        Method::PUT,
        &db.tenant_a,
        &format!("/api/v1/tenant/groups/{group}/members/{}", member.account_id),
    ))
    .await;
    assert_eq!(status, 204);

    let (status, _) = send(
        api.request(
            Method::DELETE,
            &db.tenant_a,
            &format!("{MEMBERS}/{}", member.account_id),
        )
        .header("if-match", "\"1\""),
    )
    .await;
    assert_eq!(status, 204);

    // Their next call to alpha is refused; beta is untouched.
    let (status, body) = send(call(
        &api,
        &db.tenant_a,
        &alpha,
        Method::GET,
        "/api/v1/auth/session",
    ))
    .await;
    assert_problem(status, &body, 401, "unauthenticated");
    let (status, _) = send(call(
        &api,
        &db.tenant_b,
        &beta,
        Method::GET,
        "/api/v1/auth/session",
    ))
    .await;
    assert_eq!(status, 200);
    assert!(
        !members_of(&api, &db.tenant_a, "")
            .await
            .iter()
            .any(|m| m["accountId"] == json!(member.account_id))
    );
    // Out of alpha's groups too.
    let (_, detail) = send(api.request(
        Method::GET,
        &db.tenant_a,
        &format!("/api/v1/tenant/groups/{group}"),
    ))
    .await;
    assert_eq!(detail["members"], json!([]));
    assert_eq!(detail["memberCount"], 0);
    // The password no longer opens alpha.
    let (status, body) = send(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/sign-in")
            .json(&json!({"email": member.email, "password": TEST_PASSWORD})),
    )
    .await;
    assert_problem(status, &body, 401, "auth.invalid_credentials");
}

#[tokio::test]
async fn a_member_made_external_is_signed_out_of_the_application() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (browser, _) = sign_in(&api, &db.tenant_a, &member.email, TEST_PASSWORD).await;
    let (status, body) = send(
        api.request(
            Method::PUT,
            &db.tenant_a,
            &format!("{MEMBERS}/{}", member.account_id),
        )
        .header("if-match", "*")
        .json(&json!({"accountType": "external", "tenantRole": "member", "localAccount": false})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = send(call(
        &api,
        &db.tenant_a,
        &browser,
        Method::GET,
        "/api/v1/auth/session",
    ))
    .await;
    assert_problem(status, &body, 401, "unauthenticated");
}
