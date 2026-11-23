//! Integration tests of teams (L07-US02) on real PostgreSQL and Valkey:
//! names in the language of the request, members added and removed, a
//! resource in several teams and the team filter of the resource list, who
//! manages and who reads, versions, tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const TEAMS: &str = "/api/v1/teams";
const RESOURCES: &str = "/api/v1/resources";

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

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant, role: TenantRole) -> (String, String) {
    let account = db.add_member(tenant, role).await.account_id;
    session_of(api, tenant, account).await
}

/// A request by the tenant admin of `tenant`.
fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path)
}

async fn create_team(api: &TestApi, tenant: &SeedTenant, body: Value) -> Value {
    let (status, team) = send(admin(api, tenant, Method::POST, TEAMS).json(&body)).await;
    assert_eq!(status, 201, "{body}: {team}");
    team
}

async fn create_resource(api: &TestApi, tenant: &SeedTenant, name: &str) -> String {
    let (status, resource) = send(
        admin(api, tenant, Method::POST, RESOURCES).json(&json!({"kind": "generic", "displayName": name})),
    )
    .await;
    assert_eq!(status, 201, "{resource}");
    resource["id"].as_str().unwrap().to_owned()
}

async fn add(api: &TestApi, tenant: &SeedTenant, team: &str, resource: &str) -> u16 {
    admin(
        api,
        tenant,
        Method::PUT,
        &format!("{TEAMS}/{team}/members/{resource}"),
    )
    .send()
    .await
    .unwrap()
    .status()
    .as_u16()
}

