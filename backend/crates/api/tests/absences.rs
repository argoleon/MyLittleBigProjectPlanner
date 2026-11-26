//! Integration tests of absences (L07-US05) on real PostgreSQL and Valkey:
//! a person declares their own, a manager anyone's, others nobody's; the
//! comment for the person and managers only; the capacity of the days off;
//! overlaps refused; the resource of the signed-in member; isolation.

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

/// A member of `tenant` linked to a person resource: (account, session, resource id).
async fn person(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (Uuid, (String, String), String) {
    let account = db.add_member(tenant, TenantRole::Member).await.account_id;
    let (status, created) = send(
        api.request(Method::POST, tenant, "/api/v1/resources")
            .json(&json!({"kind": "person", "accountId": account})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    (
        account,
        session_of(api, tenant, account).await,
        created["id"].as_str().unwrap().to_owned(),
    )
}

/// The minutes of `day` for `resource`.
async fn minutes(api: &TestApi, tenant: &SeedTenant, resource: &str, day: &str) -> Value {
    let (status, days) = send(api.request(
        Method::GET,
        tenant,
        &format!("/api/v1/resources/{resource}/days?from={day}&to={day}"),
    ))
    .await;
    assert_eq!(status, 200, "{days}");
    days[0].clone()
}

#[tokio::test]
async fn a_person_declares_their_own_leave_and_half_day_training() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (me, session, resource) = person(&db, &api, a).await;
    let path = format!("/api/v1/resources/{resource}/absences");

    // Their resource, from the account.
    let (status, mine) = send(by(&api, a, &session, Method::GET, "/api/v1/resources/mine")).await;
    assert_eq!((status, &mine["id"]), (200, &json!(resource)));

    // A full day of leave on Friday 4 December 2026.
    let res = by(&api, a, &session, Method::POST, &path)
        .json(&json!({"kind": "leave", "start": "2026-12-04", "end": "2026-12-04", "comment": " Family "}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let (_, leave) = json(res).await;
    assert_eq!(
        leave,
        json!({
            "id": leave["id"], "kind": "leave", "start": "2026-12-04", "startsAtNoon": false, "end": "2026-12-04",
            "endsAtNoon": false, "comment": "Family", "declaredBy": {"accountId": me, "displayName": "Test Member"},
            "conflictMinutes": 0, "version": 1
        })
    );
    let friday = minutes(&api, a, &resource, "2026-12-04").await;
    assert_eq!(
        (&friday["minutes"], &friday["absence"]),
        (&json!(0), &json!({"kind": "leave", "part": "whole"}))
    );

    // Half a day of training on a 7 h Monday: 3 h 30 left.
    let (status, training) = send(by(&api, a, &session, Method::POST, &path).json(
        &json!({"kind": "training", "start": "2026-12-07", "startsAtNoon": true, "end": "2026-12-07"}),
    ))
    .await;
    assert_eq!(status, 201, "{training}");
    let monday = minutes(&api, a, &resource, "2026-12-07").await;
    assert_eq!(
        (&monday["minutes"], &monday["absence"]["part"]),
        (&json!(210), &json!("afternoon"))
    );

    // A whole day over it is refused.
    let (status, problem) = send(
        by(&api, a, &session, Method::POST, &path)
            .json(&json!({"kind": "other", "start": "2026-12-07", "end": "2026-12-07"})),
    )
    .await;
    assert_eq!((status, &problem["code"]), (409, &json!("absence.overlap")));

    // Changed with its version, then deleted.
    let one = format!("{path}/{}", training["id"].as_str().unwrap());
    let body = json!({"kind": "training", "start": "2026-12-08", "end": "2026-12-08", "endsAtNoon": true});
    let (status, _) = send(by(&api, a, &session, Method::PUT, &one).json(&body)).await;
    assert_eq!(status, 428);
    let (status, _) = send(
        by(&api, a, &session, Method::PUT, &one)
            .header("if-match", "\"3\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 412);
    let (status, moved) = send(
        by(&api, a, &session, Method::PUT, &one)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!((status, &moved["version"]), (200, &json!(2)));
    assert_eq!(minutes(&api, a, &resource, "2026-12-07").await["minutes"], 420);
    let (status, _) = send(by(&api, a, &session, Method::DELETE, &one).header("if-match", "\"2\"")).await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn a_manager_declares_for_someone_and_others_only_see_the_dates() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (_, person_session, resource) = person(&db, &api, a).await;
    let manager_account = db.add_member(a, TenantRole::ResourceManager).await.account_id;
    let manager = session_of(&api, a, manager_account).await;
    let (_, colleague, _) = person(&db, &api, a).await;
    let path = format!("/api/v1/resources/{resource}/absences");

    let (status, declared) =
        send(by(&api, a, &manager, Method::POST, &path).json(
            &json!({"kind": "leave", "start": "2026-12-21", "end": "2026-12-24", "comment": "Holidays"}),
        ))
        .await;
    assert_eq!(status, 201, "{declared}");
    assert_eq!(declared["declaredBy"]["accountId"], json!(manager_account));

    let range = format!("{path}?from=2026-12-01&to=2026-12-31");
    let (_, theirs) = send(by(&api, a, &person_session, Method::GET, &range)).await;
    assert_eq!(
        (&theirs["items"][0]["comment"], &theirs["canManage"]),
        (&json!("Holidays"), &json!(true))
    );
    let (_, seen) = send(by(&api, a, &colleague, Method::GET, &range)).await;
    assert_eq!(seen["canManage"], false);
    assert!(
        seen["items"][0].get("comment").is_none(),
        "the comment is personal"
    );
    assert_eq!(seen["items"][0]["kind"], "leave");

    // Neither the person nor a manager: refused.
    let one = format!("{path}/{}", declared["id"].as_str().unwrap());
    for request in [
        by(&api, a, &colleague, Method::POST, &path)
            .json(&json!({"kind": "leave", "start": "2026-12-28", "end": "2026-12-28"})),
        by(&api, a, &colleague, Method::PUT, &one)
            .header("if-match", "*")
            .json(&json!({"kind": "leave", "start": "2026-12-28", "end": "2026-12-28"})),
        by(&api, a, &colleague, Method::DELETE, &one).header("if-match", "*"),
    ] {
        let (status, problem) = send(request).await;
        assert_eq!((status, &problem["code"]), (403, &json!("forbidden")));
    }
}

#[tokio::test]
async fn invalid_absences_are_refused_with_their_field() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let (_, session, resource) = person(&db, &api, a).await;
    let path = format!("/api/v1/resources/{resource}/absences");
    for (body, field, code) in [
        (
            json!({"kind": "leave", "start": "2026-12-08", "end": "2026-12-07"}),
            "end",
            "before_start",
        ),
        (
            json!({"kind": "leave", "start": "2026-01-01", "end": "2027-01-05"}),
            "end",
            "out_of_range",
        ),
        (
            json!({"kind": "leave", "start": "2026-12-08", "startsAtNoon": true, "end": "2026-12-08", "endsAtNoon": true}),
            "endsAtNoon",
            "not_allowed",
        ),
        (
            json!({"kind": "leave", "start": "2026-12-08", "end": "2026-12-08", "comment": "x".repeat(501)}),
            "comment",
            "too_long",
        ),
    ] {
        let (status, problem) = send(by(&api, a, &session, Method::POST, &path).json(&body)).await;
        assert_eq!(status, 400, "{body}: {problem}");
        assert_eq!(
            problem["errors"][0],
            json!({"field": field, "code": code}),
            "{body}"
        );
    }
    let (status, _) = send(
        by(&api, a, &session, Method::POST, &path)
            .json(&json!({"kind": "sick", "start": "2026-12-08", "end": "2026-12-08"})),
    )
    .await;
    assert_eq!(status, 400, "no sick leave kind");
    // A member linked to no resource has none.
    let unlinked = session_of(&api, a, db.add_member(a, TenantRole::Member).await.account_id).await;
    let (status, _) = send(by(&api, a, &unlinked, Method::GET, "/api/v1/resources/mine")).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_tenant_never_reaches_the_absences_of_another() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (_, session, resource) = person(&db, &api, &db.tenant_a).await;
    let path = format!("/api/v1/resources/{resource}/absences");
    let (status, _) = send(
        by(&api, &db.tenant_a, &session, Method::POST, &path)
            .json(&json!({"kind": "leave", "start": "2026-12-04", "end": "2026-12-04"})),
    )
    .await;
    assert_eq!(status, 201);
    let b = &db.tenant_b;
    let (status, _) =
        send(api.request(Method::GET, b, &format!("{path}?from=2026-12-01&to=2026-12-31"))).await;
    assert_eq!(status, 404);
    let (status, _) = send(
        api.request(Method::POST, b, &path)
            .json(&json!({"kind": "leave", "start": "2026-12-05", "end": "2026-12-05"})),
    )
    .await;
    assert_eq!(status, 404);
}
