//! Integration tests on a real PostgreSQL: the issue types of a
//! configuration version (L04-US02). Default types in every configuration,
//! their copy into a draft or a duplicate with the same identities, unique
//! keys, changes, removal from the parents of the others, order, the types
//! a draft removes, and tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::issue_types::{DEFAULT_TYPES, hierarchy_refusal};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issue_types::{self, IssueType, TypeFields, WriteConflict};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use testkit::TestDb;

fn labels(fr: &str, en: &str) -> BTreeMap<String, String> {
    [("fr".to_owned(), fr.to_owned()), ("en".to_owned(), en.to_owned())].into()
}

fn keys(types: &[IssueType]) -> Vec<&str> {
    types.iter().map(|t| t.key.as_str()).collect()
}

/// The parents of `key`, by key.
fn parents_of<'a>(types: &'a [IssueType], key: &str) -> Vec<&'a str> {
    let by_id: BTreeMap<Uuid, &str> = types.iter().map(|t| (t.item_id, t.key.as_str())).collect();
    let mut parents: Vec<&str> = types
        .iter()
        .find(|t| t.key == key)
        .unwrap()
        .parent_ids
        .iter()
        .map(|p| by_id[p])
        .collect();
    parents.sort_unstable();
    parents
}

/// A new configuration of the current tenant; its published version.
async fn config(conn: &mut PgConnection, fr: &str, en: &str) -> (Uuid, Uuid) {
    let config = configs::insert(conn, new_id(), &labels(fr, en), None)
        .await
        .unwrap()
        .unwrap();
    (config.id, config.published.id)
}

#[tokio::test]
async fn every_configuration_starts_with_the_default_types() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    // The tenant's default configuration (seeded with the tenant) and a new one.
    let default = configs::default_id(&mut tx).await.unwrap().unwrap();
    let default_version = configs::find(&mut tx, default)
        .await
        .unwrap()
        .unwrap()
        .published
        .id;
    let (_, created_version) = config(&mut tx, "Développement", "Software").await;
    for version in [default_version, created_version] {
        let types = issue_types::list(&mut tx, version).await.unwrap();
        // In step with the domain's list.
        assert_eq!(keys(&types), DEFAULT_TYPES.map(|t| t.key));
        for (stored, expected) in types.iter().zip(DEFAULT_TYPES) {
            assert_eq!(
                stored.label_translations,
                labels(expected.label_fr, expected.label_en)
            );
            assert_eq!(stored.icon, expected.icon);
            assert_eq!(stored.color, expected.color.as_str());
            assert_eq!(stored.is_subtask, expected.is_subtask);
            let mut parents = expected.parents.to_vec();
            parents.sort_unstable();
            assert_eq!(parents_of(&types, expected.key), parents, "{}", expected.key);
        }
        let nodes: Vec<_> = types.iter().map(IssueType::node).collect();
        assert_eq!(hierarchy_refusal(&nodes, None), None);
    }
    // Each configuration has its own identities.
    let a = issue_types::list(&mut tx, default_version).await.unwrap();
    let b = issue_types::list(&mut tx, created_version).await.unwrap();
    assert!(a.iter().all(|t| b.iter().all(|u| u.item_id != t.item_id)));
}

#[tokio::test]
async fn a_draft_and_a_duplicate_copy_the_types_with_their_identities() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Développement", "Software").await;
    let original = issue_types::list(&mut tx, published).await.unwrap();

    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let copied = issue_types::list(&mut tx, draft).await.unwrap();
    let identity = |types: &[IssueType]| -> Vec<(Uuid, String, Vec<Uuid>, i32)> {
        types
            .iter()
            .map(|t| (t.item_id, t.key.clone(), t.parent_ids.clone(), t.position))
            .collect()
    };
    assert_eq!(identity(&copied), identity(&original));

    let copy = configs::duplicate(&mut tx, id, new_id(), &labels("Copie", "Copy"), None, None)
        .await
        .unwrap()
        .unwrap();
    let duplicated = issue_types::list(&mut tx, copy.published.id).await.unwrap();
    assert_eq!(
        identity(&duplicated),
        identity(&original),
        "not seeded on top of the copy"
    );
}