/// The names of the resources listed with `query`.
async fn resource_names(api: &TestApi, tenant: &SeedTenant, query: &str) -> Vec<String> {
    let (status, page) = send(admin(api, tenant, Method::GET, &format!("{RESOURCES}{query}"))).await;
    assert_eq!(status, 200, "{page}");
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["displayName"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_team_gathers_resources_named_in_the_language_of_the_request() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let res = admin(&api, a, Method::POST, TEAMS)
        .header("accept-language", "en")
        .json(
            &json!({"nameTranslations": {"fr": "Plateforme", "en": "Platform"}, "description": " Identity "}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.headers()["etag"], "\"1\"");
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    let (_, platform) = json(res).await;
    assert_eq!(
        platform,
        json!({
            "id": platform["id"], "name": "Platform", "nameTranslations": {"fr": "Plateforme", "en": "Platform"},
            "description": "Identity", "memberCount": 0, "version": 1
        })
    );
    let run = create_team(&api, a, json!({"nameTranslations": {"fr": "Exploitation"}})).await;
    let (pid, rid) = (platform["id"].as_str().unwrap(), run["id"].as_str().unwrap());

    // Three resources in Platform, one of them in Run too.
    let ids = [
        create_resource(&api, a, "Alice").await,
        create_resource(&api, a, "Bob").await,
        create_resource(&api, a, "Carol").await,
    ];
    for id in &ids {
        assert_eq!(add(&api, a, pid, id).await, 204);
    }
    assert_eq!(add(&api, a, pid, &ids[0]).await, 204, "already in: unchanged");
    assert_eq!(add(&api, a, rid, &ids[1]).await, 204);
    let (_, read) = send(admin(&api, a, Method::GET, &location).header("accept-language", "fr")).await;
    assert_eq!(
        (&read["name"], &read["memberCount"]),
        (&json!("Plateforme"), &json!(3))
    );
    assert_eq!(
        resource_names(&api, a, &format!("?team={pid}")).await,
        ["Alice", "Bob", "Carol"]
    );
    assert_eq!(resource_names(&api, a, &format!("?team={rid}")).await, ["Bob"]);

    // A resource tells its teams, by name in the language of the request.
    let res = admin(&api, a, Method::GET, &format!("{RESOURCES}/{}", ids[1]))
        .header("accept-language", "en")
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-language"], "en");
    let (_, bob) = json(res).await;
    assert_eq!(
        bob["teams"],
        json!([{"id": rid, "name": "Exploitation"}, {"id": pid, "name": "Platform"}])
    );

    // The list, by name in the request's language.
    let (_, list) = send(admin(&api, a, Method::GET, TEAMS).header("accept-language", "en")).await;
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Exploitation", "Platform"]);
    assert_eq!(list["canManage"], true);

    // Out of Platform, Bob stays in Run.
    let (status, _) = send(admin(
        &api,
        a,
        Method::DELETE,
        &format!("{TEAMS}/{pid}/members/{}", ids[1]),
    ))
    .await;
    assert_eq!(status, 204);
    let (status, _) = send(admin(
        &api,
        a,
        Method::DELETE,
        &format!("{TEAMS}/{pid}/members/{}", ids[1]),
    ))
    .await;
    assert_eq!(status, 404, "no longer in it");
    assert_eq!(
        resource_names(&api, a, &format!("?team={pid}")).await,
        ["Alice", "Carol"]
    );
    assert_eq!(resource_names(&api, a, &format!("?team={rid}")).await, ["Bob"]);
}

#[tokio::test]
async fn names_are_checked_unique_and_versions_kept() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let platform = create_team(&api, a, json!({"nameTranslations": {"fr": "Plateforme"}})).await;
    let path = format!("{TEAMS}/{}", platform["id"].as_str().unwrap());

    let (status, problem) =
        send(admin(&api, a, Method::POST, TEAMS).json(&json!({"nameTranslations": {"fr": "PLATEFORME"}})))
            .await;
    assert_eq!((status, &problem["code"]), (409, &json!("team.name_taken")));
    for (body, field, code) in [
        (json!({"nameTranslations": {}}), "nameTranslations", "required"),
        (
            json!({"nameTranslations": {"fr": "x".repeat(101)}}),
            "nameTranslations.fr",
            "too_long",
        ),
        (
            json!({"nameTranslations": {"de": "Plattform"}}),
            "nameTranslations",
            "required",
        ),
        (
            json!({"nameTranslations": {"fr": "Run"}, "description": "d".repeat(1001)}),
            "description",
            "too_long",
        ),
    ] {
        let (status, problem) = send(admin(&api, a, Method::POST, TEAMS).json(&body)).await;
        assert_eq!(status, 400, "{body}: {problem}");
        assert_eq!(
            problem["errors"][0],
            json!({"field": field, "code": code}),
            "{body}"
        );
    }

    let body = json!({"nameTranslations": {"fr": "Plateforme", "en": "Platform"}});
    let (status, _) = send(admin(&api, a, Method::PUT, &path).json(&body)).await;
    assert_eq!(status, 428);
    let (status, _) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"9\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let (status, changed) = send(
        admin(&api, a, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!((status, &changed["version"]), (200, &json!(2)));
    let (status, _) = send(admin(&api, a, Method::DELETE, &path).header("if-match", "\"1\"")).await;
    assert_eq!(status, 412);
    let (status, _) = send(admin(&api, a, Method::DELETE, &path).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204);
    let (status, _) = send(admin(&api, a, Method::GET, &path)).await;
    assert_eq!(status, 404);
    // Unknown team or resource.
    let resource = create_resource(&api, a, "Alice").await;
    assert_eq!(add(&api, a, &Uuid::nil().to_string(), &resource).await, 404);
    let other = create_team(&api, a, json!({"nameTranslations": {"fr": "Run"}})).await;
    assert_eq!(
        add(&api, a, other["id"].as_str().unwrap(), &Uuid::nil().to_string()).await,
        404
    );
}

#[tokio::test]
async fn resource_managers_manage_teams_and_every_internal_member_reads_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let manager = member(&db, &api, a, TenantRole::ResourceManager).await;
    let plain = member(&db, &api, a, TenantRole::Member).await;
    let resource = create_resource(&api, a, "Alice").await;

    let (status, team) = send(
        by(&api, a, &manager, Method::POST, TEAMS).json(&json!({"nameTranslations": {"fr": "Plateforme"}})),
    )
    .await;
    assert_eq!(status, 201, "{team}");
    let id = team["id"].as_str().unwrap();
    let (status, _) = send(by(
        &api,
        a,
        &manager,
        Method::PUT,
        &format!("{TEAMS}/{id}/members/{resource}"),
    ))
    .await;
    assert_eq!(status, 204);

    let (status, list) = send(by(&api, a, &plain, Method::GET, TEAMS)).await;
    assert_eq!((status, &list["canManage"]), (200, &json!(false)));
    let (status, _) = send(by(&api, a, &plain, Method::GET, &format!("{TEAMS}/{id}"))).await;
    assert_eq!(status, 200);
    for request in [
        by(&api, a, &plain, Method::POST, TEAMS).json(&json!({"nameTranslations": {"fr": "Run"}})),
        by(&api, a, &plain, Method::PUT, &format!("{TEAMS}/{id}"))
            .header("if-match", "*")
            .json(&json!({"nameTranslations": {"fr": "Run"}})),
        by(&api, a, &plain, Method::DELETE, &format!("{TEAMS}/{id}")).header("if-match", "*"),
        by(
            &api,
            a,
            &plain,
            Method::PUT,
            &format!("{TEAMS}/{id}/members/{resource}"),
        ),
        by(
            &api,
            a,
            &plain,
            Method::DELETE,
            &format!("{TEAMS}/{id}/members/{resource}"),
        ),
    ] {
        let (status, problem) = send(request).await;
        assert_eq!((status, &problem["code"]), (403, &json!("forbidden")));
    }
}

#[tokio::test]
async fn a_tenant_never_sees_nor_fills_the_teams_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let of_alpha = create_team(&api, a, json!({"nameTranslations": {"fr": "Plateforme"}})).await;
    let alpha_id = of_alpha["id"].as_str().unwrap();
    let alpha_resource = create_resource(&api, a, "Alice").await;
    let of_beta = create_team(&api, b, json!({"nameTranslations": {"fr": "Run"}})).await;
    let beta_id = of_beta["id"].as_str().unwrap();
    let beta_resource = create_resource(&api, b, "Bob").await;

    let (_, list) = send(admin(&api, b, Method::GET, TEAMS)).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (status, _) = send(admin(&api, b, Method::GET, &format!("{TEAMS}/{alpha_id}"))).await;
    assert_eq!(status, 404);
    assert_eq!(add(&api, b, alpha_id, &beta_resource).await, 404);
    assert_eq!(add(&api, b, beta_id, &alpha_resource).await, 404);
    assert!(
        resource_names(&api, b, &format!("?team={alpha_id}"))
            .await
            .is_empty()
    );
}
