//! Integration tests on a real PostgreSQL: teams (L07-US02), their names
//! unique per language, their members, the team filter of the resource
//! list, and tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::resources::Kind;
use domain::teams::TeamInput;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::resources::{self, ResourceFilter, ResourceInput};
use infra::sqlx;
use infra::teams::{self, MemberChange, WriteConflict};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

fn team(fr: &str, en: Option<&str>) -> TeamInput {
    let mut names = BTreeMap::from([("fr".to_owned(), fr.to_owned())]);
    if let Some(en) = en {
        names.insert("en".to_owned(), en.to_owned());
    }
    TeamInput {
        name_translations: names,
        description: Some("  ".to_owned()),
    }
}

async fn resource(db: &TestDb, tenant: &SeedTenant, name: &str) -> Uuid {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let created = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some(name),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    created.id
}

async fn create(db: &TestDb, tenant: &SeedTenant, input: &TeamInput) -> teams::Team {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let created = teams::insert(&mut tx, new_id(), input).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    created
}

async fn names_in(db: &TestDb, tenant: &SeedTenant, team: Uuid) -> Vec<String> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let filter = ResourceFilter {
        team: Some(team),
        ..ResourceFilter::default()
    };
    resources::list(&mut tx, &filter, None, 50)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|r| r.display_name)
        .collect()
}

#[tokio::test]
async fn a_team_holds_resources_and_a_resource_several_teams() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let platform = create(&db, a, &team(" Plateforme ", Some("Platform"))).await;
    assert_eq!(
        platform.name_translations,
        BTreeMap::from([
            ("en".to_owned(), "Platform".to_owned()),
            ("fr".to_owned(), "Plateforme".to_owned())
        ])
    );
    assert_eq!(
        (platform.description.as_deref(), platform.member_count),
        (None, 0)
    );
    let run = create(&db, a, &team("Run", None)).await;
    let [alice, bob, carol] = [
        resource(&db, a, "Alice").await,
        resource(&db, a, "Bob").await,
        resource(&db, a, "Carol").await,
    ];

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for id in [alice, bob, carol] {
        assert_eq!(
            teams::add_member(&mut tx, platform.id, id).await.unwrap(),
            MemberChange::Done
        );
    }
    assert_eq!(
        teams::add_member(&mut tx, platform.id, bob).await.unwrap(),
        MemberChange::Unchanged
    );
    assert_eq!(
        teams::add_member(&mut tx, run.id, bob).await.unwrap(),
        MemberChange::Done
    );
    assert_eq!(
        teams::add_member(&mut tx, new_id(), bob).await.unwrap(),
        MemberChange::NoTeam
    );
    assert_eq!(
        teams::add_member(&mut tx, run.id, new_id()).await.unwrap(),
        MemberChange::NoResource
    );
    let found = teams::find(&mut tx, platform.id).await.unwrap().unwrap();
    assert_eq!((found.member_count, found.version), (3, platform.version + 3));
    let of_bob = teams::of_resources(&mut tx, &[bob]).await.unwrap();
    assert_eq!(
        of_bob.iter().map(|(_, t)| t.id).collect::<Vec<_>>(),
        [platform.id, run.id]
    );
    tx.commit().await.unwrap();

    assert_eq!(names_in(&db, a, platform.id).await, ["Alice", "Bob", "Carol"]);
    assert_eq!(names_in(&db, a, run.id).await, ["Bob"]);

    // Out of a team, a resource stays, in its other teams.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        teams::remove_member(&mut tx, platform.id, bob).await.unwrap(),
        MemberChange::Done
    );
    assert_eq!(
        teams::remove_member(&mut tx, platform.id, bob).await.unwrap(),
        MemberChange::Unchanged
    );
    assert_eq!(
        teams::remove_member(&mut tx, new_id(), bob).await.unwrap(),
        MemberChange::NoTeam
    );
    tx.commit().await.unwrap();
    assert_eq!(names_in(&db, a, platform.id).await, ["Alice", "Carol"]);
    assert_eq!(names_in(&db, a, run.id).await, ["Bob"]);

    // A deleted team leaves its resources.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        teams::delete(&mut tx, run.id, Some(&[0])).await.unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    teams::delete(&mut tx, run.id, None).await.unwrap().unwrap();
    assert!(resources::find(&mut tx, bob).await.unwrap().is_some());
    assert!(teams::of_resources(&mut tx, &[bob]).await.unwrap().is_empty());
    assert_eq!(
        teams::delete(&mut tx, run.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn names_are_unique_per_language_whatever_the_case() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let platform = create(&db, a, &team("Plateforme", Some("Platform"))).await;
    let run = create(&db, a, &team("Run", None)).await;
    for taken in [team("PLATEFORME", None), team("Exploitation", Some("platform"))] {
        let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
        assert_eq!(
            teams::insert(&mut tx, new_id(), &taken).await.unwrap(),
            Err(WriteConflict::NameTaken)
        );
    }
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        teams::update(&mut tx, run.id, None, &team("plateforme", None))
            .await
            .unwrap(),
        Err(WriteConflict::NameTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let renamed = teams::update(
        &mut tx,
        platform.id,
        Some(&[platform.version]),
        &team("Plateforme", Some("Core")),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(renamed.version, platform.version + 1);
    assert_eq!(
        teams::update(&mut tx, platform.id, Some(&[platform.version]), &team("X", None))
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    tx.commit().await.unwrap();
    // Another tenant may use the same names.
    create(&db, &db.tenant_b, &team("Plateforme", Some("Platform"))).await;
}

#[tokio::test]
async fn a_tenant_sees_and_fills_only_its_own_teams() {
    let db = TestDb::new().await;
    let of_alpha = create(&db, &db.tenant_a, &team("Plateforme", None)).await;
    let alpha_resource = resource(&db, &db.tenant_a, "Alice").await;
    let of_beta = create(&db, &db.tenant_b, &team("Run", None)).await;
    let beta_resource = resource(&db, &db.tenant_b, "Bob").await;

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(
        teams::list(&mut tx)
            .await
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        [of_beta.id]
    );
    assert_eq!(teams::find(&mut tx, of_alpha.id).await.unwrap(), None);
    // Neither a team nor a resource of alpha, from beta.
    assert_eq!(
        teams::add_member(&mut tx, of_alpha.id, beta_resource)
            .await
            .unwrap(),
        MemberChange::NoTeam
    );
    assert_eq!(
        teams::add_member(&mut tx, of_beta.id, alpha_resource)
            .await
            .unwrap(),
        MemberChange::NoResource
    );
    // The database refuses a membership crossing tenants, whoever writes.
    let refused = sqlx::query(
        "INSERT INTO team_member (id, tenant_id, team_id, resource_id) VALUES ($1, app_current_tenant(), $2, $3)",
    )
    .bind(new_id())
    .bind(of_beta.id)
    .bind(alpha_resource)
    .execute(&mut *tx)
    .await;
    assert!(refused.is_err());
}
