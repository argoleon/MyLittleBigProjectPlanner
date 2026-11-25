//! Integration tests on a real PostgreSQL: resources (L07-US01), their
//! tenant isolation, one resource per member, and what happens to a linked
//! person when its member leaves the tenant or is erased.

#![allow(clippy::unwrap_used)]

use domain::resources::Kind;
use infra::accounts::{AccountType, TenantRole};
use infra::db::{begin_tenant_tx, set_account};
use infra::erasure::{self, AccountErasure};
use infra::id::new_id;
use infra::resources::{self, NameKey, ResourceFilter, ResourceInput, WriteConflict};
use infra::sqlx;
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

fn person<'a>(name: &'a str, account: Option<Uuid>) -> ResourceInput<'a> {
    ResourceInput {
        kind: Kind::Person,
        display_name: Some(name),
        job_title: None,
        account_id: account,
        active: true,
        calendar_id: None,
    }
}

fn generic(name: &str) -> ResourceInput<'_> {
    ResourceInput {
        kind: Kind::Generic,
        display_name: Some(name),
        job_title: Some("Back end"),
        account_id: None,
        active: true,
        calendar_id: None,
    }
}

async fn create(db: &TestDb, tenant: &SeedTenant, input: &ResourceInput<'_>) -> resources::Resource {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let resource = resources::insert(&mut tx, new_id(), input)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    resource
}

async fn find(db: &TestDb, tenant: &SeedTenant, id: Uuid) -> Option<resources::Resource> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    resources::find(&mut tx, id).await.unwrap()
}

