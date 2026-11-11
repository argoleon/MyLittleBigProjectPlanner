//! Integration tests of the groups of members (L02-US06): create, rename,
//! delete, add and remove members, and isolation between tenants.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const GROUPS: &str = "/api/v1/tenant/groups";

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

async fn create(api: &TestApi, tenant: &SeedTenant, body: Value) -> (u16, Value) {
    send(api.request(Method::POST, tenant, GROUPS).json(&body)).await
}

fn member_path(group: &str, account_id: Uuid) -> String {
    format!("{GROUPS}/{group}/members/{account_id}")
}

#[tokio::test]
async fn a_group_is_created_renamed_and_deleted() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let (status, body) = create(
        &api,
        &db.tenant_a,
        json!({"name": " ", "description": "x".repeat(1001)}),
    )
    .await;
    assert_problem(status, &body, 400, "validation_failed");
    assert_eq!(body["errors"].as_array().unwrap().len(), 2);

    let res = api
        .request(Method::POST, &db.tenant_a, GROUPS)
        .header("idempotency-key", "group-platform")
        .json(&json!({"name": "  Platform ", "description": "Platform team"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    let (_, group) = json(res).await;
    assert_eq!(group["name"], "Platform");
    assert_eq!(group["description"], "Platform team");
    assert_eq!(group["memberCount"], 0);
    let id = group["id"].as_str().unwrap().to_owned();
    assert_eq!(location, format!("{GROUPS}/{id}"));
    // Replayed, not created twice.
    let res = api
        .request(Method::POST, &db.tenant_a, GROUPS)
        .header("idempotency-key", "group-platform")
        .json(&json!({"name": "  Platform ", "description": "Platform team"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["idempotent-replayed"], "true");

    // Unique whatever the case.
    let (status, body) = create(&api, &db.tenant_a, json!({"name": "PLATFORM"})).await;
    assert_problem(status, &body, 409, "group.name_taken");
    assert_eq!(body["errors"][0]["field"], "name");
    // Another tenant may use the name.
    let (status, _) = create(&api, &db.tenant_b, json!({"name": "Platform"})).await;
    assert_eq!(status, 201);

    let (status, managers) = create(&api, &db.tenant_a, json!({"name": "Managers"})).await;
    assert_eq!(status, 201);
    let (status, page) = send(api.request(Method::GET, &db.tenant_a, &format!("{GROUPS}?limit=1"))).await;
    assert_eq!(status, 200);
    assert_eq!(page["items"][0]["name"], "Platform");
    let cursor = page["nextCursor"].as_str().unwrap();
    let (_, page) = send(api.request(
        Method::GET,
        &db.tenant_a,
        &format!("{GROUPS}?limit=1&cursor={cursor}"),
    ))
    .await;
    assert_eq!(page["items"][0]["id"], managers["id"]);
    assert_eq!(page["nextCursor"], Value::Null);

    let (status, body) = send(
        api.request(Method::PUT, &db.tenant_a, &format!("{GROUPS}/{id}"))
            .header("if-match", "\"1\"")
            .json(&json!({"name": "Managers"})),
    )
    .await;
    assert_problem(status, &body, 409, "group.name_taken");
    let res = api
        .request(Method::PUT, &db.tenant_a, &format!("{GROUPS}/{id}"))
        .header("if-match", "\"1\"")
        .json(&json!({"name": "Platform team"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["etag"], "\"2\"");
    let (_, renamed) = json(res).await;
    assert_eq!(renamed["name"], "Platform team");
    assert_eq!(renamed["description"], Value::Null);

    let (status, body) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{GROUPS}/{id}"))
            .header("if-match", "\"1\""),
    )
    .await;
    assert_problem(status, &body, 412, "precondition_failed");
    let (status, _) = send(
        api.request(Method::DELETE, &db.tenant_a, &format!("{GROUPS}/{id}"))
            .header("if-match", "\"2\""),
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = send(api.request(Method::GET, &db.tenant_a, &format!("{GROUPS}/{id}"))).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn members_are_added_to_and_removed_from_a_group() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let ines = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let (_, group) = create(&api, &db.tenant_a, json!({"name": "Platform"})).await;
    let id = group["id"].as_str().unwrap().to_owned();

    for member in [&lucas, &ines, &lucas] {
        let (status, _) =
            send(api.request(Method::PUT, &db.tenant_a, &member_path(&id, member.account_id))).await;
        assert_eq!(status, 204);
    }
    let res = api
        .request(Method::GET, &db.tenant_a, &format!("{GROUPS}/{id}"))
        .send()
        .await
        .unwrap();
    // Two additions, the repeated one changed nothing.
    assert_eq!(res.headers()["etag"], "\"3\"");
    let (_, detail) = json(res).await;
    assert_eq!(detail["memberCount"], 2);
    let members: Vec<&Value> = detail["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["accountId"])
        .collect();
    assert_eq!(members.len(), 2);
    assert!(members.contains(&&json!(lucas.account_id)));

    // The members list shows each member's groups.
    let (_, page) = send(api.request(Method::GET, &db.tenant_a, "/api/v1/tenant/members")).await;
    let lucas_row = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["accountId"] == json!(lucas.account_id))
        .unwrap();
    assert_eq!(lucas_row["groups"], json!([{"id": id, "name": "Platform"}]));

    let (status, _) =
        send(api.request(Method::DELETE, &db.tenant_a, &member_path(&id, lucas.account_id))).await;
    assert_eq!(status, 204);
    let (status, body) =
        send(api.request(Method::DELETE, &db.tenant_a, &member_path(&id, lucas.account_id))).await;
    assert_problem(status, &body, 404, "not_found");
    let (_, detail) = send(api.request(Method::GET, &db.tenant_a, &format!("{GROUPS}/{id}"))).await;
    assert_eq!(
        detail["members"],
        json!([{"accountId": ines.account_id, "email": ines.email, "displayName": "Test Member"}])
    );
}

#[tokio::test]
async fn groups_and_members_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let alpha_member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let beta_member = db.add_member(&db.tenant_b, TenantRole::Member).await;
    let (_, alpha_group) = create(&api, &db.tenant_a, json!({"name": "Platform"})).await;
    let (_, beta_group) = create(&api, &db.tenant_b, json!({"name": "Suppliers"})).await;
    let alpha_group = alpha_group["id"].as_str().unwrap().to_owned();
    let beta_group = beta_group["id"].as_str().unwrap().to_owned();

    // A member of another tenant cannot join.
    let (status, body) = send(api.request(
        Method::PUT,
        &db.tenant_a,
        &member_path(&alpha_group, beta_member.account_id),
    ))
    .await;
    assert_problem(status, &body, 404, "not_found");
    // Another tenant's group is invisible.
    let requests = [
        api.request(Method::GET, &db.tenant_a, &format!("{GROUPS}/{beta_group}")),
        api.request(Method::PUT, &db.tenant_a, &format!("{GROUPS}/{beta_group}"))
            .header("if-match", "*")
            .json(&json!({"name": "Taken over"})),
        api.request(Method::DELETE, &db.tenant_a, &format!("{GROUPS}/{beta_group}"))
            .header("if-match", "*"),
        api.request(
            Method::PUT,
            &db.tenant_a,
            &member_path(&beta_group, alpha_member.account_id),
        ),
        api.request(
            Method::DELETE,
            &db.tenant_a,
            &member_path(&beta_group, beta_member.account_id),
        ),
    ];
    for request in requests {
        let (status, body) = send(request).await;
        assert_problem(status, &body, 404, "not_found");
    }
    let (_, page) = send(api.request(Method::GET, &db.tenant_a, GROUPS)).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (_, beta) = send(api.request(Method::GET, &db.tenant_b, &format!("{GROUPS}/{beta_group}"))).await;
    assert_eq!(beta["name"], "Suppliers");
}
