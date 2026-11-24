//! Integration tests of resources (L07-US01) on real PostgreSQL and Valkey:
//! persons linked to a member or not, generic resources, one resource per
//! member, who manages and who reads them, versions, a member leaving the
//! tenant, tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::{AccountType, TenantRole};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const PATH: &str = "/api/v1/resources";

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

/// A session opened for `account` in `tenant`: its cookie and CSRF token.
async fn session_of(api: &TestApi, tenant: &SeedTenant, account: Uuid) -> (String, String) {
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account, tenant.id)
    .await
    .unwrap();
    (opened.token, opened.session.csrf_token)
}

/// A request by the member of `session`.
fn by(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &(String, String),
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

/// A member of `tenant` with `role`, and their session.
async fn member(
    db: &TestDb,
    api: &TestApi,
    tenant: &SeedTenant,
    role: TenantRole,
) -> (Uuid, (String, String)) {
    let account = db.add_member(tenant, role).await.account_id;
    (account, session_of(api, tenant, account).await)
}

async fn rename_account(db: &TestDb, account: Uuid, name: &str) {
    infra::sqlx::query("UPDATE account SET display_name = $2 WHERE id = $1")
        .bind(account)
        .bind(name)
        .execute(&db.owner_pool)
        .await
        .unwrap();
}

/// The resource `body`, created by the tenant admin of `tenant`.
async fn create(api: &TestApi, tenant: &SeedTenant, body: Value) -> Value {
    let (status, resource) = send(api.request(Method::POST, tenant, PATH).json(&body)).await;
    assert_eq!(status, 201, "{body}: {resource}");
    resource
}

/// The names `session` lists with `query`.
async fn names(api: &TestApi, tenant: &SeedTenant, session: &(String, String), query: &str) -> Vec<String> {
    let (status, page) = send(by(api, tenant, session, Method::GET, &format!("{PATH}{query}"))).await;
    assert_eq!(status, 200, "{page}");
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["displayName"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_tenant_admin_creates_persons_with_or_without_account_and_generic_resources() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (camille, _) = member(&db, &api, a, TenantRole::Member).await;
    rename_account(&db, camille, "Camille Martin").await;

    // A person linked to a member shows the member's name.
    let res = api
        .request(Method::POST, a, PATH)
        .json(&json!({"kind": "person", "accountId": camille, "jobTitle": " Dev back "}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["etag"], "\"1\"");
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    let (_, linked) = json(res).await;
    assert_eq!(location, format!("{PATH}/{}", linked["id"].as_str().unwrap()));
    assert_eq!(
        linked,
        json!({
            "id": linked["id"], "kind": "person", "displayName": "Camille Martin", "jobTitle": "Dev back",
            "accountId": camille, "formerMember": false, "active": true, "teams": [], "site": null, "version": 1
        })
    );

    // A person without account: its own name, nobody signs in as it.
    let contractor = create(
        &api,
        a,
        json!({"kind": "person", "displayName": "  Bob Subcontractor "}),
    )
    .await;
    assert_eq!(contractor["displayName"], "Bob Subcontractor");
    assert_eq!(contractor["accountId"], Value::Null);
    assert!(contractor.get("jobTitle").is_none());

    // A generic resource.
    let rust = create(
        &api,
        a,
        json!({"kind": "generic", "displayName": "Rust developer"}),
    )
    .await;
    assert_eq!(rust["kind"], "generic");

    let admin = (api.session(a).token.clone(), api.session(a).csrf_token.clone());
    assert_eq!(
        names(&api, a, &admin, "").await,
        ["Bob Subcontractor", "Camille Martin", "Rust developer"]
    );
    assert_eq!(names(&api, a, &admin, "?kind=generic").await, ["Rust developer"]);
    assert_eq!(
        names(&api, a, &admin, "?search=DEV%20BACK").await,
        ["Camille Martin"]
    );

    let (status, read) = send(api.request(Method::GET, a, &location)).await;
    assert_eq!((status, &read["displayName"]), (200, &json!("Camille Martin")));
    let res = api
        .request(Method::GET, a, &location)
        .header("if-none-match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
}

#[tokio::test]
async fn pages_follow_the_name_order() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    for name in ["Delta", "alpha", "Charlie", "bravo", "Echo"] {
        create(&api, a, json!({"kind": "generic", "displayName": name})).await;
    }
    let mut seen = Vec::new();
    let mut query = format!("{PATH}?limit=2");
    loop {
        let (status, page) = send(api.request(Method::GET, a, &query)).await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["canManage"], true);
        for item in page["items"].as_array().unwrap() {
            seen.push(item["displayName"].as_str().unwrap().to_owned());
        }
        match page["nextCursor"].as_str() {
            Some(cursor) => query = format!("{PATH}?limit=2&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(seen, ["alpha", "bravo", "Charlie", "Delta", "Echo"]);

    for bad in ["?limit=0", "?limit=201", "?cursor=nonsense", "?kind=room"] {
        let (status, problem) = send(api.request(Method::GET, a, &format!("{PATH}{bad}"))).await;
        assert_eq!(status, 400, "{bad}: {problem}");
    }
}

#[tokio::test]
async fn a_member_is_linked_to_one_resource_and_invalid_resources_are_refused() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (camille, _) = member(&db, &api, a, TenantRole::Member).await;
    let external = db
        .add_member_with(a, TenantRole::Member, AccountType::External)
        .await
        .account_id;
    let of_beta = db.add_member(&db.tenant_b, TenantRole::Member).await.account_id;
    create(&api, a, json!({"kind": "person", "accountId": camille})).await;

    // The members still to link: neither Camille nor the external one.
    let (status, free) =
        send(api.request(Method::GET, a, &format!("{PATH}/linkable-accounts?limit=50"))).await;
    assert_eq!(status, 200, "{free}");
    let free: Vec<&str> = free
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["accountId"].as_str().unwrap())
        .collect();
    assert!(!free.contains(&camille.to_string().as_str()));
    assert!(!free.contains(&external.to_string().as_str()));

    let (status, problem) = send(
        api.request(Method::POST, a, PATH)
            .json(&json!({"kind": "person", "accountId": camille})),
    )
    .await;
    assert_eq!(status, 409, "{problem}");
    assert_eq!(problem["code"], "resource.account_taken");
    assert_eq!(problem["errors"][0]["field"], "accountId");

    for (body, field, code) in [
        (
            json!({"kind": "person", "accountId": external}),
            "accountId",
            "not_internal_member",
        ),
        (
            json!({"kind": "person", "accountId": of_beta}),
            "accountId",
            "not_internal_member",
        ),
        (
            json!({"kind": "generic", "displayName": "X", "accountId": camille}),
            "accountId",
            "person_only",
        ),
        (
            json!({"kind": "person", "displayName": "  "}),
            "displayName",
            "required",
        ),
        (json!({"kind": "generic"}), "displayName", "required"),
        (
            json!({"kind": "generic", "displayName": "x".repeat(201)}),
            "displayName",
            "too_long",
        ),
        (
            json!({"kind": "generic", "displayName": "X", "jobTitle": "y".repeat(101)}),
            "jobTitle",
            "too_long",
        ),
    ] {
        let (status, problem) = send(api.request(Method::POST, a, PATH).json(&body)).await;
        assert_eq!(status, 400, "{body}: {problem}");
        assert_eq!(problem["code"], "validation_failed", "{body}");
        assert_eq!(
            problem["errors"][0],
            json!({"field": field, "code": code}),
            "{body}"
        );
    }
    let (status, _) = send(
        api.request(Method::POST, a, PATH)
            .json(&json!({"kind": "room", "displayName": "X"})),
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_change_needs_the_current_version_and_deactivates_without_deleting() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let rust = create(
        &api,
        a,
        json!({"kind": "generic", "displayName": "Rust developer"}),
    )
    .await;
    let path = format!("{PATH}/{}", rust["id"].as_str().unwrap());
    let body = json!({"kind": "generic", "displayName": "Rust developer", "active": false});

    let (status, _) = send(api.request(Method::PUT, a, &path).json(&body)).await;
    assert_eq!(status, 428);
    let (status, _) = send(
        api.request(Method::PUT, a, &path)
            .header("if-match", "\"2\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let res = api
        .request(Method::PUT, a, &path)
        .header("if-match", "\"1\"")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["etag"], "\"2\"");
    let (status, changed) = json(res).await;
    assert_eq!(
        (status, &changed["active"], &changed["version"]),
        (200, &json!(false), &json!(2))
    );

    let admin = (api.session(a).token.clone(), api.session(a).csrf_token.clone());
    assert!(names(&api, a, &admin, "?active=true").await.is_empty());
    assert_eq!(names(&api, a, &admin, "?active=false").await, ["Rust developer"]);
    let (status, _) = send(api.request(Method::DELETE, a, &path)).await;
    assert_eq!(status, 405, "resources are never deleted");
    let (status, _) = send(
        api.request(Method::PUT, a, &format!("{PATH}/{}", Uuid::nil()))
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn resource_managers_manage_resources_and_every_internal_member_reads_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (_, manager) = member(&db, &api, a, TenantRole::ResourceManager).await;
    let (_, plain) = member(&db, &api, a, TenantRole::Member).await;
    let (_, config_admin) = member(&db, &api, a, TenantRole::ConfigurationAdmin).await;
    let external = db
        .add_member_with(a, TenantRole::Member, AccountType::External)
        .await
        .account_id;
    let external = session_of(&api, a, external).await;

    let (status, created) = send(
        by(&api, a, &manager, Method::POST, PATH).json(&json!({"kind": "generic", "displayName": "Tester"})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let path = format!("{PATH}/{}", created["id"].as_str().unwrap());
    let (status, _) = send(
        by(&api, a, &manager, Method::PUT, &path)
            .header("if-match", "*")
            .json(&json!({"kind": "generic", "displayName": "QA tester"})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(by(
        &api,
        a,
        &manager,
        Method::GET,
        &format!("{PATH}/linkable-accounts"),
    ))
    .await;
    assert_eq!(status, 200);

    for reader in [&plain, &config_admin] {
        assert_eq!(names(&api, a, reader, "").await, ["QA tester"]);
        let (_, page) = send(by(&api, a, reader, Method::GET, PATH)).await;
        assert_eq!(page["canManage"], false);
        let (status, _) = send(by(&api, a, reader, Method::GET, &path)).await;
        assert_eq!(status, 200);
        let (status, problem) = send(
            by(&api, a, reader, Method::POST, PATH).json(&json!({"kind": "generic", "displayName": "X"})),
        )
        .await;
        assert_eq!((status, &problem["code"]), (403, &json!("forbidden")));
        let (status, _) = send(
            by(&api, a, reader, Method::PUT, &path)
                .header("if-match", "*")
                .json(&json!({"kind": "generic", "displayName": "X"})),
        )
        .await;
        assert_eq!(status, 403);
        let (status, _) = send(by(
            &api,
            a,
            reader,
            Method::GET,
            &format!("{PATH}/linkable-accounts"),
        ))
        .await;
        assert_eq!(status, 403);
    }
    // An external account never reaches the internal application.
    let (status, _) = send(by(&api, a, &external, Method::GET, PATH)).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_removed_member_leaves_an_inactive_resource_under_their_name() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (camille, _) = member(&db, &api, a, TenantRole::Member).await;
    rename_account(&db, camille, "Camille Martin").await;
    let resource = create(&api, a, json!({"kind": "person", "accountId": camille})).await;
    let path = format!("{PATH}/{}", resource["id"].as_str().unwrap());

    let (status, _) = send(
        api.request(Method::DELETE, a, &format!("/api/v1/tenant/members/{camille}"))
            .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 204);
    let (_, left) = send(api.request(Method::GET, a, &path)).await;
    assert_eq!(
        (
            &left["displayName"],
            &left["accountId"],
            &left["formerMember"],
            &left["active"]
        ),
        (
            &json!("Camille Martin"),
            &Value::Null,
            &json!(true),
            &json!(false)
        )
    );
    // Reactivated, it stays a person without account.
    let (status, back) = send(
        api.request(Method::PUT, a, &path)
            .header("if-match", "*")
            .json(&json!({"kind": "person", "displayName": "Camille Martin"})),
    )
    .await;
    assert_eq!(status, 200, "{back}");
    assert_eq!(
        (&back["active"], &back["formerMember"]),
        (&json!(true), &json!(true))
    );
}

#[tokio::test]
async fn a_tenant_never_sees_nor_changes_the_resources_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let of_alpha = create(
        &api,
        &db.tenant_a,
        json!({"kind": "generic", "displayName": "Rust developer"}),
    )
    .await;
    create(
        &api,
        &db.tenant_b,
        json!({"kind": "generic", "displayName": "Java developer"}),
    )
    .await;
    let path = format!("{PATH}/{}", of_alpha["id"].as_str().unwrap());
    let b = &db.tenant_b;
    let beta_admin = (api.session(b).token.clone(), api.session(b).csrf_token.clone());

    assert_eq!(names(&api, b, &beta_admin, "").await, ["Java developer"]);
    let (status, _) = send(api.request(Method::GET, b, &path)).await;
    assert_eq!(status, 404);
    let (status, _) = send(
        api.request(Method::PUT, b, &path)
            .header("if-match", "*")
            .json(&json!({"kind": "generic", "displayName": "Stolen"})),
    )
    .await;
    assert_eq!(status, 404);
    // A member of alpha cannot be linked from beta.
    let of_alpha_member = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let (status, problem) = send(
        api.request(Method::POST, b, PATH)
            .json(&json!({"kind": "person", "accountId": of_alpha_member})),
    )
    .await;
    assert_eq!(
        (status, &problem["errors"][0]["code"]),
        (400, &json!("not_internal_member"))
    );
}
