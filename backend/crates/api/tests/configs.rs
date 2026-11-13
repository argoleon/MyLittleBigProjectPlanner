//! Integration tests of project configurations (L04-US01) on real
//! PostgreSQL and Valkey: creation and the default configuration, sharing
//! and the projects of a configuration, the draft and its publication after
//! the impact, detachment by duplication, deletion, who may do what, and
//! tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

type Session = (String, String);

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

/// A request of the seeded tenant admin of `tenant`.
fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path)
}

/// A request of the member whose session is `session`.
fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

/// A new member of `tenant` with `role`, with a session opened for them.
async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant, role: TenantRole) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, role).await.account_id;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account_id, tenant.id)
    .await
    .unwrap();
    (account_id, (opened.token, opened.session.csrf_token))
}

/// Grants the default role labelled `label` (English) to `account` on
/// `node`, as the tenant admin.
async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: &str, label: &str, account: Uuid) {
    let role: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
            .bind(tenant.id)
            .bind(label)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let path = format!("/api/v1/nodes/{node}/grants");
    let body = json!({"roleId": role, "accountId": account});
    let (status, granted) = send(admin(api, tenant, Method::POST, &path).json(&body)).await;
    assert_eq!(status, 201, "{granted}");
}

/// Creates a node as the tenant admin; its id.
async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// Creates a configuration as the tenant admin.
async fn config(api: &TestApi, tenant: &SeedTenant, fr: &str, en: &str) -> Value {
    let body = json!({"nameTranslations": {"fr": fr, "en": en}});
    let (status, config) =
        send(admin(api, tenant, Method::POST, "/api/v1/project-configs").json(&body)).await;
    assert_eq!(status, 201, "{config}");
    config
}

/// Moves `project` to `config`, as the tenant admin.
async fn assign(api: &TestApi, tenant: &SeedTenant, project: &str, config: &Value) -> (u16, Value) {
    let path = format!("/api/v1/nodes/{project}/configuration");
    send(admin(api, tenant, Method::PUT, &path).json(&json!({"configId": config["id"]}))).await
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap_or("(veiled)"))
        .collect()
}

