//! Integration tests on a real PostgreSQL: the signed-in account's
//! memberships across tenants (`app_my_memberships()`), the only query path
//! that crosses row-level security, and only for the caller.

#![allow(clippy::unwrap_used)]

use infra::accounts::{self, AccountType, NewMember, TenantRole};
use infra::db::{begin_tenant_tx, set_account};
use infra::memberships::mine;
use testkit::{SeedTenant, TestDb};

/// Makes the account of `email` a member of `tenant` too.
async fn join(db: &TestDb, tenant: &SeedTenant, email: &str, role: TenantRole, account_type: AccountType) {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    accounts::add_member(
        &mut tx,
        &NewMember {
            email,
            display_name: "Test Member",
            password_hash: None,
            account_type,
            tenant_role: role,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn the_caller_sees_its_own_memberships_in_every_tenant() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    join(
        &db,
        &db.tenant_b,
        &member.email,
        TenantRole::Member,
        AccountType::External,
    )
    .await;
    // Other accounts, in both tenants.
    db.add_member(&db.tenant_a, TenantRole::Member).await;
    db.add_member(&db.tenant_b, TenantRole::Member).await;

    // From alpha's transaction, beta's membership too.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    set_account(&mut tx, member.account_id).await.unwrap();
    let mut found = mine(&mut tx).await.unwrap();
    found.sort_by_key(|m| m.slug.clone());
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].tenant_id, db.tenant_a.id);
    assert_eq!(found[0].slug, db.tenant_a.slug);
    assert_eq!(found[0].name, db.tenant_a.name);
    assert_eq!(found[0].tenant_role, TenantRole::TenantAdmin);
    assert_eq!(found[0].account_type, AccountType::Internal);
    assert_eq!(found[1].tenant_id, db.tenant_b.id);
    assert_eq!(found[1].tenant_role, TenantRole::Member);
    assert_eq!(found[1].account_type, AccountType::External);
}

#[tokio::test]
async fn nothing_is_listed_without_a_signed_in_account() {
    let db = TestDb::new().await;
    db.add_member(&db.tenant_a, TenantRole::Member).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(mine(&mut tx).await.unwrap().is_empty());
    // The setting is local to the transaction.
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    set_account(&mut tx, member.account_id).await.unwrap();
    assert_eq!(mine(&mut tx).await.unwrap().len(), 1);
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(mine(&mut tx).await.unwrap().is_empty());
}

#[tokio::test]
async fn suspended_memberships_and_tenants_being_deleted_are_left_out() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(
        &db,
        &db.tenant_b,
        &member.email,
        TenantRole::Member,
        AccountType::Internal,
    )
    .await;
    let count = || async {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        set_account(&mut tx, member.account_id).await.unwrap();
        mine(&mut tx).await.unwrap().len()
    };
    assert_eq!(count().await, 2);

    db.exec_as_owner(&format!(
        "UPDATE membership SET status = 'suspended' WHERE account_id = '{}' AND tenant_id = '{}'",
        member.account_id, db.tenant_b.id
    ))
    .await;
    assert_eq!(count().await, 1);

    db.exec_as_owner(&format!(
        "UPDATE membership SET status = 'active' WHERE tenant_id = '{}'",
        db.tenant_b.id
    ))
    .await;
    db.exec_as_owner(&format!(
        "UPDATE tenant SET status = 'deleting' WHERE id = '{}'",
        db.tenant_b.id
    ))
    .await;
    assert_eq!(count().await, 1);
}

#[tokio::test]
async fn the_runtime_role_cannot_read_memberships_of_other_tenants_directly() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(
        &db,
        &db.tenant_b,
        &member.email,
        TenantRole::Member,
        AccountType::Internal,
    )
    .await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    set_account(&mut tx, member.account_id).await.unwrap();
    // Row-level security still applies to the table itself.
    let rows: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM membership WHERE account_id = $1")
        .bind(member.account_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}
