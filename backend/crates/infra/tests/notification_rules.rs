//! Integration tests on a real PostgreSQL: the notification rules of a
//! configuration version (L04-US07). One rule per event in every
//! configuration, in step with the domain's defaults; copies into a draft
//! and a duplicate; a change refused on a stale version; tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::notifications::{DEFAULT_RULES, Event, RecipientKind, Rule};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::notification_rules::{self, WriteConflict};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use testkit::TestDb;

fn labels(en: &str) -> BTreeMap<String, String> {
    [("en".to_owned(), en.to_owned())].into()
}

async fn config(conn: &mut PgConnection, en: &str) -> (Uuid, Uuid) {
    let config = configs::insert(conn, new_id(), &labels(en), None)
        .await
        .unwrap()
        .unwrap();
    (config.id, config.published.id)
}

fn reporter_and_managers() -> Rule {
    Rule {
        kinds: vec![RecipientKind::Reporter],
        roles: vec![7],
        groups: vec![8],
        accounts: vec![9],
    }
}

#[tokio::test]
async fn every_configuration_starts_with_the_default_rules() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let default = configs::default_id(&mut tx).await.unwrap().unwrap();
    let default_version = configs::find(&mut tx, default)
        .await
        .unwrap()
        .unwrap()
        .published
        .id;
    let (_, created_version) = config(&mut tx, "Software").await;
    for version in [default_version, created_version] {
        let rules = notification_rules::list(&mut tx, version).await.unwrap();
        // In step with the domain's list, in the order of the events.
        assert_eq!(rules.len(), DEFAULT_RULES.len());
        for (stored, expected) in rules.iter().zip(DEFAULT_RULES) {
            assert_eq!(stored.event, expected.event);
            assert_eq!(stored.rule.kinds, expected.kinds);
            assert!(
                stored.rule.roles.is_empty()
                    && stored.rule.groups.is_empty()
                    && stored.rule.accounts.is_empty()
            );
        }
    }
}

#[tokio::test]
async fn rules_are_changed_and_copied_with_their_configuration() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Software").await;
    notification_rules::update(
        &mut tx,
        published,
        Event::Created,
        Some(&[1]),
        &reporter_and_managers(),
    )
    .await
    .unwrap()
    .unwrap();
    let created = |rules: &[notification_rules::NotificationRule]| {
        rules.iter().find(|r| r.event == Event::Created).unwrap().clone()
    };
    let stored = created(&notification_rules::list(&mut tx, published).await.unwrap());
    assert_eq!(
        (stored.rule.clone(), stored.version),
        (reporter_and_managers(), 2)
    );
    // A stale version is refused.
    assert_eq!(
        notification_rules::update(&mut tx, published, Event::Created, Some(&[1]), &Rule::default())
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );

    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let duplicate = configs::duplicate(&mut tx, id, new_id(), &labels("Copy"), None, None)
        .await
        .unwrap()
        .unwrap()
        .published
        .id;
    for version in [draft, duplicate] {
        let rules = notification_rules::list(&mut tx, version).await.unwrap();
        assert_eq!(rules.len(), DEFAULT_RULES.len());
        assert_eq!(created(&rules).rule, reporter_and_managers());
    }
    // Turned off in the draft: the published version keeps it.
    notification_rules::update(&mut tx, draft, Event::Created, None, &Rule::default())
        .await
        .unwrap()
        .unwrap();
    assert!(
        created(&notification_rules::list(&mut tx, draft).await.unwrap())
            .rule
            .is_off()
    );
    assert_eq!(
        created(&notification_rules::list(&mut tx, published).await.unwrap()).rule,
        reporter_and_managers()
    );
}

#[tokio::test]
async fn rules_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Software").await;
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        notification_rules::list(&mut tx, published)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        notification_rules::update(&mut tx, published, Event::Created, None, &reporter_and_managers())
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
}
