//! Integration tests: idempotency key storage on a real PostgreSQL database.

#![allow(clippy::unwrap_used)]

use infra::db::begin_tenant_tx;
use infra::idempotency::{self, Claim, StoredResponse};
use infra::sqlx;
use serde_json::json;
use testkit::TestDb;

fn stored() -> StoredResponse {
    StoredResponse {
        status: 201,
        body: json!({"id": "x"}),
        location: Some("/api/v1/things/x".to_owned()),
        etag: Some("\"1\"".to_owned()),
    }
}

#[tokio::test]
async fn claim_complete_then_replay() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        idempotency::claim(&mut tx, "k1", b"hash").await.unwrap(),
        Claim::New
    );
    idempotency::complete(&mut tx, "k1", &stored()).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        idempotency::claim(&mut tx, "k1", b"hash").await.unwrap(),
        Claim::Replay(stored())
    );
    assert_eq!(
        idempotency::claim(&mut tx, "k1", b"other").await.unwrap(),
        Claim::Mismatch
    );
}

#[tokio::test]
async fn keys_are_tenant_scoped_under_rls() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    idempotency::claim(&mut tx, "shared", b"hash").await.unwrap();
    idempotency::complete(&mut tx, "shared", &stored()).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM idempotency_key")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 0, "tenant B sees none of A's keys");
    assert_eq!(
        idempotency::claim(&mut tx, "shared", b"different").await.unwrap(),
        Claim::New,
        "the same key is new for tenant B"
    );
}

#[tokio::test]
async fn expired_key_can_be_reused_and_is_purged() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    idempotency::claim(&mut tx, "old", b"hash").await.unwrap();
    idempotency::complete(&mut tx, "old", &stored()).await.unwrap();
    idempotency::claim(&mut tx, "older", b"hash").await.unwrap();
    tx.commit().await.unwrap();
    // Even the owner is subject to forced row-level security: expire the keys
    // from within tenant A's context.
    db.exec_as_owner(&format!(
        "BEGIN;
         SELECT set_config('app.tenant_id', '{}', true);
         UPDATE idempotency_key SET expires_at = now() - interval '1 second';
         COMMIT;",
        db.tenant_a.id
    ))
    .await;

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        idempotency::claim(&mut tx, "old", b"new request").await.unwrap(),
        Claim::New,
        "an expired key protects nothing"
    );
    assert_eq!(
        idempotency::purge_expired(&mut tx).await.unwrap(),
        1,
        "only 'older' is expired now"
    );
    let left: Vec<String> = sqlx::query_scalar("SELECT key FROM idempotency_key")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(left, vec!["old".to_owned()]);
}

#[tokio::test]
async fn new_tables_follow_the_rls_rules() {
    let db = TestDb::new().await;
    assert!(
        infra::db::tables_breaking_rls_rules(&db.owner_pool)
            .await
            .unwrap()
            .is_empty()
    );
    let tenant_first: Vec<String> = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes
         WHERE tablename IN ('idempotency_key', 'reference_item') AND indexname NOT LIKE '%_pkey'",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    assert!(!tenant_first.is_empty());
    for index in tenant_first {
        assert!(
            index.contains("(tenant_id"),
            "index must start with tenant_id: {index}"
        );
    }
}
