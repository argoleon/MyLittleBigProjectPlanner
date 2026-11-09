//! Integration tests of the reference endpoint, on a real PostgreSQL
//! database: every API convention of the specification (§15).

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi, start};
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

const PATH: &str = "/api/v1/reference-items";

fn body(code: &str) -> Value {
    json!({
        "code": code,
        "labelTranslations": {"fr": format!("Libellé {code}"), "en": format!("Label {code}")},
        "quantity": 3
    })
}

async fn create(api: &TestApi, tenant: &SeedTenant, code: &str) -> Value {
    let res = api
        .request(Method::POST, tenant, PATH)
        .json(&body(code))
        .send()
        .await
        .unwrap();
    let (status, created) = json(res).await;
    assert_eq!(status, 201, "{created}");
    created
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
    assert_eq!(body["status"], expected_status);
    assert_eq!(body["type"], format!("urn:mlbpp:problem:{code}"));
    assert!(body["title"].as_str().is_some_and(|t| !t.is_empty()));
}

#[tokio::test]
async fn create_returns_201_with_location_etag_and_resource() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("accept-language", "en-GB,en;q=0.9")
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let id_location = res.headers()["location"].to_str().unwrap().to_owned();
    assert_eq!(res.headers()["etag"], "\"1\"");
    assert_eq!(res.headers()["content-language"], "en");
    let (_, item) = json(res).await;
    assert_eq!(id_location, format!("{PATH}/{}", item["id"].as_str().unwrap()));
    assert_eq!(item["code"], "REF-1");
    assert_eq!(item["label"], "Label REF-1");
    assert_eq!(item["quantity"], 3);
    assert_eq!(item["version"], 1);
    assert!(item.get("labelTranslations").is_none(), "only on request");
    assert!(item["createdAt"].as_str().is_some());
}

#[tokio::test]
async fn invalid_payload_is_a_400_with_field_errors() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .json(&json!({"code": "bad code", "labelTranslations": {"de": "x"}, "quantity": -1}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-type"], "application/problem+json");
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(
        problem["errors"],
        json!([
            {"field": "code", "code": "format"},
            {"field": "labelTranslations", "code": "required"},
            {"field": "labelTranslations.de", "code": "unsupported_locale"},
            {"field": "quantity", "code": "out_of_range"}
        ])
    );
}

#[tokio::test]
async fn malformed_or_mistyped_json_is_a_problem() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 400, "invalid_json");

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .json(&json!({"code": "REF-1", "labelTranslations": {"fr": "x"}, "colour": "red"}))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 400, "invalid_json");
    assert!(
        problem["detail"].as_str().unwrap().contains("colour"),
        "{problem}"
    );

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .body(body("REF-1").to_string())
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 415, "unsupported_media_type");
}

#[tokio::test]
async fn duplicate_code_is_a_409_on_the_code_field() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    create(&api, &db.tenant_a, "REF-1").await;

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 409, "reference_item.code_taken");
    assert_eq!(problem["errors"], json!([{"field": "code", "code": "taken"}]));

    // Codes are unique per tenant only.
    create(&api, &db.tenant_b, "REF-1").await;
}

