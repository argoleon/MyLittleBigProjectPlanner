//! Integration tests of node roles and of the permission engine (L02-US07),
//! on PostgreSQL and Valkey: roles CRUD, default roles, languages, refusal
//! to delete a role in use, the cached permission check and its
//! invalidation. Grants arrive with L03: a test grant source stands in.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use api::AppState;
use api::permissions;
use axum::extract::State;
use axum::routing::post;
use common::{TestApi, json, multi, start, start_with};
use domain::permissions::Permission;
use infra::accounts::TenantRole;
use infra::permissions::{GrantFuture, GrantSource, PermissionCache};
use infra::sqlx::postgres::PgConnection;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TEST_PASSWORD, TestDb};

const ROLES: &str = "/api/v1/tenant/roles";
const PROBE: &str = "/api/v1/test/comment";

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

async fn roles_of(api: &TestApi, tenant: &SeedTenant, language: &str) -> Vec<Value> {
    let (status, page) = send(
        api.request(Method::GET, tenant, ROLES)
            .header("accept-language", language),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    page["items"].as_array().unwrap().clone()
}

fn find<'a>(roles: &'a [Value], label_en: &str) -> &'a Value {
    roles
        .iter()
        .find(|r| r["labelTranslations"]["en"] == label_en)
        .unwrap_or_else(|| panic!("no role {label_en}"))
}

#[tokio::test]
async fn every_tenant_starts_with_four_roles_shown_in_the_users_language() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let english = roles_of(&api, &db.tenant_a, "en").await;
    let labels: Vec<&str> = english.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["Reader", "Contributor", "Project manager", "Node admin"]);
    assert_eq!(english[0]["permissions"], json!(["view"]));
    assert_eq!(english[3]["permissions"].as_array().unwrap().len(), 10);
    assert_eq!(english[0]["grantCount"], 0);
    let french = roles_of(&api, &db.tenant_a, "fr-FR").await;
    assert_eq!(french[2]["label"], "Chef de projet");
    assert_eq!(
        french[2]["labelTranslations"],
        json!({"fr": "Chef de projet", "en": "Project manager"})
    );
}

#[tokio::test]
async fn a_tenant_admin_creates_changes_and_deletes_a_role() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let body = json!({
        "labelTranslations": {"fr": "Chef de projet senior", "en": "Senior project manager"},
        "permissions": ["view", "create_ticket", "edit", "comment", "log_time", "assign_resources", "force_overload", "manage_sprints"]
    });
    let res = api
        .request(Method::POST, &db.tenant_a, ROLES)
        .header("accept-language", "fr")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    let (_, role) = json(res).await;
    assert_eq!(role["label"], "Chef de projet senior");
    assert_eq!(role["permissions"], body["permissions"]);
    assert_eq!(location, format!("{ROLES}/{}", role["id"].as_str().unwrap()));
    let id = role["id"].as_str().unwrap().to_owned();

    // Labels are unique per language, whatever the case; another tenant may reuse them.
    let (status, problem) = send(api.request(Method::POST, &db.tenant_a, ROLES).json(&json!({
        "labelTranslations": {"en": "SENIOR PROJECT MANAGER"},
        "permissions": ["view"]
    })))
    .await;
    assert_problem(status, &problem, 409, "role.label_taken");
    let (status, _) = send(api.request(Method::POST, &db.tenant_b, ROLES).json(&body)).await;
    assert_eq!(status, 201);

    // Invalid: no label, no permission; an unknown permission is not JSON of a role.
    let (status, problem) = send(api.request(Method::POST, &db.tenant_a, ROLES).json(&json!({
        "labelTranslations": {"de": "Leser"},
        "permissions": []
    })))
    .await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(problem["errors"].as_array().unwrap().len(), 3);
    let (status, problem) = send(api.request(Method::POST, &db.tenant_a, ROLES).json(&json!({
        "labelTranslations": {"en": "Root"},
        "permissions": ["delete_everything"]
    })))
    .await;
    assert_problem(status, &problem, 400, "invalid_json");

    // Changed with its version.
    let changed = json!({"labelTranslations": {"fr": "Chef de projet senior", "en": "Senior PM"}, "permissions": ["view", "comment"]});
    let (status, problem) = send(
        api.request(Method::PUT, &db.tenant_a, &format!("{ROLES}/{id}"))
            .json(&changed),
    )
    .await;
    assert_problem(status, &problem, 428, "precondition_required");
    let res = api
        .request(Method::PUT, &db.tenant_a, &format!("{ROLES}/{id}"))
        .header("if-match", "\"1\"")
        .header("accept-language", "en")
        .json(&changed)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["etag"], "\"2\"");
    let (_, role) = json(res).await;
    assert_eq!(role["label"], "Senior PM");
    assert_eq!(role["permissions"], json!(["view", "comment"]));

    let (status, problem) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{ROLES}/{id}"))
            .header("if-match", "\"1\""),
    )
    .await;
    assert_problem(status, &problem, 412, "precondition_failed");
    let (status, _) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{ROLES}/{id}"))
            .header("if-match", "\"2\""),
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = send(api.request(Method::GET, &db.tenant_a, &format!("{ROLES}/{id}"))).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn roles_belong_to_their_tenant_and_to_tenant_admins() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let beta_role = roles_of(&api, &db.tenant_b, "en").await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for request in [
        api.request(Method::GET, &db.tenant_a, &format!("{ROLES}/{beta_role}")),
        api.request(Method::PUT, &db.tenant_a, &format!("{ROLES}/{beta_role}"))
            .header("if-match", "*")
            .json(&json!({"labelTranslations": {"en": "Taken over"}, "permissions": ["view"]})),
        api.request(Method::DELETE, &db.tenant_a, &format!("{ROLES}/{beta_role}"))
            .header("if-match", "*"),
    ] {
        let (status, body) = send(request).await;
        assert_problem(status, &body, 404, "not_found");
    }
    // Alpha's list holds alpha's roles only.
    assert!(
        !roles_of(&api, &db.tenant_a, "en")
            .await
            .iter()
            .any(|r| r["id"] == beta_role)
    );

    let config_admin = db.add_member(&db.tenant_a, TenantRole::ConfigurationAdmin).await;
    let res = api
        .anonymous(Method::POST, &db.tenant_a, "/api/v1/auth/sign-in")
        .json(&json!({"email": config_admin.email, "password": TEST_PASSWORD}))
        .send()
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (_, session) = json(res).await;
    let (status, body) = send(
        api.anonymous(Method::GET, &db.tenant_a, ROLES)
            .header("cookie", &cookie)
            .header("x-csrf-token", session["csrfToken"].as_str().unwrap()),
    )
    .await;
    assert_problem(status, &body, 403, "forbidden");
}

