//! Integration tests on a real PostgreSQL and Valkey: node roles under
//! row-level security, the default roles of a tenant, and the permission
//! cache (L02-US07).

#![allow(clippy::unwrap_used)]

use domain::permissions::{DEFAULT_ROLES, Permission, PermissionSet, RoleInput};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::permissions::PermissionCache;
use infra::roles::{self, WriteConflict};
use testkit::TestDb;

fn input(fr: &str, en: &str, permissions: &[Permission]) -> RoleInput {
    RoleInput {
        label_translations: [("fr".to_owned(), fr.to_owned()), ("en".to_owned(), en.to_owned())].into(),
        permissions: permissions.iter().copied().collect(),
    }
}

#[tokio::test]
async fn a_tenant_starts_with_the_default_roles_of_the_domain() {
    let db = TestDb::new().await;
    for tenant in [&db.tenant_a, &db.tenant_b] {
        let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
        let seeded = roles::list(&mut tx, None, 50).await.unwrap().items;
        assert_eq!(seeded.len(), DEFAULT_ROLES.len());
        for (role, expected) in seeded.iter().zip(DEFAULT_ROLES) {
            assert_eq!(role.label_translations["fr"], expected.label_fr);
            assert_eq!(role.label_translations["en"], expected.label_en);
            let permissions: PermissionSet = expected.permissions.iter().copied().collect();
            assert_eq!(role.permissions, permissions, "{}", expected.label_en);
        }
    }
}

#[tokio::test]
async fn roles_are_isolated_and_labelled_once_per_language() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let role = roles::insert(
        &mut tx,
        new_id(),
        &input("Auditeur", "Auditor", &[Permission::View]),
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    // The unique violation aborts the transaction: the API rolls back.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        roles::insert(&mut tx, new_id(), &input("Autre", "AUDITOR", &[Permission::View]))
            .await
            .unwrap(),
        Err(WriteConflict::LabelTaken)
    );
    drop(tx);

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(roles::find(&mut tx, role.id).await.unwrap(), None);
    assert_eq!(
        roles::permissions_of(&mut tx, &[role.id]).await.unwrap(),
        PermissionSet::EMPTY
    );
    assert_eq!(
        roles::delete(&mut tx, role.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    roles::insert(
        &mut tx,
        new_id(),
        &input("Auditeur", "Auditor", &[Permission::View]),
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn the_permissions_of_several_roles_add_up() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let a = roles::insert(&mut tx, new_id(), &input("A", "A", &[Permission::View]))
        .await
        .unwrap()
        .unwrap();
    let b = roles::insert(
        &mut tx,
        new_id(),
        &input("B", "B", &[Permission::Comment, Permission::LogTime]),
    )
    .await
    .unwrap()
    .unwrap();
    let set = roles::permissions_of(&mut tx, &[a.id, b.id, new_id()])
        .await
        .unwrap();
    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [Permission::View, Permission::Comment, Permission::LogTime]
    );
    assert_eq!(
        roles::permissions_of(&mut tx, &[]).await.unwrap(),
        PermissionSet::EMPTY
    );
}

#[tokio::test]
async fn the_cache_forgets_a_tenant_at_once_and_only_that_tenant() {
    let (valkey, prefix) = testkit::valkey().await;
    let cache = PermissionCache::new(valkey, prefix);
    let (alpha, beta, account) = (new_id(), new_id(), new_id());
    let reader: PermissionSet = [Permission::View].into_iter().collect();

    assert_eq!(cache.epoch(alpha).await.unwrap(), 0);
    cache.put(alpha, 0, account, None, reader).await.unwrap();
    cache.put(beta, 0, account, None, reader).await.unwrap();
    assert_eq!(cache.get(alpha, 0, account, None).await.unwrap(), Some(reader));
    assert_eq!(cache.get(alpha, 0, account, Some(new_id())).await.unwrap(), None);

    cache.invalidate(alpha).await.unwrap();
    let epoch = cache.epoch(alpha).await.unwrap();
    assert_eq!(epoch, 1);
    assert_eq!(cache.get(alpha, epoch, account, None).await.unwrap(), None);
    assert_eq!(cache.epoch(beta).await.unwrap(), 0);
    assert_eq!(cache.get(beta, 0, account, None).await.unwrap(), Some(reader));
}