async fn rename_account(db: &TestDb, account: Uuid, name: &str) {
    sqlx::query("UPDATE account SET display_name = $2 WHERE id = $1")
        .bind(account)
        .bind(name)
        .execute(&db.owner_pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn persons_with_or_without_account_and_generic_resources_are_listed_by_name() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let member = db.add_member(a, TenantRole::Member).await;
    rename_account(&db, member.account_id, "Camille Martin").await;

    let linked = create(&db, a, &person("ignored", Some(member.account_id))).await;
    assert_eq!(linked.display_name, "Camille Martin");
    assert_eq!(linked.account_id, Some(member.account_id));
    assert!(linked.active && !linked.former_member);
    let contractor = create(&db, a, &person("Bob Subcontractor", None)).await;
    assert_eq!(contractor.account_id, None);
    let rust = create(&db, a, &generic("Rust developer")).await;
    assert_eq!(
        (rust.kind, rust.job_title.as_deref()),
        (Kind::Generic, Some("Back end"))
    );

    // The linked person follows its member's name.
    rename_account(&db, member.account_id, "Camille Durand").await;
    assert_eq!(
        find(&db, a, linked.id).await.unwrap().display_name,
        "Camille Durand"
    );

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let all = resources::list(&mut tx, &ResourceFilter::default(), None, 50)
        .await
        .unwrap();
    let names: Vec<&str> = all.items.iter().map(|r| r.display_name.as_str()).collect();
    assert_eq!(names, ["Bob Subcontractor", "Camille Durand", "Rust developer"]);
    assert_eq!(all.next, None);

    // Pages follow the name order.
    let first = resources::list(&mut tx, &ResourceFilter::default(), None, 2)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let next: NameKey = first.next.clone().unwrap();
    let second = resources::list(&mut tx, &ResourceFilter::default(), Some(&next), 2)
        .await
        .unwrap();
    assert_eq!(second.items.iter().map(|r| r.id).collect::<Vec<_>>(), [rust.id]);
    assert_eq!(second.next, None);

    // Filters: kind, active, search in the name and job title.
    let generics = resources::list(
        &mut tx,
        &ResourceFilter {
            kind: Some(Kind::Generic),
            ..ResourceFilter::default()
        },
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(generics.items.iter().map(|r| r.id).collect::<Vec<_>>(), [rust.id]);
    for search in ["durand", "BACK"] {
        let found = resources::list(
            &mut tx,
            &ResourceFilter {
                search: Some(search),
                ..ResourceFilter::default()
            },
            None,
            50,
        )
        .await
        .unwrap();
        assert_eq!(found.items.len(), 1, "{search}");
    }
}

#[tokio::test]
async fn a_member_is_linked_to_one_resource_and_only_an_internal_member_is_linked() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let member = db.add_member(a, TenantRole::Member).await;
    let external = db
        .add_member_with(a, TenantRole::Member, AccountType::External)
        .await;
    let of_beta = db.add_member(&db.tenant_b, TenantRole::Member).await;
    create(&db, a, &person("x", Some(member.account_id))).await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        resources::insert(&mut tx, new_id(), &person("x", Some(member.account_id)))
            .await
            .unwrap(),
        Err(WriteConflict::AccountTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for account in [external.account_id, of_beta.account_id, new_id()] {
        assert_eq!(
            resources::insert(&mut tx, new_id(), &person("x", Some(account)))
                .await
                .unwrap(),
            Err(WriteConflict::NotInternalMember)
        );
    }
    // The members still free to link: neither the linked one nor externals.
    let free = resources::linkable_accounts(&mut tx, None, 50).await.unwrap();
    assert!(free.iter().all(|f| f.account_id != member.account_id));
    assert!(free.iter().all(|f| f.account_id != external.account_id));

    // The database refuses it too, whoever writes.
    let unique = sqlx::query(
        "INSERT INTO resource (id, tenant_id, kind, display_name, account_id)
         VALUES ($1, app_current_tenant(), 'person', 'Twin', $2)",
    )
    .bind(new_id())
    .bind(member.account_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(unique.to_string().contains("resource_account_idx"), "{unique}");
}

#[tokio::test]
async fn the_database_keeps_generic_resources_unlinked_and_linked_accounts_in_the_tenant() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let of_beta = db.add_member(&db.tenant_b, TenantRole::Member).await;
    for (kind, account) in [("generic", member.account_id), ("person", of_beta.account_id)] {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        let refused = sqlx::query(
            "INSERT INTO resource (id, tenant_id, kind, display_name, account_id)
             VALUES ($1, app_current_tenant(), $2, 'X', $3)",
        )
        .bind(new_id())
        .bind(kind)
        .bind(account)
        .execute(&mut *tx)
        .await;
        assert!(refused.is_err(), "{kind}");
    }
}

#[tokio::test]
async fn updates_check_the_version_and_linking_forgets_the_former_member() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let member = db.add_member(a, TenantRole::Member).await;
    rename_account(&db, member.account_id, "Camille Martin").await;
    let resource = create(&db, a, &person("Camille M.", None)).await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        resources::update(&mut tx, resource.id, Some(&[resource.version + 1]), &generic("x"))
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        resources::update(&mut tx, new_id(), None, &generic("x"))
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    let linked = resources::update(
        &mut tx,
        resource.id,
        Some(&[resource.version]),
        &person("ignored", Some(member.account_id)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (linked.display_name.as_str(), linked.version),
        ("Camille Martin", resource.version + 1)
    );
    let deactivated = resources::update(
        &mut tx,
        resource.id,
        None,
        &ResourceInput {
            active: false,
            ..person("Camille Martin", None)
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!deactivated.active);
    assert_eq!(deactivated.account_id, None);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_member_leaving_the_tenant_leaves_an_inactive_resource_under_their_name() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let member = db.add_member(a, TenantRole::Member).await;
    rename_account(&db, member.account_id, "Camille Martin").await;
    let resource = create(&db, a, &person("x", Some(member.account_id))).await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::members::remove(&mut tx, member.account_id, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let left = find(&db, a, resource.id).await.unwrap();
    assert_eq!(left.display_name, "Camille Martin");
    assert_eq!(left.account_id, None);
    assert!(left.former_member && !left.active);
    assert_eq!(left.version, resource.version + 1);

    // Erasing the global account later anonymizes the copy too.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    set_account(&mut tx, member.account_id).await.unwrap();
    let erased = erasure::erase_account(&mut tx, member.account_id, member.account_id, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let AccountErasure::Erased(number) = erased else {
        panic!("{erased:?}")
    };
    assert_eq!(
        find(&db, a, resource.id).await.unwrap().display_name,
        format!("Deleted user #{number}")
    );
}

#[tokio::test]
async fn an_erased_member_leaves_a_resource_under_their_pseudonym() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await;
    let in_tenant = db.add_member(a, TenantRole::Member).await;
    let everywhere = db.add_member(a, TenantRole::Member).await;
    let first = create(&db, a, &person("x", Some(in_tenant.account_id))).await;
    let second = create(&db, a, &person("x", Some(everywhere.account_id))).await;

    // A tenant admin erases one member in this tenant.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    set_account(&mut tx, admin.account_id).await.unwrap();
    let number = erasure::erase_in_tenant(
        &mut tx,
        admin.account_id,
        in_tenant.account_id,
        &in_tenant.email,
        false,
    )
    .await
    .unwrap();
    infra::members::remove(&mut tx, in_tenant.account_id, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let left = find(&db, a, first.id).await.unwrap();
    assert_eq!(left.display_name, format!("Deleted user #{number}"));
    assert!(left.former_member && !left.active);

    // The other erases their global account.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    set_account(&mut tx, everywhere.account_id).await.unwrap();
    let AccountErasure::Erased(number) =
        erasure::erase_account(&mut tx, everywhere.account_id, everywhere.account_id, false)
            .await
            .unwrap()
    else {
        panic!("not erased")
    };
    tx.commit().await.unwrap();
    let left = find(&db, a, second.id).await.unwrap();
    assert_eq!(left.display_name, format!("Deleted user #{number}"));
    assert!(left.former_member && !left.active);
    assert_eq!(left.account_id, None);
}

#[tokio::test]
async fn a_tenant_sees_and_writes_only_its_own_resources() {
    let db = TestDb::new().await;
    let of_alpha = create(&db, &db.tenant_a, &generic("Rust developer")).await;
    let of_beta = create(&db, &db.tenant_b, &generic("Java developer")).await;

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let listed = resources::list(&mut tx, &ResourceFilter::default(), None, 50)
        .await
        .unwrap();
    assert_eq!(
        listed.items.iter().map(|r| r.id).collect::<Vec<_>>(),
        [of_beta.id]
    );
    assert_eq!(resources::find(&mut tx, of_alpha.id).await.unwrap(), None);
    assert_eq!(
        resources::update(&mut tx, of_alpha.id, None, &generic("Stolen"))
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    // Row-level security refuses a row of another tenant outright.
    let refused = sqlx::query(
        "INSERT INTO resource (id, tenant_id, kind, display_name) VALUES ($1, $2, 'generic', 'X')",
    )
    .bind(new_id())
    .bind(db.tenant_a.id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("row-level security"), "{refused}");
    tx.rollback().await.unwrap();

    assert_eq!(
        find(&db, &db.tenant_a, of_alpha.id).await.unwrap().display_name,
        "Rust developer"
    );
}
