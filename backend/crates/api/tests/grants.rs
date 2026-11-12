//! Integration tests of the grants of roles on nodes (L03-US03) on real
//! PostgreSQL and Valkey: inheritance by the descendants, union with several
//! parents, the effective access of a node, revocation applied to the next
//! request, who manages grants, and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

type Session = (String, String);

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

fn field_codes(body: &Value) -> Vec<(String, String)> {
    body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
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

/// A new member of `tenant`, with a session opened for them.
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

/// The default role labelled `label` (English) of `tenant`.
async fn role(db: &TestDb, tenant: &SeedTenant, label: &str) -> Uuid {
    infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
        .bind(tenant.id)
        .bind(label)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

/// Creates a node as the tenant admin; its id.
async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// Creates a group of `members` as the tenant admin; its id.
async fn group(api: &TestApi, tenant: &SeedTenant, name: &str, members: &[Uuid]) -> String {
    let (status, group) =
        send(admin(api, tenant, Method::POST, "/api/v1/tenant/groups").json(&json!({"name": name}))).await;
    assert_eq!(status, 201, "{group}");
    let id = group["id"].as_str().unwrap().to_owned();
    for account in members {
        let path = format!("/api/v1/tenant/groups/{id}/members/{account}");
        let res = admin(api, tenant, Method::PUT, &path).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 204);
    }
    id
}

/// Grants as the tenant admin; the grant.
async fn grant(api: &TestApi, tenant: &SeedTenant, node: &str, body: Value) -> Value {
    let path = format!("/api/v1/nodes/{node}/grants");
    let (status, granted) = send(
        admin(api, tenant, Method::POST, &path)
            .header("accept-language", "en")
            .json(&body),
    )
    .await;
    assert_eq!(status, 201, "{granted}");
    granted
}

/// The permissions of the member on `node`, or the status if they have none.
async fn permissions_of(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    node: &str,
) -> Result<Vec<String>, u16> {
    let path = format!("/api/v1/nodes/{node}/my-permissions");
    let (status, body) = send(as_member(api, tenant, session, Method::GET, &path)).await;
    if status != 200 {
        return Err(status);
    }
    Ok(body
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect())
}

/// Names of the nodes the member lists, sorted.
async fn listed(api: &TestApi, tenant: &SeedTenant, session: &Session) -> Vec<String> {
    let (status, page) = send(as_member(api, tenant, session, Method::GET, "/api/v1/nodes")).await;
    assert_eq!(status, 200, "{page}");
    let mut names: Vec<String> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

const PROJECT_MANAGER: [&str; 8] = [
    "view",
    "create_ticket",
    "edit",
    "comment",
    "log_time",
    "assign_resources",
    "force_overload",
    "manage_sprints",
];

#[tokio::test]
async fn a_group_role_on_a_portfolio_reaches_the_projects_below_until_it_is_revoked() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let digital = node(
        &api,
        t,
        json!({"type": "portfolio", "name": "Digital transformation"}),
    )
    .await;
    let program = node(
        &api,
        t,
        json!({"type": "program", "name": "IT Modernization", "parentId": digital}),
    )
    .await;
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": program}),
    )
    .await;
    let (lucas, session) = member(&db, &api, t).await;
    let get = |path: String| as_member(&api, t, &session, Method::GET, &path);

    // Without any grant: the nodes do not exist for them.
    let (status, _) = send(get(format!("/api/v1/nodes/{plat}"))).await;
    assert_eq!(status, 404);
    assert_eq!(permissions_of(&api, t, &session, &plat).await, Err(404));
    assert!(listed(&api, t, &session).await.is_empty());

    // Group "Managers" is project manager on the portfolio.
    let managers = group(&api, t, "Managers", &[lucas]).await;
    let manager = role(&db, t, "Project manager").await;
    let granted = grant(&api, t, &digital, json!({"roleId": manager, "groupId": managers})).await;
    assert_eq!(
        granted["holder"],
        json!({"kind": "group", "id": managers, "name": "Managers"})
    );
    assert_eq!(
        (
            &granted["role"]["label"],
            &granted["origin"]["name"],
            &granted["direct"]
        ),
        (
            &json!("Project manager"),
            &json!("Digital transformation"),
            &json!(true)
        )
    );

    // A member of the group opens a project under it with the role's permissions.
    let (status, seen) = send(get(format!("/api/v1/nodes/{plat}"))).await;
    assert_eq!((status, &seen["name"]), (200, &json!("Identity platform")));
    assert_eq!(
        permissions_of(&api, t, &session, &plat).await.unwrap(),
        PROJECT_MANAGER
    );
    assert_eq!(
        listed(&api, t, &session).await,
        ["Digital transformation", "IT Modernization", "Identity platform"]
    );
    // Not more: they neither administer the project nor manage its members.
    let (status, problem) = send(
        as_member(&api, t, &session, Method::PUT, &format!("/api/v1/nodes/{plat}"))
            .header("if-match", "*")
            .json(&json!({"name": "Renamed"})),
    )
    .await;
    assert_problem(status, &problem, 403, "forbidden");
    let (status, problem) = send(get(format!("/api/v1/nodes/{plat}/access"))).await;
    assert_problem(status, &problem, 403, "permission.denied");
    assert_eq!(problem["detail"], "missing permission: manage_members");

    // The grant removed on the portfolio: gone at their next call.
    let grant_path = format!(
        "/api/v1/nodes/{digital}/grants/{}",
        granted["grantId"].as_str().unwrap()
    );
    let res = admin(&api, t, Method::DELETE, &grant_path).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 204);
    let (status, _) = send(get(format!("/api/v1/nodes/{plat}"))).await;
    assert_eq!(status, 404);
    assert!(listed(&api, t, &session).await.is_empty());
    let (status, _) = send(admin(&api, t, Method::DELETE, &grant_path)).await;
    assert_eq!(status, 404, "already revoked");

    // Granted again; leaving the group ends it as well.
    grant(&api, t, &digital, json!({"roleId": manager, "groupId": managers})).await;
    assert_eq!(listed(&api, t, &session).await.len(), 3);
    let membership = format!("/api/v1/tenant/groups/{managers}/members/{lucas}");
    let res = admin(&api, t, Method::DELETE, &membership).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert!(listed(&api, t, &session).await.is_empty());

    // Back in the group, until the group itself is deleted with its grants.
    let res = admin(&api, t, Method::PUT, &membership).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert_eq!(
        permissions_of(&api, t, &session, &plat).await.unwrap(),
        PROJECT_MANAGER
    );
    let res = admin(
        &api,
        t,
        Method::DELETE,
        &format!("/api/v1/tenant/groups/{managers}"),
    )
    .header("if-match", "*")
    .send()
    .await
    .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert_eq!(permissions_of(&api, t, &session, &plat).await, Err(404));
    let (_, access) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{digital}/access"),
    ))
    .await;
    assert_eq!(access, json!([]));
    // The role is free again: no grant uses it.
    let (_, read) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/tenant/roles/{manager}"),
    ))
    .await;
    assert_eq!(read["grantCount"], 0);
}

