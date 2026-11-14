//! Integration tests on a real PostgreSQL: the workflows of a
//! configuration version (L04-US04). The standard workflow in every
//! configuration, followed by its types; copies into a draft keeping
//! identities, a workflow duplicated with identities of its own; unique
//! identifiers; one initial status; a status removed with its transitions;
//! the statuses a draft removes; tenant isolation; transition rules kept in
//! copies, leaving a removed field (L04-US05).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::workflows::{STANDARD_KEY, STANDARD_STATUSES, STANDARD_TRANSITIONS, problems};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issue_types;
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use infra::workflows::{self, StatusFields, TransitionFields, Workflows, WriteConflict};
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

/// The statuses of `workflow`, by key.
fn status_keys(all: &Workflows, workflow: Uuid) -> Vec<String> {
    all.statuses_of(workflow).map(|s| s.key.clone()).collect()
}

/// The transitions of `workflow` as (English name, from key or `*`, to key).
fn transitions(all: &Workflows, workflow: Uuid) -> Vec<(String, String, String)> {
    let key = |id: Uuid| all.statuses.iter().find(|s| s.item_id == id).unwrap().key.clone();
    all.transitions_of(workflow)
        .map(|t| {
            (
                t.name_translations["en"].clone(),
                t.from_status_id.map_or_else(|| "*".to_owned(), key),
                key(t.to_status_id),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_configuration_starts_with_the_standard_workflow_its_types_follow() {
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
        let all = workflows::load(&mut tx, version).await.unwrap();
        assert_eq!(all.workflows.len(), 1);
        let standard = &all.workflows[0];
        assert_eq!(standard.key, STANDARD_KEY);
        assert_eq!(
            standard.label_translations,
            labels("Workflow standard", "Standard workflow")
        );
        // In step with the domain's lists.
        let statuses: Vec<_> = all.statuses_of(standard.item_id).collect();
        assert_eq!(statuses.len(), STANDARD_STATUSES.len());
        for (stored, expected) in statuses.iter().zip(STANDARD_STATUSES) {
            assert_eq!(stored.key, expected.key);
            assert_eq!(
                stored.label_translations,
                labels(expected.label_fr, expected.label_en)
            );
            assert_eq!(
                stored.portal_label_translations,
                labels(expected.portal_fr, expected.portal_en)
            );
            assert_eq!(stored.category, expected.category.as_str());
            assert_eq!(stored.is_initial, expected.is_initial);
        }
        let expected: Vec<(String, String, String)> = STANDARD_TRANSITIONS
            .iter()
            .map(|t| {
                (
                    t.name_en.to_owned(),
                    t.from.unwrap_or("*").to_owned(),
                    t.to.to_owned(),
                )
            })
            .collect();
        assert_eq!(transitions(&all, standard.item_id), expected);
        for t in STANDARD_TRANSITIONS {
            let stored = all
                .transitions_of(standard.item_id)
                .find(|s| s.name_translations["en"] == t.name_en)
                .unwrap();
            assert_eq!(stored.name_translations, labels(t.name_fr, t.name_en));
        }
        let nodes: Vec<_> = statuses.iter().map(|s| s.node()).collect();
        let edges: Vec<_> = all
            .transitions_of(standard.item_id)
            .map(workflows::Transition::edge)
            .collect();
        assert_eq!(problems(&nodes, &edges), []);
        // Every type follows it.
        let types = issue_types::list(&mut tx, version).await.unwrap();
        assert_eq!(types.len(), 5);
        assert!(types.iter().all(|t| t.workflow_id == Some(standard.item_id)));
    }
}

#[tokio::test]
async fn a_draft_keeps_the_identities_and_a_duplicate_gets_its_own() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Software").await;
    let original = workflows::load(&mut tx, published).await.unwrap();
    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let copied = workflows::load(&mut tx, draft).await.unwrap();
    let ids = |all: &Workflows| -> (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) {
        (
            all.workflows.iter().map(|w| w.item_id).collect(),
            all.statuses.iter().map(|s| s.item_id).collect(),
            all.transitions.iter().map(|t| t.item_id).collect(),
        )
    };
    assert_eq!(ids(&copied), ids(&original));
    assert_eq!(
        issue_types::list(&mut tx, draft).await.unwrap()[0].workflow_id,
        Some(copied.workflows[0].item_id)
    );

    // Duplicated within the draft: the same statuses and transitions, its own identities.
    let standard = copied.workflows[0].item_id;
    let bug = new_id();
    workflows::duplicate_workflow(
        &mut tx,
        draft,
        standard,
        bug,
        "bug",
        &labels("Workflow bug", "Bug workflow"),
    )
    .await
    .unwrap()
    .unwrap();
    let both = workflows::load(&mut tx, draft).await.unwrap();
    assert_eq!(both.workflows.len(), 2);
    assert_eq!(status_keys(&both, bug), status_keys(&both, standard));
    assert_eq!(transitions(&both, bug), transitions(&both, standard));
    let shared: Vec<_> = both
        .statuses_of(bug)
        .filter(|s| both.statuses_of(standard).any(|o| o.item_id == s.item_id))
        .collect();
    assert!(shared.is_empty(), "identities of its own");
    assert!(
        both.transitions_of(bug)
            .all(|t| both.statuses_of(bug).any(|s| s.item_id == t.to_status_id))
    );
}

#[tokio::test]
async fn statuses_are_added_changed_and_removed_with_their_transitions() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Software").await;
    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let all = workflows::load(&mut tx, draft).await.unwrap();
    let standard = all.workflows[0].item_id;
    let status = |key: &str| all.statuses.iter().find(|s| s.key == key).unwrap().clone();
    let (todo, review) = (status("todo"), status("in_review"));

    // Added last; made initial, it takes the place of To do.
    let blocked = new_id();
    let label = labels("Bloqué", "Blocked");
    let empty = BTreeMap::new();
    let fields = StatusFields {
        label_translations: &label,
        portal_label_translations: &empty,
        category: "in_progress",
        is_initial: true,
    };
    workflows::insert_status(&mut tx, draft, standard, blocked, "blocked", &fields)
        .await
        .unwrap()
        .unwrap();
    let after = workflows::load(&mut tx, draft).await.unwrap();
    assert_eq!(
        status_keys(&after, standard).last().map(String::as_str),
        Some("blocked")
    );
    let initials: Vec<&str> = after
        .statuses
        .iter()
        .filter(|s| s.is_initial)
        .map(|s| s.key.as_str())
        .collect();
    assert_eq!(initials, ["blocked"]);
    // Back to To do, with the version it was read at.
    let back = StatusFields {
        label_translations: &todo.label_translations,
        portal_label_translations: &todo.portal_label_translations,
        category: "todo",
        is_initial: true,
    };
    assert_eq!(
        workflows::update_status(&mut tx, draft, todo.item_id, Some(&[todo.version]), &back)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch),
        "it lost its initial flag meanwhile"
    );
    workflows::update_status(&mut tx, draft, todo.item_id, None, &back)
        .await
        .unwrap()
        .unwrap();
    let after = workflows::load(&mut tx, draft).await.unwrap();
    let initials: Vec<&str> = after
        .statuses
        .iter()
        .filter(|s| s.is_initial)
        .map(|s| s.key.as_str())
        .collect();
    assert_eq!(initials, ["todo"]);

    // A transition from any status to it; then the status goes, with its transitions.
    let name = labels("Bloquer", "Block");
    workflows::insert_transition(
        &mut tx,
        draft,
        standard,
        new_id(),
        &TransitionFields {
            rules: None,
            name_translations: &name,
            from_status_id: None,
            to_status_id: blocked,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        workflows::load(&mut tx, draft).await.unwrap().transitions.len(),
        6
    );
    workflows::delete_status(&mut tx, draft, review.item_id, None)
        .await
        .unwrap()
        .unwrap();
    let after = workflows::load(&mut tx, draft).await.unwrap();
    let kept: Vec<(String, String, String)> = transitions(&after, standard);
    assert_eq!(
        kept,
        [
            ("Start".to_owned(), "todo".to_owned(), "in_progress".to_owned()),
            ("Cancel".to_owned(), "*".to_owned(), "cancelled".to_owned()),
            ("Block".to_owned(), "*".to_owned(), "blocked".to_owned()),
        ]
    );
    assert_eq!(
        workflows::delete_status(&mut tx, draft, review.item_id, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );

    // What the draft removes from the published version: In review.
    let removed = workflows::removed_statuses(&mut tx, published, draft)
        .await
        .unwrap();
    let removed: Vec<(&str, &str)> = removed
        .iter()
        .map(|(s, workflow)| (s.key.as_str(), workflow["en"].as_str()))
        .collect();
    assert_eq!(removed, [("in_review", "Standard workflow")]);
}

#[tokio::test]
async fn identifiers_are_unique_per_version_and_per_workflow() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, _) = config(&mut tx, "Software").await;
    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    let standard = workflows::load(&mut tx, draft).await.unwrap().workflows[0].item_id;
    let bug = new_id();
    workflows::insert_workflow(
        &mut tx,
        draft,
        bug,
        "bug",
        &labels("Workflow bug", "Bug workflow"),
    )
    .await
    .unwrap()
    .unwrap();
    // A status key of another workflow is free.
    let label = labels("À faire", "To do");
    let empty = BTreeMap::new();
    let fields = StatusFields {
        label_translations: &label,
        portal_label_translations: &empty,
        category: "todo",
        is_initial: true,
    };
    workflows::insert_status(&mut tx, draft, bug, new_id(), "todo", &fields)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        workflows::insert_workflow(&mut tx, draft, new_id(), "bug", &labels("Autre", "Other"))
            .await
            .unwrap(),
        Err(WriteConflict::KeyTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        workflows::insert_status(&mut tx, draft, standard, new_id(), "todo", &fields)
            .await
            .unwrap(),
        Err(WriteConflict::KeyTaken)
    );
}