#[tokio::test]
async fn types_are_added_changed_ordered_and_removed_in_a_version() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Développement", "Software").await;
    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let types = issue_types::list(&mut tx, draft).await.unwrap();
    let epic = types[0].item_id;
    let story = types[1].item_id;
    let standard = types[0].workflow_id.unwrap();

    // Added last.
    let incident = new_id();
    let label = labels("Incident", "Incident");
    let fields = TypeFields {
        label_translations: &label,
        icon: "emergency",
        color: "bug",
        is_subtask: false,
        parent_ids: &[epic],
        workflow_id: standard,
    };
    let added = issue_types::insert(&mut tx, draft, incident, "incident", &fields)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((added.position, added.version), (5, 1));
    let (_, other) = config(&mut tx, "Support", "Support").await;
    let elsewhere = TypeFields {
        workflow_id: issue_types::list(&mut tx, other).await.unwrap()[0]
            .workflow_id
            .unwrap(),
        ..fields.clone()
    };
    assert_eq!(
        issue_types::insert(&mut tx, other, new_id(), "incident", &elsewhere)
            .await
            .unwrap()
            .map(|t| t.key),
        Ok("incident".to_owned()),
        "a key is unique per version"
    );
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        issue_types::insert(&mut tx, draft, new_id(), "incident", &fields)
            .await
            .unwrap(),
        Err(WriteConflict::KeyTaken)
    );
    tx.rollback().await.unwrap();

    // Changed with its version.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let relabel = labels("Incident majeur", "Major incident");
    let changed = TypeFields {
        label_translations: &relabel,
        parent_ids: &[epic, story],
        ..fields.clone()
    };
    let updated = issue_types::update(&mut tx, draft, incident, Some(&[1]), &changed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (updated.version, updated.parent_ids.clone()),
        (2, vec![epic, story])
    );
    assert_eq!(updated.key, "incident");
    assert_eq!(
        issue_types::update(&mut tx, draft, incident, Some(&[1]), &changed)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        issue_types::update(&mut tx, draft, new_id(), None, &changed)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );

    // Ordered: only the ones that move.
    let mut order: Vec<Uuid> = issue_types::list(&mut tx, draft)
        .await
        .unwrap()
        .iter()
        .map(|t| t.item_id)
        .collect();
    order.swap(0, 5);
    let moved = issue_types::reorder(&mut tx, draft, &order).await.unwrap();
    assert_eq!(moved.len(), 2);
    let listed: Vec<Uuid> = issue_types::list(&mut tx, draft)
        .await
        .unwrap()
        .iter()
        .map(|t| t.item_id)
        .collect();
    assert_eq!(listed, order);

    // Removed: out of the parents of the others too.
    issue_types::delete(&mut tx, draft, story, None)
        .await
        .unwrap()
        .unwrap();
    let left = issue_types::list(&mut tx, draft).await.unwrap();
    assert!(
        left.iter()
            .all(|t| t.item_id != story && !t.parent_ids.contains(&story))
    );
    assert_eq!(parents_of(&left, "incident"), ["epic"]);
    assert_eq!(parents_of(&left, "subtask"), ["bug", "task"]);
    assert_eq!(
        issue_types::delete(&mut tx, draft, story, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );

    // What the draft removes from the published version: Story.
    let removed = issue_types::removed(&mut tx, published, draft).await.unwrap();
    assert_eq!(keys(&removed), ["story"]);
    // The published version is untouched.
    assert_eq!(issue_types::list(&mut tx, published).await.unwrap().len(), 5);
}

#[tokio::test]
async fn types_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Développement", "Software").await;
    let types = issue_types::list(&mut tx, published).await.unwrap();
    let (bug, standard) = (types[3].item_id, types[3].workflow_id.unwrap());
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(issue_types::list(&mut tx, published).await.unwrap().is_empty());
    assert_eq!(issue_types::find(&mut tx, published, bug).await.unwrap(), None);
    let label = labels("Volé", "Stolen");
    let fields = TypeFields {
        label_translations: &label,
        icon: "bug_report",
        color: "bug",
        is_subtask: false,
        parent_ids: &[],
        workflow_id: standard,
    };
    assert_eq!(
        issue_types::update(&mut tx, published, bug, None, &fields)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        issue_types::delete(&mut tx, published, bug, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    // Not even into another tenant's version: its foreign key is checked with the tenant.
    let foreign = issue_types::insert(&mut tx, published, new_id(), "stolen", &fields).await;
    assert!(foreign.is_err());
}
