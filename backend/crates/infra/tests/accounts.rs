//! Integration tests on a real PostgreSQL: accounts, memberships and password
//! resets under row-level security.

#![allow(clippy::unwrap_used)]

use chrono::{Duration, Utc};
use infra::accounts::{self, AccountType, NewMember, TenantRole};
use infra::credentials::token_hash;
use infra::db::begin_tenant_tx;
use infra::sqlx;
use infra::uuid::Uuid;
use testkit::TestDb;

#[tokio::test]
async fn a_member_is_found_in_its_tenant_only() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let found = accounts::find_by_email(&mut tx, &member.email)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.member.account_id, member.account_id);
    assert_eq!(found.member.tenant_role, TenantRole::TenantAdmin);
    assert_eq!(found.member.account_type, AccountType::Internal);
    assert!(found.member.active);
    assert!(infra::credentials::verify_password(
        &member.password,
        found.password_hash.as_deref().unwrap()
    ));
    tx.rollback().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        accounts::find_by_email(&mut tx, &member.email)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        accounts::find(&mut tx, member.account_id)
            .await
            .unwrap()
            .is_none()
    );
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM account")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 0, "accounts of other tenants are invisible");
    assert!(
        !accounts::set_password(&mut tx, member.account_id, "x")
            .await
            .unwrap(),
        "nor changeable"
    );
}

#[tokio::test]
async fn adding_an_existing_email_to_another_tenant_links_the_same_account() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let added = accounts::add_member(
        &mut tx,
        &NewMember {
            email: &member.email,
            display_name: "Someone Else",
            password_hash: None,
            account_type: AccountType::External,
            tenant_role: TenantRole::Member,
        },
    )
    .await
    .unwrap();
    assert_eq!(added.account_id, member.account_id);
    assert!(!added.new_account);
    let found = accounts::find_by_email(&mut tx, &member.email)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.member.account_type, AccountType::External);
    assert_eq!(
        found.member.display_name, "Test Member",
        "the account is unchanged"
    );
    assert!(
        infra::credentials::verify_password(&member.password, found.password_hash.as_deref().unwrap()),
        "the account keeps its password"
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_external_member_cannot_hold_an_administration_role() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let err = accounts::add_member(
        &mut tx,
        &NewMember {
            email: "portal.user@example.org",
            display_name: "Portal User",
            password_hash: None,
            account_type: AccountType::External,
            tenant_role: TenantRole::TenantAdmin,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("check"), "{err}");
}

async fn reset(db: &TestDb, tenant: Uuid, account: Uuid, token: &str, expires_in: Duration) {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    accounts::create_password_reset(&mut tx, account, &token_hash(token), Utc::now() + expires_in)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn consume(db: &TestDb, tenant: Uuid, token: &str) -> Option<Uuid> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let account = accounts::consume_password_reset(&mut tx, &token_hash(token))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    account
}

#[tokio::test]
async fn a_password_reset_is_used_once_within_its_lifetime_in_its_tenant() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    reset(
        &db,
        db.tenant_a.id,
        member.account_id,
        "valid-token",
        Duration::hours(1),
    )
    .await;
    reset(
        &db,
        db.tenant_a.id,
        member.account_id,
        "expired-token",
        Duration::seconds(-1),
    )
    .await;

    assert_eq!(
        consume(&db, db.tenant_b.id, "valid-token").await,
        None,
        "other tenant"
    );
    assert_eq!(consume(&db, db.tenant_a.id, "expired-token").await, None);
    assert_eq!(consume(&db, db.tenant_a.id, "unknown-token").await, None);
    assert_eq!(
        consume(&db, db.tenant_a.id, "valid-token").await,
        Some(member.account_id)
    );
    assert_eq!(consume(&db, db.tenant_a.id, "valid-token").await, None, "used");
}

#[tokio::test]
async fn a_new_password_replaces_the_old_one() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let hash = infra::credentials::hash_password("a brand new passphrase").unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(
        accounts::set_password(&mut tx, member.account_id, &hash)
            .await
            .unwrap()
    );
    let stored = accounts::find_by_email(&mut tx, &member.email)
        .await
        .unwrap()
        .unwrap()
        .password_hash
        .unwrap();
    assert!(infra::credentials::verify_password(
        "a brand new passphrase",
        &stored
    ));
    assert!(!infra::credentials::verify_password(&member.password, &stored));
}
