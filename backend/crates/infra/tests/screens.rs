//! Integration tests on a real PostgreSQL: the screens of a configuration
//! version (L04-US06). The general screens of every configuration, in step
//! with the domain; one screen of a kind per type; copies into a draft and
//! a duplicate keeping identities and the transition screens; a type
//! removed taking its screens; a transition screen kept while a transition
//! shows it; fields added to and removed from screens; tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::screens::{DEFAULT_SCREENS, Kind};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::screens::{self, WriteConflict};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use infra::workflows::{self, TransitionFields};
use testkit::TestDb;

fn labels(fr: &str, en: &str) -> BTreeMap<String, String> {
    [("fr".to_owned(), fr.to_owned()), ("en".to_owned(), en.to_owned())].into()
}

/// A new configuration of the current tenant; its id and published version.
async fn config(conn: &mut PgConnection, en: &str) -> (Uuid, Uuid) {
    let config = configs::insert(conn, new_id(), &labels(en, en), None)
        .await
        .unwrap()
        .unwrap();
    (config.id, config.published.id)
}

async fn type_id(conn: &mut PgConnection, version: Uuid, key: &str) -> Uuid {
    infra::issue_types::list(conn, version)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == key)
        .unwrap()
        .item_id
}

async fn field_ids(conn: &mut PgConnection, version: Uuid) -> Vec<Uuid> {
    infra::fields::list(conn, version)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.item_id)
        .collect()
}

/// Attaches `screen` to the first transition of `version`; that transition.
async fn attach(conn: &mut PgConnection, version: Uuid, screen: Option<Uuid>) -> Uuid {
    let all = workflows::load(conn, version).await.unwrap();
    let transition = all.transitions[0].clone();
    workflows::update_transition(
        conn,
        version,
        transition.item_id,
        None,
        &TransitionFields {
            name_translations: &transition.name_translations,
            from_status_id: transition.from_status_id,
            to_status_id: transition.to_status_id,
            rules: None,
            screen_id: Some(screen),
        },
    )
    .await
    .unwrap()
    .unwrap();
    transition.item_id
}

#[tokio::test]
async fn every_configuration_starts_with_the_general_screens_of_its_fields() {
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
        let fields = field_ids(&mut tx, version).await;
        let all = screens::list(&mut tx, version).await.unwrap();
        // In step with the domain's list.
        assert_eq!(all.len(), DEFAULT_SCREENS.len());
        for (stored, expected) in all.iter().zip(DEFAULT_SCREENS) {
            assert_eq!(stored.kind, expected.kind);
            assert_eq!(stored.issue_type_id, None);
            assert_eq!(
                stored.name_translations,
                labels(expected.name_fr, expected.name_en)
            );
            assert_eq!(stored.field_ids, fields);
        }
    }
}

#[tokio::test]
async fn a_type_has_one_screen_of_a_kind_and_takes_it_when_removed() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, version) = config(&mut tx, "Software").await;
    let bug = type_id(&mut tx, version, "bug").await;
    let fields = field_ids(&mut tx, version).await;
    let bug_creation = new_id();
    screens::insert(
        &mut tx,
        version,
        bug_creation,
        Kind::Creation,
        Some(bug),
        &labels("Création d’un bug", "Bug creation"),
        &fields[1..],
    )
    .await
    .unwrap()
    .unwrap();
    // Two transition screens are fine.
    for name in ["Motif", "Validation"] {
        screens::insert(
            &mut tx,
            version,
            new_id(),
            Kind::Transition,
            None,
            &labels(name, name),
            &[],
        )
        .await
        .unwrap()
        .unwrap();
    }
    let kinds: Vec<(Kind, bool)> = screens::list(&mut tx, version)
        .await
        .unwrap()
        .iter()
        .map(|s| (s.kind, s.issue_type_id.is_some()))
        .collect();
    assert_eq!(
        kinds,
        [
            (Kind::Creation, false),
            (Kind::Creation, true),
            (Kind::View, false),
            (Kind::Transition, false),
            (Kind::Transition, false),
        ]
    );
    // The type removed: its screen goes.
    infra::issue_types::delete(&mut tx, version, bug, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        screens::find(&mut tx, version, bug_creation)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    // A second general creation screen, or a second for a type: refused.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        screens::insert(
            &mut tx,
            version,
            new_id(),
            Kind::Creation,
            None,
            &labels("Bis", "Again"),
            &[]
        )
        .await
        .unwrap(),
        Err(WriteConflict::Exists)
    );
}