#[tokio::test]
async fn read_honours_language_fields_and_if_none_match() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let id = create(&api, &db.tenant_a, "REF-1").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("{PATH}/{id}");

    // No Accept-Language: the tenant's default language (fr).
    let res = api
        .request(Method::GET, &db.tenant_a, &path)
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["etag"], "\"1\"");
    assert_eq!(res.headers()["content-language"], "fr");
    let (_, item) = json(res).await;
    assert_eq!(item["label"], "Libellé REF-1");

    let res = api
        .request(
            Method::GET,
            &db.tenant_a,
            &format!("{path}?fields=id,labelTranslations"),
        )
        .send()
        .await
        .unwrap();
    let (_, item) = json(res).await;
    assert_eq!(
        item,
        json!({"id": id, "labelTranslations": {"fr": "Libellé REF-1", "en": "Label REF-1"}})
    );

    let res = api
        .request(Method::GET, &db.tenant_a, &format!("{path}?fields=id,secret"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 400, "invalid_query");
    assert_eq!(
        problem["errors"],
        json!([{"field": "fields.secret", "code": "unknown_field"}])
    );

    let res = api
        .request(Method::GET, &db.tenant_a, &path)
        .header("if-none-match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);
}

#[tokio::test]
async fn update_requires_a_current_if_match() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let id = create(&api, &db.tenant_a, "REF-1").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("{PATH}/{id}");

    // Bring the item to version 3.
    for (n, version) in [(1, "\"1\""), (2, "\"2\"")] {
        let res = api
            .request(Method::PUT, &db.tenant_a, &path)
            .header("if-match", version)
            .json(&json!({"code": "REF-1", "labelTranslations": {"fr": format!("v{n}")}, "quantity": n}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }
    let res = api
        .request(Method::GET, &db.tenant_a, &path)
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["etag"], "\"3\"");

    // Stale ETag: 412, nothing overwritten.
    let res = api
        .request(Method::PUT, &db.tenant_a, &path)
        .header("if-match", "\"2\"")
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 412, "precondition_failed");

    // No If-Match at all: 428.
    let res = api
        .request(Method::PUT, &db.tenant_a, &path)
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 428, "precondition_required");

    // Current ETag: updated, version 4.
    let res = api
        .request(Method::PUT, &db.tenant_a, &path)
        .header("if-match", "\"3\"")
        .json(&body("REF-2"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["etag"], "\"4\"");
    let (status, item) = json(res).await;
    assert_eq!(status, 200);
    assert_eq!(item["code"], "REF-2");
    assert_eq!(item["version"], 4);
}

