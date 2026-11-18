//! Integration tests of the saved filters (L06-US04) on real PostgreSQL and
//! Valkey: saving, sharing with groups or the tenant, who edits and who
//! deletes, results limited to each member's sight, stable identifiers,
//! invalid filters, versions, the departure of an owner, tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

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

/// The filter `body`, saved by `session`.
async fn save(api: &TestApi, tenant: &SeedTenant, session: &(String, String), body: Value) -> Value {
    let (status, filter) =
        send(by(api, tenant, session, Method::POST, "/api/v1/saved-filters").json(&body)).await;
    assert_eq!(status, 201, "{body}: {filter}");
    filter
}

/// The names of the filters `session` sees.
async fn names(api: &TestApi, tenant: &SeedTenant, session: &(String, String)) -> Vec<String> {
    let (status, list) = send(by(api, tenant, session, Method::GET, "/api/v1/saved-filters")).await;
    assert_eq!(status, 200, "{list}");
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .collect()
}

/// A group of `tenant` holding `members`, made by its tenant admin.
async fn group(api: &TestApi, tenant: &SeedTenant, name: &str, members: &[Uuid]) -> Uuid {
    let (status, group) = send(
        api.request(Method::POST, tenant, "/api/v1/tenant/groups")
            .json(&json!({"name": name})),
    )
    .await;
    assert_eq!(status, 201, "{group}");
    let id: Uuid = group["id"].as_str().unwrap().parse().unwrap();
    for account in members {
        let res = api
            .request(
                Method::PUT,
                tenant,
                &format!("/api/v1/tenant/groups/{id}/members/{account}"),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
    }
    id
}

/// The keys `session` finds with `query`.
async fn found(api: &TestApi, tenant: &SeedTenant, session: &(String, String), query: &str) -> Vec<String> {
    let (status, page) =
        send(by(api, tenant, session, Method::POST, "/api/v1/search").json(&json!({"query": query}))).await;
    assert_eq!(status, 200, "{page}");
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_member_saves_a_filter_and_finds_it_among_theirs() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (me, mine) = member(&db, &api, a, TenantRole::Member).await;
    let res = by(&api, a, &mine, Method::POST, "/api/v1/saved-filters")
        .json(&json!({"name": "  My open tickets ", "icon": "person", "query": "assignee = currentUser() AND status != done"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    assert_eq!(res.headers()["etag"], "\"1\"");
    let (_, filter) = json(res).await;
    assert_eq!(
        location,
        format!("/api/v1/saved-filters/{}", filter["id"].as_str().unwrap())
    );
    assert_eq!(
        filter,
        json!({
            "id": filter["id"], "name": "My open tickets", "icon": "person",
            "query": "assignee = currentUser() AND status != done",
            "owner": {"accountId": me, "displayName": filter["owner"]["displayName"], "member": true},
            "sharing": {"all": false, "groups": []},
            "mine": true, "canEdit": true, "canDelete": true, "version": 1
        })
    );
    // The default icon.
    let other = save(&api, a, &mine, json!({"name": "Bugs", "query": "type = bug"})).await;
    assert_eq!(other["icon"], "filter_alt");
    assert_eq!(names(&api, a, &mine).await, ["Bugs", "My open tickets"]);

    let (status, read) = send(by(&api, a, &mine, Method::GET, &location)).await;
    assert_eq!((status, &read["name"]), (200, &json!("My open tickets")));
    let res = by(&api, a, &mine, Method::GET, &location)
        .header("if-none-match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
    // Private: nobody else sees it, not even a tenant admin.
    assert!(
        names(
            &api,
            a,
            &(api.session(a).token.clone(), api.session(a).csrf_token.clone())
        )
        .await
        .is_empty()
    );
    let (status, _) = send(api.request(Method::GET, a, &location)).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_filter_shared_with_a_group_is_run_by_its_members_never_changed_by_them() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (_, owner) = member(&db, &api, a, TenantRole::Member).await;
    let (colleague, colleague_session) = member(&db, &api, a, TenantRole::Member).await;
    let (_, outsider) = member(&db, &api, a, TenantRole::Member).await;
    let team = group(&api, a, "Platform", &[colleague]).await;
    let filter = save(
        &api,
        a,
        &owner,
        json!({"name": "Critical bugs", "icon": "bug_report", "query": "type = bug AND priority = highest",
               "sharing": {"groupIds": [team]}}),
    )
    .await;
    assert_eq!(
        filter["sharing"],
        json!({"all": false, "groups": [{"id": team, "name": "Platform"}]})
    );
    let path = format!("/api/v1/saved-filters/{}", filter["id"].as_str().unwrap());

    assert_eq!(names(&api, a, &colleague_session).await, ["Critical bugs"]);
    let (status, seen) = send(by(&api, a, &colleague_session, Method::GET, &path)).await;
    assert_eq!(status, 200);
    assert_eq!(
        (&seen["mine"], &seen["canEdit"], &seen["canDelete"]),
        (&json!(false), &json!(false), &json!(false))
    );
    let (status, refused) = send(
        by(&api, a, &colleague_session, Method::PUT, &path)
            .header("if-match", "*")
            .json(&json!({"name": "Mine now", "query": "type = bug"})),
    )
    .await;
    assert_eq!((status, &refused["code"]), (403, &json!("forbidden")));
    let (status, _) =
        send(by(&api, a, &colleague_session, Method::DELETE, &path).header("if-match", "*")).await;
    assert_eq!(status, 403);
    // Outside the group: not there.
    assert!(names(&api, a, &outsider).await.is_empty());
    let (status, _) = send(by(&api, a, &outsider, Method::GET, &path)).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_filter_shared_with_the_tenant_is_for_everyone_and_admins_may_delete_it() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (_, owner) = member(&db, &api, a, TenantRole::Member).await;
    let (_, anyone) = member(&db, &api, a, TenantRole::Member).await;
    let shared = save(
        &api,
        a,
        &owner,
        json!({"name": "Late", "query": "due < now()", "sharing": {"all": true}}),
    )
    .await;
    save(
        &api,
        a,
        &owner,
        json!({"name": "Mine", "query": "assignee = currentUser()"}),
    )
    .await;
    assert_eq!(names(&api, a, &anyone).await, ["Late"]);
    let admin = (api.session(a).token.clone(), api.session(a).csrf_token.clone());
    let (status, seen) = send(by(&api, a, &admin, Method::GET, "/api/v1/saved-filters")).await;
    assert_eq!(status, 200);
    assert_eq!(
        (&seen["items"][0]["canEdit"], &seen["items"][0]["canDelete"]),
        (&json!(false), &json!(true)),
        "an admin moderates, never edits"
    );
    let path = format!("/api/v1/saved-filters/{}", shared["id"].as_str().unwrap());
    let res = api
        .request(Method::DELETE, a, &path)
        .header("if-match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(names(&api, a, &anyone).await.is_empty());
    assert_eq!(names(&api, a, &owner).await, ["Mine"]);
}

#[tokio::test]
async fn a_shared_filter_finds_only_what_each_member_sees_whatever_the_labels() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let admin = (api.session(a).token.clone(), api.session(a).csrf_token.clone());
    let mut projects = Vec::new();
    for (name, key) in [("Payroll", "PAY"), ("Restructuring", "SECRET")] {
        let (status, node) = send(
            api.request(Method::POST, a, "/api/v1/nodes")
                .json(&json!({"type": "project", "name": name, "key": key})),
        )
        .await;
        assert_eq!(status, 201, "{node}");
        projects.push((node["id"].as_str().unwrap().parse::<Uuid>().unwrap(), key));
    }
    let (status, body) = send(
        api.request(
            Method::PUT,
            a,
            &format!("/api/v1/nodes/{}/confidentiality", projects[1].0),
        )
        .header("if-match", "*")
        .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for (project, key) in &projects {
        infra::sqlx::query(
            r#"
            INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority)
            VALUES (uuidv7(), app_current_tenant(), $1, 1, $2 || '-1',
                    (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                     AND v.state = 'published' WHERE t.key = 'task' LIMIT 1),
                    (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                     AND v.state = 'published' WHERE s.key = 'in_progress' LIMIT 1),
                    'Ticket', 'medium')
            "#,
        )
        .bind(project)
        .bind(key)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let filter = save(
        &api,
        a,
        &admin,
        json!({"name": "In progress", "query": "status = in_progress ORDER BY key", "sharing": {"all": true}}),
    )
    .await;
    let query = filter["query"].as_str().unwrap();
    let (reader_account, reader) = member(&db, &api, a, TenantRole::Member).await;
    let reader_role: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = 'Reader'")
            .bind(a.id)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let (status, body) = send(
        api.request(
            Method::POST,
            a,
            &format!("/api/v1/nodes/{}/grants", projects[0].0),
        )
        .json(&json!({"roleId": reader_role, "accountId": reader_account})),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(found(&api, a, &admin, query).await, ["PAY-1", "SECRET-1"]);
    assert_eq!(
        found(&api, a, &reader, query).await,
        ["PAY-1"],
        "not the confidential project"
    );

    // Renamed in every language: the filter names its key. (In the tenant's
    // context: row-level security is forced on the owner too.)
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let renamed = infra::sqlx::query(
        "UPDATE workflow_status SET label = '{\"fr\": \"Sur le métier\", \"en\": \"Being worked on\"}' \
         WHERE key = 'in_progress'",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(renamed.rows_affected() > 0);
    tx.commit().await.unwrap();
    assert_eq!(found(&api, a, &admin, query).await, ["PAY-1", "SECRET-1"]);
}

#[tokio::test]
async fn invalid_filters_are_refused_with_what_is_wrong() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let (_, mine) = member(&db, &api, a, TenantRole::Member).await;
    let (_, other) = member(&db, &api, a, TenantRole::Member).await;
    let refusal = |body: Value| {
        let request = by(&api, a, &mine, Method::POST, "/api/v1/saved-filters").json(&body);
        async move { send(request).await }
    };
    let (status, problem) = refusal(json!({"name": "Broken", "query": "status = "})).await;
    assert_eq!((status, &problem["code"]), (400, &json!("query.invalid")));
    assert_eq!(problem["query"]["position"], 9);
    let (status, problem) = refusal(json!({"name": " ", "icon": "rocket", "query": "type = bug"})).await;
    assert_eq!((status, &problem["code"]), (400, &json!("validation_failed")));
    assert_eq!(
        problem["errors"],
        json!([{"field": "name", "code": "required"}, {"field": "icon", "code": "not_allowed"}])
    );
    // A group of another tenant is no group here.
    let elsewhere = group(&api, b, "Elsewhere", &[]).await;
    let (status, problem) =
        refusal(json!({"name": "Shared", "query": "type = bug", "sharing": {"groupIds": [elsewhere]}})).await;
    assert_eq!(
        (status, &problem["errors"]),
        (
            400,
            &json!([{"field": "sharing.groupIds", "code": "unknown_group"}])
        )
    );
    let (status, _) = refusal(json!({"name": "Shared", "query": "type = bug", "unknown": true})).await;
    assert_eq!(status, 400);

    save(&api, a, &mine, json!({"name": "Bugs", "query": "type = bug"})).await;
    let (status, problem) = refusal(json!({"name": "BUGS", "query": "type = story"})).await;
    assert_eq!(
        (status, &problem["code"]),
        (409, &json!("saved_filter.name_taken"))
    );
    // Another member's name.
    save(&api, a, &other, json!({"name": "Bugs", "query": "type = bug"})).await;
}

#[tokio::test]
async fn its_owner_changes_a_filter_from_the_version_read() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (_, mine) = member(&db, &api, a, TenantRole::Member).await;
    let (colleague, colleague_session) = member(&db, &api, a, TenantRole::Member).await;
    let team = group(&api, a, "Platform", &[colleague]).await;
    let filter = save(
        &api,
        a,
        &mine,
        json!({"name": "Bugs", "query": "type = bug", "sharing": {"groupIds": [team]}}),
    )
    .await;
    let path = format!("/api/v1/saved-filters/{}", filter["id"].as_str().unwrap());
    let body = json!({"name": "Open bugs", "icon": "flag", "query": "type = bug AND status != done", "sharing": {"all": false}});
    let (status, _) = send(by(&api, a, &mine, Method::PUT, &path).json(&body)).await;
    assert_eq!(status, 428);
    let (status, _) = send(
        by(&api, a, &mine, Method::PUT, &path)
            .header("if-match", "\"7\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let res = by(&api, a, &mine, Method::PUT, &path)
        .header("if-match", "\"1\"")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["etag"], "\"2\"");
    let (status, changed) = json(res).await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            &changed["name"],
            &changed["icon"],
            &changed["sharing"],
            &changed["version"]
        ),
        (
            &json!("Open bugs"),
            &json!("flag"),
            &json!({"all": false, "groups": []}),
            &json!(2)
        )
    );
    // No longer shared with the group.
    assert!(names(&api, a, &colleague_session).await.is_empty());
    let (status, _) = send(by(&api, a, &mine, Method::DELETE, &path).header("if-match", "\"1\"")).await;
    assert_eq!(status, 412);
    let (status, _) = send(by(&api, a, &mine, Method::DELETE, &path).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204);
    let (status, _) = send(by(&api, a, &mine, Method::GET, &path)).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_departed_owner_takes_their_private_filters_and_leaves_the_shared_ones() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (owner, owner_session) = member(&db, &api, a, TenantRole::Member).await;
    let (_, colleague) = member(&db, &api, a, TenantRole::Member).await;
    save(
        &api,
        a,
        &owner_session,
        json!({"name": "Mine", "query": "assignee = currentUser()"}),
    )
    .await;
    let shared = save(
        &api,
        a,
        &owner_session,
        json!({"name": "Late", "query": "due < now()", "sharing": {"all": true}}),
    )
    .await;
    let res = api
        .request(Method::DELETE, a, &format!("/api/v1/tenant/members/{owner}"))
        .header("if-match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let left: Vec<String> = infra::sqlx::query_scalar("SELECT name FROM saved_filter ORDER BY name")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(left, ["Late"], "the private one went with its owner");
    let (status, seen) = send(by(
        &api,
        a,
        &colleague,
        Method::GET,
        &format!("/api/v1/saved-filters/{}", shared["id"].as_str().unwrap()),
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        (&seen["owner"]["accountId"], &seen["owner"]["member"]),
        (&json!(owner), &json!(false))
    );
}

#[tokio::test]
async fn tenants_keep_their_filters_and_groups_apart() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let admin_a = (api.session(a).token.clone(), api.session(a).csrf_token.clone());
    let admin_b = (api.session(b).token.clone(), api.session(b).csrf_token.clone());
    let filter = save(
        &api,
        a,
        &admin_a,
        json!({"name": "Late", "query": "due < now()", "sharing": {"all": true}}),
    )
    .await;
    assert!(names(&api, b, &admin_b).await.is_empty());
    let (status, _) = send(by(
        &api,
        b,
        &admin_b,
        Method::GET,
        &format!("/api/v1/saved-filters/{}", filter["id"].as_str().unwrap()),
    ))
    .await;
    assert_eq!(status, 404);
    group(&api, a, "Platform", &[]).await;
    group(&api, a, "Payroll team", &[]).await;
    group(&api, b, "Platform B", &[]).await;
    let (_, anyone) = member(&db, &api, a, TenantRole::Member).await;
    let (status, groups) = send(by(
        &api,
        a,
        &anyone,
        Method::GET,
        "/api/v1/saved-filters/share-groups?search=PLAT",
    ))
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        groups
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["name"].clone())
            .collect::<Vec<_>>(),
        [json!("Platform")]
    );
    let (status, groups) = send(by(
        &api,
        a,
        &anyone,
        Method::GET,
        "/api/v1/saved-filters/share-groups?limit=1",
    ))
    .await;
    assert_eq!((status, groups.as_array().unwrap().len()), (200, 1));
    let (status, _) = send(by(
        &api,
        a,
        &anyone,
        Method::GET,
        "/api/v1/saved-filters/share-groups?limit=51",
    ))
    .await;
    assert_eq!(status, 400);
}
