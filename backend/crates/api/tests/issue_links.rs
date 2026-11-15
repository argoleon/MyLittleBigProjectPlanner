//! Integration tests of links between tickets, parents and the ticket
//! search (L05-US03) on real PostgreSQL and Valkey: a link across projects
//! read one way from each side, refused to itself or twice, removed; links
//! to tickets out of sight hidden; edit needed on both tickets; sub-tasks
//! listed and parents changed with the type hierarchy; search by key,
//! former key or summary; tenant isolation.

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

fn field_codes(body: &Value) -> Vec<(String, String)> {
    body["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no errors in {body}"))
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("accept-language", "en")
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

fn by<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, TenantRole::Member).await.account_id;
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

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: &str, label: &str, account: Uuid) {
    let role: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
            .bind(tenant.id)
            .bind(label)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let path = format!("/api/v1/nodes/{node}/grants");
    let (status, granted) =
        send(admin(api, tenant, Method::POST, &path).json(&json!({"roleId": role, "accountId": account})))
            .await;
    assert_eq!(status, 201, "{granted}");
}

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, code: &str) -> String {
    let body = json!({"type": "project", "name": name, "key": code});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

async fn confidential(api: &TestApi, tenant: &SeedTenant, node: &str) {
    let (status, body) = send(
        admin(
            api,
            tenant,
            Method::PUT,
            &format!("/api/v1/nodes/{node}/confidentiality"),
        )
        .header("if-match", "*")
        .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

async fn type_id(api: &TestApi, tenant: &SeedTenant, code: &str, type_key: &str) -> Value {
    let form = get(api, tenant, &format!("/api/v1/projects/{code}/issue-form")).await;
    by(&form["types"], "key", type_key)["id"].clone()
}

async fn create(
    api: &TestApi,
    tenant: &SeedTenant,
    code: &str,
    type_key: &str,
    summary: &str,
    parent: Option<&Value>,
) -> Value {
    let body =
        json!({"typeId": type_id(api, tenant, code, type_key).await, "summary": summary, "parentId": parent});
    let (status, created) = send(
        admin(
            api,
            tenant,
            Method::POST,
            &format!("/api/v1/projects/{code}/issues"),
        )
        .json(&body),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    created
}

fn link(api: &TestApi, tenant: &SeedTenant, from: &str, relation: &str, to: &str) -> reqwest::RequestBuilder {
    admin(api, tenant, Method::POST, &format!("/api/v1/issues/{from}/links"))
        .json(&json!({"relation": relation, "issue": to}))
}

/// `(relation, other key)` of each link of `key`, as the admin reads them.
async fn links_of(api: &TestApi, tenant: &SeedTenant, key: &str) -> Vec<(String, String)> {
    get(api, tenant, &format!("/api/v1/issues/{key}?expand=links")).await["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["relation"].as_str().unwrap().to_owned(),
                l["issue"]["key"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

async fn events(db: &TestDb, tenant: &SeedTenant, kind: &str) -> Vec<Value> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let events: Vec<Value> =
        infra::sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
            .bind(kind)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    events
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

#[tokio::test]
async fn tickets_are_linked_across_projects_each_side_reading_its_way() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    project(&api, t, "Identity platform", "PLAT").await;
    project(&api, t, "Customer relations", "CRM").await;
    create(&api, t, "PLAT", "story", "Migrate to OIDC", None).await;
    create(&api, t, "CRM", "story", "Customer portal forms", None).await;

    // PLAT-1 blocks CRM-1: CRM-1 is blocked by PLAT-1.
    let (status, blocks) = send(link(&api, t, "PLAT-1", "blocks", "CRM-1")).await;
    assert_eq!(status, 201, "{blocks}");
    assert_eq!(
        (&blocks["relation"], &blocks["issue"]["key"]),
        (&json!("blocks"), &json!("CRM-1"))
    );
    assert_eq!(blocks["issue"]["project"]["key"], "CRM");
    assert_eq!(blocks["issue"]["status"]["category"], "todo");
    assert_eq!(links_of(&api, t, "PLAT-1").await, pairs(&[("blocks", "CRM-1")]));
    assert_eq!(
        links_of(&api, t, "CRM-1").await,
        pairs(&[("blocked_by", "PLAT-1")])
    );
    // Without `expand`, no links.
    assert!(get(&api, t, "/api/v1/issues/PLAT-1").await.get("links").is_none());

    // Asked from the other side: the same link, refused. Itself, or unknown: refused.
    let (status, refused) = send(link(&api, t, "CRM-1", "blocked_by", "PLAT-1")).await;
    assert_problem(status, &refused, 409, "issue_link.exists");
    for (to, code) in [
        ("PLAT-1", "self"),
        ("PLAT-99", "not_found"),
        ("nonsense", "not_found"),
    ] {
        let (status, refused) = send(link(&api, t, "PLAT-1", "relates_to", to)).await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(field_codes(&refused), pairs(&[("issue", code)]), "{to}");
    }
    // "Relates to", the same both ways; "depends on" from CRM-1, another link.
    let (status, _) = send(link(&api, t, "CRM-1", "relates_to", "PLAT-1")).await;
    assert_eq!(status, 201);
    let (status, refused) = send(link(&api, t, "PLAT-1", "relates_to", "CRM-1")).await;
    assert_problem(status, &refused, 409, "issue_link.exists");
    let (status, _) = send(link(&api, t, "CRM-1", "depends_on", "PLAT-1")).await;
    assert_eq!(status, 201);
    assert_eq!(
        links_of(&api, t, "PLAT-1").await,
        pairs(&[
            ("blocks", "CRM-1"),
            ("relates_to", "CRM-1"),
            ("required_by", "CRM-1")
        ])
    );
    let linked = events(&db, t, "issue.linked").await;
    assert_eq!(linked.len(), 3);
    assert_eq!(
        (
            &linked[0]["linkType"],
            &linked[0]["sourceKey"],
            &linked[0]["targetKey"]
        ),
        (&json!("blocks"), &json!("PLAT-1"), &json!("CRM-1"))
    );

    // Removed from either side; then gone.
    let path = format!("/api/v1/issues/CRM-1/links/{}", blocks["id"].as_str().unwrap());
    let response = admin(&api, t, Method::DELETE, &path).send().await.unwrap();
    assert_eq!(response.status(), 204);
    let (status, gone) = send(admin(&api, t, Method::DELETE, &path)).await;
    assert_problem(status, &gone, 404, "not_found");
    assert_eq!(
        links_of(&api, t, "CRM-1").await,
        pairs(&[("relates_to", "PLAT-1"), ("depends_on", "PLAT-1")])
    );
    assert_eq!(events(&db, t, "issue.unlinked").await[0]["linkId"], blocks["id"]);
}

#[tokio::test]
async fn links_need_edit_on_both_tickets_and_hide_tickets_out_of_sight() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let ops = project(&api, a, "Operations", "OPS").await;
    let secret = project(&api, a, "Mergers", "MRG").await;
    create(&api, a, "PLAT", "story", "Mine", None).await;
    create(&api, a, "OPS", "story", "Theirs", None).await;
    create(&api, a, "MRG", "story", "Secret", None).await;
    confidential(&api, a, &secret).await;
    let (status, _) = send(link(&api, a, "PLAT-1", "blocks", "MRG-1")).await;
    assert_eq!(status, 201);
    let (hugo, contributor) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Contributor", hugo).await;
    grant(&db, &api, a, &ops, "Reader", hugo).await;
    let post = |from: &str, to: &str| {
        as_member(
            &api,
            a,
            &contributor,
            Method::POST,
            &format!("/api/v1/issues/{from}/links"),
        )
        .json(&json!({"relation": "relates_to", "issue": to}))
    };

    // The link to the confidential project does not show, nor can one be made.
    let (status, read) = send(as_member(
        &api,
        a,
        &contributor,
        Method::GET,
        "/api/v1/issues/PLAT-1?expand=links",
    ))
    .await;
    assert_eq!((status, &read["links"]), (200, &json!([])));
    let (status, refused) = send(post("PLAT-1", "MRG-1")).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(field_codes(&refused), pairs(&[("issue", "not_found")]));
    // Seen but not editable: refused, from either side.
    for (from, to) in [("PLAT-1", "OPS-1"), ("OPS-1", "PLAT-1")] {
        let (status, refused) = send(post(from, to)).await;
        assert_problem(status, &refused, 403, "permission.denied");
    }
    grant(&db, &api, a, &ops, "Contributor", hugo).await;
    let (status, made) = send(post("OPS-1", "PLAT-1")).await;
    assert_eq!(status, 201, "{made}");

    // Another tenant: neither its tickets nor links to them.
    project(&api, b, "Identity platform", "PLAT").await;
    create(&api, b, "PLAT", "story", "Elsewhere", None).await;
    let (status, hidden) = send(admin(&api, b, Method::GET, "/api/v1/issues/OPS-1?expand=links")).await;
    assert_problem(status, &hidden, 404, "not_found");
    let (status, refused) = send(link(&api, b, "PLAT-1", "blocks", "OPS-1")).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(links_of(&api, b, "PLAT-1").await, pairs(&[]));
}

#[tokio::test]
async fn sub_tasks_are_listed_and_parents_changed_within_the_type_hierarchy() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    project(&api, t, "Customer relations", "CRM").await;
    let epic = create(&api, t, "PLAT", "epic", "Unified sign-in", None).await;
    let story = create(&api, t, "PLAT", "story", "OIDC", Some(&epic["id"])).await;
    create(&api, t, "PLAT", "subtask", "Client", Some(&story["id"])).await;
    create(&api, t, "PLAT", "subtask", "Groups", Some(&story["id"])).await;
    let other_epic = create(&api, t, "PLAT", "epic", "Security", None).await;
    let crm_epic = create(&api, t, "CRM", "epic", "Elsewhere", None).await;

    let read = get(&api, t, "/api/v1/issues/PLAT-2?expand=children,parent").await;
    let children: Vec<(&Value, &Value)> = read["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (&c["key"], &c["status"]["label"]))
        .collect();
    assert_eq!(
        children,
        [
            (&json!("PLAT-3"), &json!("To do")),
            (&json!("PLAT-4"), &json!("To do"))
        ]
    );
    assert_eq!(read["parent"]["key"], "PLAT-1");
    let (status, unknown) = send(admin(
        &api,
        t,
        Method::GET,
        "/api/v1/issues/PLAT-2?expand=history",
    ))
    .await;
    assert_problem(status, &unknown, 400, "invalid_query");

    let put = |key: &str, version: Option<&str>, parent: &Value| {
        let request = admin(&api, t, Method::PUT, &format!("/api/v1/issues/{key}/parent"))
            .json(&json!({"parentId": parent}));
        match version {
            Some(version) => request.header("if-match", version.to_owned()),
            None => request,
        }
    };
    let (status, refused) = send(put("PLAT-2", None, &Value::Null)).await;
    assert_problem(status, &refused, 428, "precondition_required");
    // The story under another epic, then at the top.
    let (status, moved) = send(put("PLAT-2", Some("\"1\""), &other_epic["id"])).await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(
        (&moved["parentId"], &moved["parent"]["key"], &moved["version"]),
        (&other_epic["id"], &json!("PLAT-5"), &json!(2))
    );
    let (status, stale) = send(put("PLAT-2", Some("\"1\""), &Value::Null)).await;
    assert_problem(status, &stale, 412, "precondition_failed");
    let (status, top) = send(put("PLAT-2", Some("\"2\""), &Value::Null)).await;
    assert_eq!((status, &top["parentId"]), (200, &Value::Null));
    let changes: Vec<Value> = events(&db, t, "issue.updated")
        .await
        .iter()
        .map(|e| e["changes"][0].clone())
        .collect();
    assert_eq!(
        changes,
        [
            json!({"field": "parentId", "old": epic["id"], "new": other_epic["id"]}),
            json!({"field": "parentId", "old": other_epic["id"], "new": null}),
        ]
    );

    // What the hierarchy refuses.
    for (key, parent, code) in [
        ("PLAT-3", Value::Null, "required"),
        ("PLAT-3", epic["id"].clone(), "not_allowed"),
        ("PLAT-2", story["id"].clone(), "cycle"),
        ("PLAT-2", crm_epic["id"].clone(), "other_project"),
        ("PLAT-2", json!(Uuid::nil()), "not_found"),
    ] {
        let (status, refused) = send(put(key, Some("*"), &parent)).await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(
            field_codes(&refused),
            pairs(&[("parentId", code)]),
            "{key} {parent}"
        );
    }

    // A reader changes nothing.
    let (lea, reader) = member(&db, &api, t).await;
    grant(&db, &api, t, &plat, "Reader", lea).await;
    let (status, refused) = send(
        as_member(&api, t, &reader, Method::PUT, "/api/v1/issues/PLAT-2/parent")
            .header("if-match", "*")
            .json(&json!({"parentId": epic["id"]})),
    )
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
}

#[tokio::test]
async fn tickets_are_found_by_key_former_key_or_summary_in_projects_in_sight() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    project(&api, a, "Customer relations", "CRM").await;
    let secret = project(&api, a, "Mergers", "MRG").await;
    create(&api, a, "PLAT", "story", "Migrate to OIDC", None).await;
    create(&api, a, "PLAT", "story", "Archive mailboxes", None).await;
    create(&api, a, "CRM", "story", "OIDC client for the portal", None).await;
    create(&api, a, "MRG", "story", "OIDC for the acquired company", None).await;
    confidential(&api, a, &secret).await;
    let keys = |found: &Value| -> Vec<String> {
        found
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_owned())
            .collect()
    };

    // Most recently updated first; any case; only in one project if asked.
    let found = get(&api, a, "/api/v1/issues?search=oidc").await;
    assert_eq!(keys(&found), ["MRG-1", "CRM-1", "PLAT-1"]);
    assert_eq!(found[1]["type"]["key"], "story");
    assert_eq!(
        keys(&get(&api, a, "/api/v1/issues?search=OIDC&projectKey=CRM").await),
        ["CRM-1"]
    );
    assert_eq!(keys(&get(&api, a, "/api/v1/issues").await), Vec::<String>::new());
    // A key first, a former key too.
    assert_eq!(
        keys(&get(&api, a, "/api/v1/issues?search=plat-2").await),
        ["PLAT-2"]
    );
    let (status, _) = send(
        admin(&api, a, Method::POST, "/api/v1/issues/PLAT-2/move")
            .header("if-match", "*")
            .json(&json!({"projectKey": "CRM"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        keys(&get(&api, a, "/api/v1/issues?search=PLAT-2").await),
        ["CRM-2"]
    );

    // A member sees what their projects hold, not the confidential one.
    let (hugo, contributor) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Contributor", hugo).await;
    let (status, found) = send(as_member(
        &api,
        a,
        &contributor,
        Method::GET,
        "/api/v1/issues?search=oidc",
    ))
    .await;
    assert_eq!(status, 200);
    assert!(!keys(&found).contains(&"MRG-1".to_owned()), "{found}");
    assert!(keys(&found).contains(&"PLAT-1".to_owned()), "{found}");
    let (status, hidden) = send(as_member(
        &api,
        a,
        &contributor,
        Method::GET,
        "/api/v1/issues?search=x&projectKey=MRG",
    ))
    .await;
    assert_problem(status, &hidden, 404, "not_found");

    // Another tenant finds none of them.
    assert_eq!(
        keys(&get(&api, b, "/api/v1/issues?search=oidc").await),
        Vec::<String>::new()
    );
}