#[tokio::test]
async fn a_configuration_is_created_shared_and_published_after_its_impact() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;

    // The default configuration comes first, and new projects get it.
    let (status, page) = send(admin(&api, t, Method::GET, "/api/v1/project-configs")).await;
    assert_eq!(status, 200, "{page}");
    let default = page["items"][0].clone();
    assert_eq!(default["isDefault"], true);
    assert_eq!(default["name"], "Configuration par défaut");
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let (status, used) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{plat}/configuration"),
    ))
    .await;
    assert_eq!(status, 200, "{used}");
    assert_eq!((&used["id"], &used["isDefault"]), (&default["id"], &json!(true)));

    // Created with a published version 1, idempotently.
    let body = json!({"nameTranslations": {"fr": "Développement logiciel", "en": "Software development"}});
    let create = || {
        admin(&api, t, Method::POST, "/api/v1/project-configs")
            .header("idempotency-key", "config-software")
            .header("accept-language", "en")
            .json(&body)
    };
    let (status, software) = send(create()).await;
    assert_eq!(status, 201, "{software}");
    assert_eq!(software["name"], "Software development");
    assert_eq!(software["published"]["number"], 1);
    assert_eq!(software["draft"], Value::Null);
    assert_eq!(
        (&software["projectCount"], &software["isDefault"]),
        (&json!(0), &json!(false))
    );
    let (status, replayed) = send(create()).await;
    assert_eq!((status, &replayed["id"]), (201, &software["id"]));
    let id = software["id"].as_str().unwrap().to_owned();

    // Shared by six projects, listed by name in its Projects tab.
    let keys = ["PLAT", "CYBP", "PORT", "DWH", "ERP", "CRM"];
    let mut projects = vec![plat.clone()];
    for key in &keys[1..] {
        let name = format!("Project {key}");
        projects.push(node(&api, t, json!({"type": "project", "name": name, "key": key})).await);
    }
    for project in &projects {
        let (status, moved) = assign(&api, t, project, &software).await;
        assert_eq!(status, 200, "{moved}");
        assert_eq!(
            (&moved["id"], &moved["isDefault"]),
            (&software["id"], &json!(false))
        );
    }
    let (status, read) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}"),
    ))
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["projectCount"], 6);
    let (_, list) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}/projects"),
    ))
    .await;
    assert_eq!(
        names(&list["items"]),
        [
            "Identity platform",
            "Project CRM",
            "Project CYBP",
            "Project DWH",
            "Project ERP",
            "Project PORT"
        ]
    );
    assert_eq!(list["items"][0]["key"], "PLAT");

    // No draft yet: no impact, nothing to publish.
    let impact_path = format!("/api/v1/project-configs/{id}/publish-impact");
    let (status, none) = send(admin(&api, t, Method::GET, &impact_path)).await;
    assert_problem(status, &none, 409, "config.no_draft");

    // A draft, shared by its admins; then the projects it will affect.
    let draft_path = format!("/api/v1/project-configs/{id}/draft");
    let (status, drafted) = send(admin(&api, t, Method::POST, &draft_path)).await;
    assert_eq!(status, 200, "{drafted}");
    assert_eq!(drafted["draft"]["number"], 2);
    assert_eq!(drafted["draft"]["changeCount"], 0);
    let (status, impact) = send(admin(&api, t, Method::GET, &impact_path)).await;
    assert_eq!(status, 200, "{impact}");
    assert_eq!(impact["projects"].as_array().unwrap().len(), 6);
    assert_eq!(impact["needsMigration"], false);
    assert_eq!(impact["version"], drafted["version"]);

    // Published only with the version the impact was read on.
    let publish_path = format!("/api/v1/project-configs/{id}/publish");
    let (status, missing) = send(admin(&api, t, Method::POST, &publish_path)).await;
    assert_problem(status, &missing, 428, "precondition_required");
    let (status, stale) = send(
        admin(&api, t, Method::POST, &publish_path)
            .header("if-match", format!("\"{}\"", software["version"])),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let (status, published) = send(
        admin(&api, t, Method::POST, &publish_path).header("if-match", format!("\"{}\"", impact["version"])),
    )
    .await;
    assert_eq!(status, 200, "{published}");
    assert_eq!(published["published"]["number"], 2);
    assert_eq!(published["draft"], Value::Null);
    let (status, again) = send(admin(&api, t, Method::POST, &publish_path).header("if-match", "*")).await;
    assert_problem(status, &again, 409, "config.no_draft");

    // A draft discarded leaves the published version.
    send(admin(&api, t, Method::POST, &draft_path)).await;
    let (status, discarded) = send(admin(&api, t, Method::DELETE, &draft_path).header("if-match", "*")).await;
    assert_eq!(status, 200, "{discarded}");
    assert_eq!(
        (&discarded["draft"], &discarded["published"]["number"]),
        (&Value::Null, &json!(2))
    );

    // Renamed at once, with its version; names stay unique.
    let rename = |name: Value, version: &Value| {
        admin(&api, t, Method::PUT, &format!("/api/v1/project-configs/{id}"))
            .header("if-match", format!("\"{version}\""))
            .json(&json!({"nameTranslations": name}))
    };
    let (status, taken) = send(rename(
        json!({"fr": "configuration PAR DÉFAUT"}),
        &discarded["version"],
    ))
    .await;
    assert_problem(status, &taken, 409, "config.name_taken");
    assert_eq!(taken["errors"][0]["field"], "nameTranslations");
    let (status, renamed) = send(rename(
        json!({"fr": "Logiciel", "en": "Software"}),
        &discarded["version"],
    ))
    .await;
    assert_eq!(status, 200, "{renamed}");
    let (status, invalid) = send(rename(json!({"de": "Software"}), &renamed["version"])).await;
    assert_problem(status, &invalid, 400, "validation_failed");
}