#[tokio::test]
async fn delete_requires_if_match_and_then_the_item_is_gone() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let id = create(&api, &db.tenant_a, "REF-1").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("{PATH}/{id}");

    let res = api
        .request(Method::DELETE, &db.tenant_a, &path)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 428);
    let res = api
        .request(Method::DELETE, &db.tenant_a, &path)
        .header("if-match", "\"9\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 412);
    let res = api
        .request(Method::DELETE, &db.tenant_a, &path)
        .header("if-match", "\"1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    let res = api
        .request(Method::GET, &db.tenant_a, &path)
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 404, "not_found");
    let res = api
        .request(Method::DELETE, &db.tenant_a, &path)
        .header("if-match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn idempotency_key_replays_the_first_response_without_duplicate() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let key = "8b4f1c1e-6a0c-4a0e-9f55-0d5c2b8f9e11"; // gitleaks:allow (an Idempotency-Key, not a secret)

    let first = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("idempotency-key", key)
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 201);
    assert!(first.headers().get("idempotent-replayed").is_none());
    let location = first.headers()["location"].clone();
    let (_, first_body) = json(first).await;

    // Same key, same body sent with other whitespace: the first response again.
    let replay = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("idempotency-key", key)
        .header("content-type", "application/json")
        .body(serde_json::to_string_pretty(&body("REF-1")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 201);
    assert_eq!(replay.headers()["idempotent-replayed"], "true");
    assert_eq!(replay.headers()["location"], location);
    assert_eq!(replay.headers()["etag"], "\"1\"");
    let (_, replay_body) = json(replay).await;
    assert_eq!(replay_body, first_body);

    let res = api.request(Method::GET, &db.tenant_a, PATH).send().await.unwrap();
    let (_, page) = json(res).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "no duplicate");

    // Same key, different body: refused.
    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("idempotency-key", key)
        .json(&body("REF-2"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 422, "idempotency_key.reused");

    // Keys belong to a tenant: the same key is new for another tenant.
    let res = api
        .request(Method::POST, &db.tenant_b, PATH)
        .header("idempotency-key", key)
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert!(res.headers().get("idempotent-replayed").is_none());
}

#[tokio::test]
async fn concurrent_requests_with_one_key_create_one_item() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let mut requests = Vec::new();
    for _ in 0..8 {
        let request = api
            .request(Method::POST, &db.tenant_a, PATH)
            .header("idempotency-key", "same-key")
            .json(&body("REF-1"));
        requests.push(tokio::spawn(async move {
            request.send().await.unwrap().status().as_u16()
        }));
    }
    for request in requests {
        assert_eq!(request.await.unwrap(), 201);
    }
    let res = api.request(Method::GET, &db.tenant_a, PATH).send().await.unwrap();
    let (_, page) = json(res).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_creation_does_not_burn_the_key() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let invalid = json!({"code": "", "labelTranslations": {"fr": "x"}});

    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("idempotency-key", "retry-me")
        .json(&invalid)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // The client fixes its request and retries with another body: allowed,
    // the failed attempt left nothing behind.
    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("idempotency-key", "retry-me")
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn invalid_idempotency_key_is_a_400() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let res = api
        .request(Method::POST, &db.tenant_a, PATH)
        .header("idempotency-key", "x".repeat(256))
        .json(&body("REF-1"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 400, "idempotency_key.invalid");
}

#[tokio::test]
async fn list_pages_with_a_cursor_in_creation_order() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    for n in 1..=5 {
        create(&api, &db.tenant_a, &format!("REF-{n}")).await;
    }
    create(&api, &db.tenant_b, "OTHER-1").await;

    let mut codes = Vec::new();
    let mut url = format!("{PATH}?limit=2&fields=code");
    let mut pages = 0;
    loop {
        let res = api.request(Method::GET, &db.tenant_a, &url).send().await.unwrap();
        let (status, page) = json(res).await;
        assert_eq!(status, 200, "{page}");
        pages += 1;
        for item in page["items"].as_array().unwrap() {
            assert_eq!(item.as_object().unwrap().len(), 1, "fields=code");
            codes.push(item["code"].as_str().unwrap().to_owned());
        }
        match page["nextCursor"].as_str() {
            Some(cursor) => url = format!("{PATH}?limit=2&fields=code&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(
        codes,
        vec!["REF-1", "REF-2", "REF-3", "REF-4", "REF-5"],
        "only tenant A, in order"
    );

    for query in ["limit=0", "limit=500", "cursor=garbage", "limit=abc"] {
        let res = api
            .request(Method::GET, &db.tenant_a, &format!("{PATH}?{query}"))
            .send()
            .await
            .unwrap();
        let (status, problem) = json(res).await;
        assert_problem(status, &problem, 400, "invalid_query");
    }
}

#[tokio::test]
async fn other_tenant_items_are_404_for_every_operation() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let id = create(&api, &db.tenant_a, "REF-1").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("{PATH}/{id}");

    let read = api
        .request(Method::GET, &db.tenant_b, &path)
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 404);
    let update = api
        .request(Method::PUT, &db.tenant_b, &path)
        .header("if-match", "*")
        .json(&body("STOLEN"))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), 404);
    let delete = api
        .request(Method::DELETE, &db.tenant_b, &path)
        .header("if-match", "*")
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status(), 404);

    let res = api
        .request(Method::GET, &db.tenant_a, &path)
        .send()
        .await
        .unwrap();
    let (status, item) = json(res).await;
    assert_eq!(status, 200);
    assert_eq!(item["code"], "REF-1", "untouched");
}

#[tokio::test]
async fn malformed_id_is_a_404_problem() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let res = api
        .request(Method::GET, &db.tenant_a, &format!("{PATH}/not-a-uuid"))
        .send()
        .await
        .unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 404, "not_found");
}

#[tokio::test]
async fn unexpected_database_error_is_a_generic_500() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    create(&api, &db.tenant_a, "REF-1").await;
    // Simulate a broken deployment: the table disappears.
    db.exec_as_owner("ALTER TABLE reference_item RENAME TO reference_item_gone")
        .await;

    let res = api.request(Method::GET, &db.tenant_a, PATH).send().await.unwrap();
    let (status, problem) = json(res).await;
    assert_problem(status, &problem, 500, "internal");
    let text = problem.to_string();
    assert!(
        !text.contains("reference_item") && !text.contains("relation"),
        "{text}"
    );
    assert!(problem.get("detail").is_none());
}