#[tokio::test]
async fn two_parents_give_the_union_and_the_access_tells_where_each_role_comes_from() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let digital = node(&api, t, json!({"type": "portfolio", "name": "Digital"})).await;
    let modernization = node(
        &api,
        t,
        json!({"type": "program", "name": "Modernization", "parentId": digital}),
    )
    .await;
    let compliance = node(
        &api,
        t,
        json!({"type": "program", "name": "Compliance", "parentId": digital}),
    )
    .await;
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": modernization}),
    )
    .await;
    let link = format!("/api/v1/nodes/{plat}/parents/{compliance}");
    let (status, _) = send(admin(&api, t, Method::PUT, &link).json(&json!({}))).await;
    assert_eq!(status, 201);
    let (lucas, session) = member(&db, &api, t).await;
    let (ines, _) = member(&db, &api, t).await;
    let as_lucas = |method: Method, path: String| as_member(&api, t, &session, method, &path);

    // Two parents granting different roles to two of Lucas's groups, and a
    // role of his own on the project.
    let platform_team = group(&api, t, "Platform team", &[lucas]).await;
    let compliance_team = group(&api, t, "Compliance team", &[lucas]).await;
    let (reader, contributor, node_admin) = (
        role(&db, t, "Reader").await,
        role(&db, t, "Contributor").await,
        role(&db, t, "Node admin").await,
    );
    let (status, custom) = send(admin(&api, t, Method::POST, "/api/v1/tenant/roles").json(&json!({
        "labelTranslations": {"fr": "Gestionnaire des membres", "en": "Member manager"},
        "permissions": ["view", "manage_members"],
    })))
    .await;
    assert_eq!(status, 201, "{custom}");
    let member_manager = custom["id"].as_str().unwrap().to_owned();
    grant(
        &api,
        t,
        &modernization,
        json!({"roleId": contributor, "groupId": platform_team}),
    )
    .await;
    grant(
        &api,
        t,
        &compliance,
        json!({"roleId": reader, "groupId": compliance_team}),
    )
    .await;
    let own = grant(
        &api,
        t,
        &plat,
        json!({"roleId": member_manager, "accountId": lucas}),
    )
    .await;

    // On the project: the union of both parents' roles and the direct one.
    assert_eq!(
        permissions_of(&api, t, &session, &plat).await.unwrap(),
        [
            "view",
            "create_ticket",
            "edit",
            "comment",
            "log_time",
            "manage_members"
        ]
    );
    // On each parent: what was granted there only; nothing on the portfolio.
    assert_eq!(
        permissions_of(&api, t, &session, &compliance).await.unwrap(),
        ["view"]
    );
    assert_eq!(permissions_of(&api, t, &session, &digital).await, Err(404));

    // The effective access of the project: direct first, then inherited,
    // each with the node it comes from, in the language asked.
    let access_path = format!("/api/v1/nodes/{plat}/access");
    let res = as_lucas(Method::GET, access_path.clone())
        .header("accept-language", "fr")
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-language"], "fr");
    let (status, access) = json(res).await;
    assert_eq!(status, 200, "{access}");
    let lines: Vec<(&str, &str, &str, &str, bool)> = access
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["holder"]["kind"].as_str().unwrap(),
                a["holder"]["name"].as_str().unwrap(),
                a["role"]["label"].as_str().unwrap(),
                a["origin"]["name"].as_str().unwrap(),
                a["direct"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(lines.len(), 3, "{access}");
    assert_eq!(
        (lines[0].0, lines[0].2, lines[0].3, lines[0].4),
        ("member", "Gestionnaire des membres", "Identity platform", true)
    );
    assert_eq!(
        lines[1..],
        [
            ("group", "Compliance team", "Lecteur", "Compliance", false),
            ("group", "Platform team", "Contributeur", "Modernization", false),
        ]
    );
    assert_eq!(access[0]["grantId"], own["grantId"]);
    assert_eq!(access[1]["origin"]["type"], "program");

    // Managing members: Lucas grants the roles within his own permissions.
    let (status, roles) = send(
        as_lucas(Method::GET, format!("/api/v1/nodes/{plat}/grantable-roles"))
            .header("accept-language", "en"),
    )
    .await;
    assert_eq!(status, 200, "{roles}");
    let labels: Vec<&str> = roles
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Reader", "Contributor", "Member manager"]);
    let (status, found) = send(as_lucas(
        Method::GET,
        format!("/api/v1/nodes/{plat}/grant-candidates?search=team"),
    ))
    .await;
    assert_eq!(status, 200, "{found}");
    let names: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Compliance team", "Platform team"]);
    assert_eq!(
        (&found[0]["kind"], &found[0]["email"]),
        (&json!("group"), &Value::Null)
    );
    let (status, _) = send(as_lucas(
        Method::GET,
        format!("/api/v1/nodes/{plat}/grant-candidates?limit=0"),
    ))
    .await;
    assert_eq!(status, 400);

    let grants_path = format!("/api/v1/nodes/{plat}/grants");
    let (status, granted) =
        send(as_lucas(Method::POST, grants_path.clone()).json(&json!({"roleId": reader, "accountId": ines})))
            .await;
    assert_eq!(status, 201, "{granted}");
    assert_eq!(granted["holder"]["kind"], "member");
    // Not a role beyond his own, neither to grant nor to revoke.
    let (status, problem) = send(
        as_lucas(Method::POST, grants_path.clone()).json(&json!({"roleId": node_admin, "accountId": lucas})),
    )
    .await;
    assert_problem(status, &problem, 403, "forbidden");
    let admins = grant(&api, t, &plat, json!({"roleId": node_admin, "accountId": ines})).await;
    let (status, problem) = send(as_lucas(
        Method::DELETE,
        format!("{grants_path}/{}", admins["grantId"].as_str().unwrap()),
    ))
    .await;
    assert_problem(status, &problem, 403, "forbidden");
    // An inherited grant is revoked where it was made, not here.
    let (status, _) = send(as_lucas(
        Method::DELETE,
        format!("{grants_path}/{}", access[1]["grantId"].as_str().unwrap()),
    ))
    .await;
    assert_eq!(status, 404);
    let res = as_lucas(
        Method::DELETE,
        format!("{grants_path}/{}", granted["grantId"].as_str().unwrap()),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    // Nor on a node where he does not manage members.
    let (status, problem) = send(
        as_lucas(Method::POST, format!("/api/v1/nodes/{compliance}/grants"))
            .json(&json!({"roleId": reader, "accountId": ines})),
    )
    .await;
    assert_problem(status, &problem, 403, "permission.denied");

    // What a grant must be.
    let post = |body: Value| admin(&api, t, Method::POST, &grants_path).json(&body);
    let (status, problem) = send(post(json!({"roleId": member_manager, "accountId": lucas}))).await;
    assert_problem(status, &problem, 409, "node.grant_exists");
    for (body, field, code) in [
        (json!({"roleId": reader}), "accountId", "required"),
        (
            json!({"roleId": reader, "accountId": ines, "groupId": platform_team}),
            "groupId",
            "not_allowed",
        ),
        (
            json!({"roleId": Uuid::now_v7(), "accountId": ines}),
            "roleId",
            "not_found",
        ),
        (
            json!({"roleId": reader, "accountId": Uuid::now_v7()}),
            "accountId",
            "unknown_member",
        ),
        (
            json!({"roleId": reader, "groupId": Uuid::now_v7()}),
            "groupId",
            "not_found",
        ),
    ] {
        let (status, problem) = send(post(body.clone())).await;
        assert_problem(status, &problem, 400, "validation_failed");
        assert_eq!(
            field_codes(&problem),
            [(field.to_owned(), code.to_owned())],
            "{body}"
        );
    }

    // A role in use is not deleted.
    let (status, problem) = send(
        admin(
            &api,
            t,
            Method::DELETE,
            &format!("/api/v1/tenant/roles/{member_manager}"),
        )
        .header("if-match", "*"),
    )
    .await;
    assert_problem(status, &problem, 409, "role.in_use");
    assert_eq!(problem["detail"], "used by 1 grants");
}

#[tokio::test]
async fn grants_never_cross_tenants() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let elsewhere = node(&api, b, json!({"type": "portfolio", "name": "Elsewhere"})).await;
    let (lucas, lucas_session) = member(&db, &api, a).await;
    let (stranger, stranger_session) = member(&db, &api, b).await;
    let (reader_a, reader_b) = (role(&db, a, "Reader").await, role(&db, b, "Reader").await);
    let team_a = group(&api, a, "Team", &[lucas]).await;
    let granted = grant(&api, a, &plat, json!({"roleId": reader_a, "accountId": lucas})).await;
    let grant_id = granted["grantId"].as_str().unwrap();

    // Tenant B knows nothing of A's node and grants.
    for (method, path) in [
        (Method::GET, format!("/api/v1/nodes/{plat}/access")),
        (Method::GET, format!("/api/v1/nodes/{plat}/my-permissions")),
        (Method::GET, format!("/api/v1/nodes/{plat}/grant-candidates")),
        (Method::GET, format!("/api/v1/nodes/{plat}/grantable-roles")),
        (Method::POST, format!("/api/v1/nodes/{plat}/grants")),
        (Method::DELETE, format!("/api/v1/nodes/{plat}/grants/{grant_id}")),
        (
            Method::DELETE,
            format!("/api/v1/nodes/{elsewhere}/grants/{grant_id}"),
        ),
    ] {
        let (status, _) = send(
            admin(&api, b, method.clone(), &path).json(&json!({"roleId": reader_b, "accountId": stranger})),
        )
        .await;
        assert_eq!(status, 404, "{method} {path}");
    }
    // Nor can it grant with A's role, member or group on its own node.
    let post = |body: Value| {
        admin(
            &api,
            b,
            Method::POST,
            &format!("/api/v1/nodes/{elsewhere}/grants"),
        )
        .json(&body)
    };
    for (body, field, code) in [
        (
            json!({"roleId": reader_a, "accountId": stranger}),
            "roleId",
            "not_found",
        ),
        (
            json!({"roleId": reader_b, "accountId": lucas}),
            "accountId",
            "unknown_member",
        ),
        (
            json!({"roleId": reader_b, "groupId": team_a}),
            "groupId",
            "not_found",
        ),
    ] {
        let (status, problem) = send(post(body.clone())).await;
        assert_problem(status, &problem, 400, "validation_failed");
        assert_eq!(
            field_codes(&problem),
            [(field.to_owned(), code.to_owned())],
            "{body}"
        );
    }
    let (_, found) = send(admin(
        &api,
        b,
        Method::GET,
        &format!("/api/v1/nodes/{elsewhere}/grant-candidates?limit=50"),
    ))
    .await;
    let ids: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&stranger.to_string().as_str()), "{found}");
    assert!(
        !ids.contains(&lucas.to_string().as_str()) && !ids.contains(&team_a.as_str()),
        "{found}"
    );

    // Each member keeps to their tenant: A's grant gives nothing in B.
    assert_eq!(
        permissions_of(&api, a, &lucas_session, &plat).await.unwrap(),
        ["view"]
    );
    assert_eq!(permissions_of(&api, b, &stranger_session, &plat).await, Err(404));
    assert_eq!(
        permissions_of(&api, b, &stranger_session, &elsewhere).await,
        Err(404)
    );
    let (_, access) = send(admin(
        &api,
        b,
        Method::GET,
        &format!("/api/v1/nodes/{elsewhere}/access"),
    ))
    .await;
    assert_eq!(access, json!([]));
}