#[tokio::test]
async fn a_project_detaches_from_a_shared_configuration_by_duplicating_it() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let software = config(&api, t, "Développement logiciel", "Software development").await;
    let id = software["id"].as_str().unwrap();
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let cyber = node(
        &api,
        t,
        json!({"type": "project", "name": "Cyber permissions", "key": "CYBP"}),
    )
    .await;
    let other = node(
        &api,
        t,
        json!({"type": "project", "name": "Customer portal", "key": "PORT"}),
    )
    .await;
    for project in [&plat, &cyber] {
        assign(&api, t, project, &software).await;
    }

    let duplicate = |body: Value| {
        admin(
            &api,
            t,
            Method::POST,
            &format!("/api/v1/project-configs/{id}/duplicate"),
        )
        .json(&body)
    };
    let named = |project: Option<&str>| {
        json!({
            "nameTranslations": {"fr": "Développement logiciel (PLAT)", "en": "Software development (PLAT)"},
            "projectId": project,
        })
    };

    // A project that uses another configuration does not detach from this one.
    let (status, refused) = send(duplicate(named(Some(&other)))).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        refused["errors"][0],
        json!({"field": "projectId", "code": "not_using"})
    );

    // Detached: its own copy; changes to the original no longer reach it.
    let (status, copy) = send(duplicate(named(Some(&plat)))).await;
    assert_eq!(status, 201, "{copy}");
    assert_eq!(
        (&copy["projectCount"], &copy["published"]["number"]),
        (&json!(1), &json!(1))
    );
    let (_, used) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{plat}/configuration"),
    ))
    .await;
    assert_eq!(used["id"], copy["id"]);
    let (_, original) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}"),
    ))
    .await;
    assert_eq!(original["projectCount"], 1);
    send(admin(
        &api,
        t,
        Method::POST,
        &format!("/api/v1/project-configs/{id}/draft"),
    ))
    .await;
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::POST,
            &format!("/api/v1/project-configs/{id}/publish"),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_eq!(status, 200);
    let copy_id = copy["id"].as_str().unwrap();
    let (_, copy_now) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{copy_id}"),
    ))
    .await;
    assert_eq!(copy_now["published"]["number"], 1);

    // A plain duplicate; an archived project stays where it is.
    let (status, plain) = send(duplicate(json!({"nameTranslations": {"en": "Software copy"}}))).await;
    assert_eq!(status, 201, "{plain}");
    assert_eq!(plain["projectCount"], 0);
    let (status, _) = send(
        admin(&api, t, Method::PUT, &format!("/api/v1/nodes/{cyber}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, archived) = send(duplicate(json!({
        "nameTranslations": {"en": "Software (CYBP)"},
        "projectId": cyber,
    })))
    .await;
    assert_problem(status, &archived, 409, "node.archived");
    let (status, frozen) = assign(&api, t, &cyber, &plain).await;
    assert_problem(status, &frozen, 409, "node.archived");
    // Still counted, and listed as archived.
    let (_, list) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}/projects"),
    ))
    .await;
    assert_eq!(
        list["items"],
        json!([{"id": cyber, "name": "Cyber permissions", "key": "CYBP", "archived": true}])
    );
}

#[tokio::test]
async fn the_default_and_used_configurations_are_not_deleted() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let (_, page) = send(admin(&api, t, Method::GET, "/api/v1/project-configs")).await;
    let default = page["items"][0]["id"].as_str().unwrap().to_owned();
    let delete = |id: &str| {
        admin(&api, t, Method::DELETE, &format!("/api/v1/project-configs/{id}")).header("if-match", "*")
    };
    let (status, refused) = send(delete(&default)).await;
    assert_problem(status, &refused, 409, "config.default");

    let software = config(&api, t, "Développement", "Software").await;
    let id = software["id"].as_str().unwrap();
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    assign(&api, t, &plat, &software).await;
    let (status, in_use) = send(delete(id)).await;
    assert_problem(status, &in_use, 409, "config.in_use");

    // Moved back to the default: it can go.
    let (status, _) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("/api/v1/nodes/{plat}/configuration"),
        )
        .json(&json!({"configId": default})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(delete(id)).await;
    assert_eq!(status, 204);
    let (status, gone) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/project-configs/{id}"),
    ))
    .await;
    assert_problem(status, &gone, 404, "not_found");

    // Only projects have a configuration; an unknown one is refused.
    let program = node(&api, t, json!({"type": "program", "name": "IT Modernization"})).await;
    let (status, none) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{program}/configuration"),
    ))
    .await;
    assert_problem(status, &none, 404, "not_found");
    let (status, unknown) = send(
        admin(
            &api,
            t,
            Method::PUT,
            &format!("/api/v1/nodes/{plat}/configuration"),
        )
        .json(&json!({"configId": Uuid::from_u128(7)})),
    )
    .await;
    assert_problem(status, &unknown, 400, "validation_failed");
    assert_eq!(
        unknown["errors"][0],
        json!({"field": "configId", "code": "not_found"})
    );
}