#[tokio::test]
async fn copies_keep_the_screens_and_the_transition_screens() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Software").await;
    let reason = new_id();
    screens::insert(
        &mut tx,
        published,
        reason,
        Kind::Transition,
        None,
        &labels("Motif", "Reason"),
        &[],
    )
    .await
    .unwrap()
    .unwrap();
    let transition = attach(&mut tx, published, Some(reason)).await;
    let original: Vec<Uuid> = screens::list(&mut tx, published)
        .await
        .unwrap()
        .iter()
        .map(|s| s.item_id)
        .collect();
    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let duplicate = configs::duplicate(&mut tx, id, new_id(), &labels("Copie", "Copy"), None, None)
        .await
        .unwrap()
        .unwrap()
        .published
        .id;
    for version in [draft, duplicate] {
        let copied: Vec<Uuid> = screens::list(&mut tx, version)
            .await
            .unwrap()
            .iter()
            .map(|s| s.item_id)
            .collect();
        assert_eq!(copied, original);
        let all = workflows::load(&mut tx, version).await.unwrap();
        let copy = all.transitions.iter().find(|t| t.item_id == transition).unwrap();
        assert_eq!(copy.screen_id, Some(reason));
    }
    // Checked at commit: everything copied holds together.
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_transition_screen_stays_while_a_transition_shows_it() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, version) = config(&mut tx, "Software").await;
    let reason = new_id();
    screens::insert(
        &mut tx,
        version,
        reason,
        Kind::Transition,
        None,
        &labels("Motif", "Reason"),
        &[],
    )
    .await
    .unwrap()
    .unwrap();
    attach(&mut tx, version, Some(reason)).await;
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    screens::delete(&mut tx, version, reason, None)
        .await
        .unwrap()
        .unwrap();
    assert!(tx.commit().await.is_err(), "the transition still shows it");

    // Detached first: it goes.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    attach(&mut tx, version, None).await;
    screens::delete(&mut tx, version, reason, Some(&[1]))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn fields_join_the_general_screens_and_leave_every_screen() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, version) = config(&mut tx, "Software").await;
    let bug = type_id(&mut tx, version, "bug").await;
    let fields = field_ids(&mut tx, version).await;
    let bug_view = new_id();
    screens::insert(
        &mut tx,
        version,
        bug_view,
        Kind::View,
        Some(bug),
        &labels("Bug", "Bug"),
        &fields,
    )
    .await
    .unwrap()
    .unwrap();
    // A new field: at the end of the general screens only.
    let new_field = new_id();
    let mut changed = screens::append_field(&mut tx, version, new_field).await.unwrap();
    changed.sort_unstable();
    let all = screens::list(&mut tx, version).await.unwrap();
    let mut general: Vec<Uuid> = all
        .iter()
        .filter(|s| s.issue_type_id.is_none())
        .map(|s| s.item_id)
        .collect();
    general.sort_unstable();
    assert_eq!(changed, general);
    for screen in &all {
        assert_eq!(
            screen.field_ids.last() == Some(&new_field),
            screen.issue_type_id.is_none(),
            "{:?}",
            screen.kind
        );
    }
    assert!(
        screens::append_field(&mut tx, version, new_field)
            .await
            .unwrap()
            .is_empty()
    );
    // A field removed: from every screen.
    let removed = fields[0];
    assert_eq!(
        screens::forget_field(&mut tx, version, removed)
            .await
            .unwrap()
            .len(),
        3
    );
    for screen in screens::list(&mut tx, version).await.unwrap() {
        assert!(!screen.field_ids.contains(&removed));
    }
}

#[tokio::test]
async fn screens_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Software").await;
    let creation = screens::list(&mut tx, published).await.unwrap()[0].item_id;
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(screens::list(&mut tx, published).await.unwrap().is_empty());
    assert_eq!(
        screens::update(&mut tx, published, creation, None, &labels("Volé", "Stolen"), &[])
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        screens::delete(&mut tx, published, creation, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    let foreign = screens::insert(
        &mut tx,
        published,
        new_id(),
        Kind::Transition,
        None,
        &labels("Volé", "Stolen"),
        &[],
    )
    .await;
    assert!(foreign.is_err());
}