#[tokio::test]
async fn workflows_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Software").await;
    let all = workflows::load(&mut tx, published).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(
        workflows::load(&mut tx, published).await.unwrap(),
        Workflows::default()
    );
    let standard = all.workflows[0].item_id;
    assert_eq!(
        workflows::update_workflow(&mut tx, published, standard, None, &labels("Volé", "Stolen"))
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        workflows::delete_status(&mut tx, published, all.statuses[0].item_id, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        workflows::delete_transition(&mut tx, published, all.transitions[0].item_id, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    // Not even into another tenant's version: its foreign keys are checked with the tenant.
    let foreign =
        workflows::insert_workflow(&mut tx, published, new_id(), "stolen", &labels("Volé", "Stolen")).await;
    assert!(foreign.is_err());
}

/// The field `key` of `version`.
async fn field(conn: &mut PgConnection, version: Uuid, key: &str) -> Uuid {
    infra::fields::list(conn, version)
        .await
        .unwrap()
        .into_iter()
        .find(|field| field.key == key)
        .unwrap()
        .item_id
}

#[tokio::test]
async fn transition_rules_are_kept_in_copies_and_leave_a_removed_field() {
    use domain::fields::Value;
    use domain::transitions::{AssignTo, Condition, PostFunction, Rules};

    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Software").await;
    let all = workflows::load(&mut tx, published).await.unwrap();
    let standard = all.workflows[0].item_id;
    let start = all.transitions[0].clone();
    assert_eq!(start.rules, Rules::default(), "no rules at first");
    let points = field(&mut tx, published, "story_points").await;
    let remaining = field(&mut tx, published, "remaining").await;
    let rules = Rules {
        condition: Condition {
            roles: vec![1],
            groups: vec![2],
        },
        validators: vec![remaining.as_u128(), points.as_u128()],
        post_functions: vec![
            PostFunction::Assign(AssignTo::CurrentUser),
            PostFunction::SetField {
                field: points.as_u128(),
                value: Value::Number(3.0),
            },
            PostFunction::AddComment {
                text: "Started".into(),
            },
        ],
    };
    let set = |rules| TransitionFields {
        rules,
        name_translations: &start.name_translations,
        from_status_id: start.from_status_id,
        to_status_id: start.to_status_id,
    };
    workflows::update_transition(&mut tx, published, start.item_id, None, &set(Some(&rules)))
        .await
        .unwrap()
        .unwrap();
    // Changed without its rules: they stay.
    workflows::update_transition(&mut tx, published, start.item_id, None, &set(None))
        .await
        .unwrap()
        .unwrap();
    let rules_of =
        |all: &Workflows, workflow: Uuid| all.transitions_of(workflow).next().unwrap().rules.clone();
    assert_eq!(
        rules_of(&workflows::load(&mut tx, published).await.unwrap(), standard),
        rules
    );

    // Into a draft, and a workflow duplicated there.
    let draft = configs::open_draft(&mut tx, id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    assert_eq!(
        rules_of(&workflows::load(&mut tx, draft).await.unwrap(), standard),
        rules
    );
    let copy = new_id();
    workflows::duplicate_workflow(&mut tx, draft, standard, copy, "copy", &labels("Copie", "Copy"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rules_of(&workflows::load(&mut tx, draft).await.unwrap(), copy),
        rules
    );

    // Points leaves the draft: its validator and its post-function go.
    let mut changed = workflows::forget_field(&mut tx, draft, points).await.unwrap();
    changed.sort_unstable();
    let mut expected = vec![standard, copy];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    let after = rules_of(&workflows::load(&mut tx, draft).await.unwrap(), standard);
    assert_eq!(after.validators, [remaining.as_u128()]);
    assert_eq!(after.post_functions.len(), 2);
    assert_eq!(after.condition, rules.condition);
    assert!(
        workflows::forget_field(&mut tx, draft, points)
            .await
            .unwrap()
            .is_empty()
    );
    // The published version keeps them.
    assert_eq!(
        rules_of(&workflows::load(&mut tx, published).await.unwrap(), standard),
        rules
    );
}