#[tokio::test]
async fn configuration_admins_manage_configurations_and_the_others_get_403() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let software = config(&api, t, "Développement", "Software").await;
    let id = software["id"].as_str().unwrap().to_owned();
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let secret = node(
        &api,
        t,
        json!({"type": "project", "name": "Merger", "key": "MRGR"}),
    )
    .await;
    for project in [&plat, &secret] {
        assign(&api, t, project, &software).await;
    }

    // A plain member, even a reader of the project.
    let (reader, session) = member(&db, &api, t, TenantRole::Member).await;
    grant(&db, &api, t, &plat, "Reader", reader).await;
    let refused = [
        (Method::GET, "/api/v1/project-configs".to_owned(), None),
        (
            Method::POST,
            "/api/v1/project-configs".to_owned(),
            Some(json!({"nameTranslations": {"en": "Mine"}})),
        ),
        (Method::GET, format!("/api/v1/project-configs/{id}"), None),
        (
            Method::GET,
            format!("/api/v1/project-configs/{id}/projects"),
            None,
        ),
        (Method::POST, format!("/api/v1/project-configs/{id}/draft"), None),
        (
            Method::GET,
            format!("/api/v1/project-configs/{id}/publish-impact"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/project-configs/{id}/publish"),
            None,
        ),
        (Method::DELETE, format!("/api/v1/project-configs/{id}"), None),
        (
            Method::POST,
            format!("/api/v1/project-configs/{id}/duplicate"),
            Some(json!({"nameTranslations": {"en": "Mine"}})),
        ),
        (
            Method::PUT,
            format!("/api/v1/nodes/{plat}/configuration"),
            Some(json!({"configId": id})),
        ),
    ];
    for (method, path, body) in refused {
        let mut request = as_member(&api, t, &session, method.clone(), &path).header("if-match", "*");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let (status, problem) = send(request).await;
        assert_problem(status, &problem, 403, "forbidden");
    }
    // They still read which configuration their project uses, and only that.
    let (status, used) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/nodes/{plat}/configuration"),
    ))
    .await;
    assert_eq!(status, 200, "{used}");
    assert_eq!((&used["id"], &used["projectCount"]), (&software["id"], &json!(2)));
    let (status, hidden) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/nodes/{secret}/configuration"),
    ))
    .await;
    assert_problem(status, &hidden, 404, "not_found");

    // A configuration admin manages them; the projects they do not see come veiled.
    let (config_admin, session) = member(&db, &api, t, TenantRole::ConfigurationAdmin).await;
    grant(&db, &api, t, &plat, "Reader", config_admin).await;
    let (status, page) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        "/api/v1/project-configs",
    ))
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let (status, created) = send(
        as_member(&api, t, &session, Method::POST, "/api/v1/project-configs")
            .json(&json!({"nameTranslations": {"en": "Support"}})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let (status, list) = send(as_member(
        &api,
        t,
        &session,
        Method::GET,
        &format!("/api/v1/project-configs/{id}/projects"),
    ))
    .await;
    assert_eq!(status, 200, "{list}");
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "Identity platform");
    assert_eq!(items[1]["veiled"], true);
    assert!(items[1].get("name").is_none() && items[1].get("key").is_none());
    assert_ne!(items[1]["id"], json!(secret));
    let trail = list.to_string();
    assert!(!trail.contains("Merger") && !trail.contains("MRGR") && !trail.contains(&secret));
    // A project they do not see stays out of their reach.
    let (status, moved) = send(
        as_member(
            &api,
            t,
            &session,
            Method::PUT,
            &format!("/api/v1/nodes/{secret}/configuration"),
        )
        .json(&json!({"configId": created["id"]})),
    )
    .await;
    assert_problem(status, &moved, 404, "not_found");
    let (status, moved) = send(
        as_member(
            &api,
            t,
            &session,
            Method::PUT,
            &format!("/api/v1/nodes/{plat}/configuration"),
        )
        .json(&json!({"configId": created["id"]})),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
}

#[tokio::test]
async fn configurations_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let software = config(&api, a, "Développement", "Software").await;
    let id = software["id"].as_str().unwrap();
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    assign(&api, a, &plat, &software).await;
    let foreign = config(&api, b, "Étrangère", "Foreign").await;

    // Tenant B sees its own default and its own configuration only.
    let (_, page) = send(admin(&api, b, Method::GET, "/api/v1/project-configs")).await;
    let ids: Vec<&Value> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["id"])
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(!ids.contains(&&software["id"]));
    for (method, path) in [
        (Method::GET, format!("/api/v1/project-configs/{id}")),
        (Method::GET, format!("/api/v1/project-configs/{id}/projects")),
        (Method::POST, format!("/api/v1/project-configs/{id}/draft")),
        (Method::DELETE, format!("/api/v1/project-configs/{id}")),
        (Method::GET, format!("/api/v1/nodes/{plat}/configuration")),
    ] {
        let (status, problem) = send(admin(&api, b, method, &path).header("if-match", "*")).await;
        assert_problem(status, &problem, 404, "not_found");
    }
    // Tenant A cannot move its project to B's configuration.
    let (status, refused) = assign(&api, a, &plat, &foreign).await;
    assert_problem(status, &refused, 400, "validation_failed");
    let (_, used) = send(admin(
        &api,
        a,
        Method::GET,
        &format!("/api/v1/nodes/{plat}/configuration"),
    ))
    .await;
    assert_eq!(used["id"], software["id"]);
}