/// Grants for the tests, until L03: roles granted to accounts, tenant wide.
#[derive(Debug, Default)]
struct TestGrants(Mutex<HashMap<Uuid, Vec<Uuid>>>);

impl TestGrants {
    fn grant(&self, account_id: Uuid, role_id: Uuid) {
        self.0
            .lock()
            .unwrap()
            .entry(account_id)
            .or_default()
            .push(role_id);
    }
}

impl GrantSource for TestGrants {
    fn roles_of<'a>(
        &'a self,
        _: &'a mut PgConnection,
        account_id: Uuid,
        _: Option<Uuid>,
    ) -> GrantFuture<'a, Vec<Uuid>> {
        let roles = self
            .0
            .lock()
            .unwrap()
            .get(&account_id)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { Ok(roles) })
    }

    fn grants_of_role<'a>(&'a self, _: &'a mut PgConnection, role_id: Uuid) -> GrantFuture<'a, u64> {
        let count = self
            .0
            .lock()
            .unwrap()
            .values()
            .flatten()
            .filter(|r| **r == role_id)
            .count();
        Box::pin(async move { Ok(u64::try_from(count).unwrap()) })
    }
}

/// An endpoint protected by the `comment` permission, as later features will be.
async fn comment(
    State(state): State<AppState>,
    mut tx: api::TenantTx,
) -> Result<axum::http::StatusCode, api::problem::Problem> {
    permissions::require(&state, &mut tx, Permission::Comment, None).await?;
    tx.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// The API with `grants` and the protected endpoint.
async fn start_with_grants(db: &TestDb, grants: Arc<TestGrants>) -> TestApi {
    start_with(db, multi(), move |state, settings| {
        let state = state.with_grants(grants);
        let probe = axum::Router::new()
            .route(PROBE, post(comment))
            .with_state(state.clone());
        api::app(state, settings).merge(api::with_middleware(probe, settings))
    })
    .await
}

#[tokio::test]
async fn a_permission_check_is_cached_and_follows_role_changes() {
    let db = TestDb::new().await;
    let grants = Arc::new(TestGrants::default());
    let api = start_with_grants(&db, grants.clone()).await;
    let member = api.session(&db.tenant_a).member.account_id;
    let reader = find(&roles_of(&api, &db.tenant_a, "en").await, "Reader").clone();
    let reader_id = reader["id"].as_str().unwrap().to_owned();
    grants.grant(member, Uuid::parse_str(&reader_id).unwrap());

    // A reader may not comment: a stable 403.
    let (status, body) = send(api.request(Method::POST, &db.tenant_a, PROBE)).await;
    assert_problem(status, &body, 403, "permission.denied");
    assert_eq!(body["detail"], "missing permission: comment");

    // The answer is cached: a change behind the API's back is not seen…
    db.exec_as_owner(&format!(
        "SELECT set_config('app.tenant_id', '{}', true); \
         UPDATE role SET permissions = ARRAY['view', 'comment'] WHERE id = '{reader_id}'",
        db.tenant_a.id
    ))
    .await;
    let (status, _) = send(api.request(Method::POST, &db.tenant_a, PROBE)).await;
    assert_eq!(status, 403);

    // …while a change through the API applies to the next request.
    let (status, body) = send(
        api.request(Method::PUT, &db.tenant_a, &format!("{ROLES}/{reader_id}"))
            .header("if-match", "*")
            .json(&json!({"labelTranslations": reader["labelTranslations"], "permissions": ["view", "comment"]})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = send(api.request(Method::POST, &db.tenant_a, PROBE)).await;
    assert_eq!(status, 204);
    let (status, _) = send(
        api.request(Method::PUT, &db.tenant_a, &format!("{ROLES}/{reader_id}"))
            .header("if-match", "*")
            .json(&json!({"labelTranslations": reader["labelTranslations"], "permissions": ["view"]})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(api.request(Method::POST, &db.tenant_a, PROBE)).await;
    assert_eq!(status, 403);

    // Beta's member, without grant, may not either; nothing leaks across tenants.
    let (status, body) = send(api.request(Method::POST, &db.tenant_b, PROBE)).await;
    assert_problem(status, &body, 403, "permission.denied");
}

#[tokio::test]
async fn a_role_in_use_cannot_be_deleted() {
    let db = TestDb::new().await;
    let grants = Arc::new(TestGrants::default());
    let api = start_with_grants(&db, grants.clone()).await;
    let roles = roles_of(&api, &db.tenant_a, "en").await;
    let contributor = find(&roles, "Contributor")["id"].as_str().unwrap().to_owned();
    let contributor_id = Uuid::parse_str(&contributor).unwrap();
    grants.grant(api.session(&db.tenant_a).member.account_id, contributor_id);
    grants.grant(Uuid::now_v7(), contributor_id);

    let roles = roles_of(&api, &db.tenant_a, "en").await;
    assert_eq!(find(&roles, "Contributor")["grantCount"], 2);
    let (status, body) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{ROLES}/{contributor}"))
            .header("if-match", "*"),
    )
    .await;
    assert_problem(status, &body, 409, "role.in_use");
    assert_eq!(body["detail"], "used by 2 grants");
    let (status, _) = send(api.request(Method::GET, &db.tenant_a, &format!("{ROLES}/{contributor}"))).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn changes_of_roles_groups_and_members_invalidate_the_tenants_permissions() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let cache = PermissionCache::new(api.valkey.clone(), api.key_prefix.clone());
    let epoch = || async { cache.epoch(db.tenant_a.id).await.unwrap() };
    let beta_epoch = cache.epoch(db.tenant_b.id).await.unwrap();
    let start = epoch().await;

    let reader = find(&roles_of(&api, &db.tenant_a, "en").await, "Reader").clone();
    let (status, _) = send(
        api.request(
            Method::PUT,
            &db.tenant_a,
            &format!("{ROLES}/{}", reader["id"].as_str().unwrap()),
        )
        .header("if-match", "*")
        .json(&json!({"labelTranslations": reader["labelTranslations"], "permissions": ["view", "comment"]})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(epoch().await, start + 1);

    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (_, group) = send(
        api.request(Method::POST, &db.tenant_a, "/api/v1/tenant/groups")
            .json(&json!({"name": "Platform"})),
    )
    .await;
    let group_members = format!(
        "/api/v1/tenant/groups/{}/members/{}",
        group["id"].as_str().unwrap(),
        member.account_id
    );
    send(api.request(Method::PUT, &db.tenant_a, &group_members)).await;
    assert_eq!(epoch().await, start + 2);
    send(api.request(Method::DELETE, &db.tenant_a, &group_members)).await;
    assert_eq!(epoch().await, start + 3);
    send(
        api.request(
            Method::DELETE,
            &db.tenant_a,
            &format!("/api/v1/tenant/members/{}", member.account_id),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(epoch().await, start + 4);
    // Beta's cache is untouched.
    assert_eq!(cache.epoch(db.tenant_b.id).await.unwrap(), beta_epoch);
}
