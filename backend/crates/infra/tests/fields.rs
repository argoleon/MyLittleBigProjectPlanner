//! Integration tests on a real PostgreSQL: the fields of a configuration
//! version and their rules (L04-US03). System fields in every
//! configuration, in step with the domain; copies into a draft and a
//! duplicate keeping identities and rules; unique identifiers and cells;
//! deactivated list values still readable; rules following the types and
//! statuses they name; tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::fields::{Kind, ListValue, Mode, Options, Rule, SYSTEM_FIELDS, Step, Unit};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::fields::{self, Field, FieldFields, WriteConflict};
use infra::id::new_id;
use infra::issue_types::{self, IssueType};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use infra::workflows::{self, StatusFields};
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

/// The draft of `config`, opened.
async fn draft(conn: &mut PgConnection, config: Uuid) -> Uuid {
    configs::open_draft(conn, config, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id
}

fn type_id(types: &[IssueType], key: &str) -> Uuid {
    types.iter().find(|t| t.key == key).unwrap().item_id
}

fn value(id: Uuid, fr: &str, en: &str) -> ListValue {
    ListValue {
        id: id.as_u128(),
        labels: labels(fr, en),
        active: true,
        children: Vec::new(),
    }
}

/// Application, a single select: Directory, Mail, ERP.
fn application(values: &[Uuid; 3]) -> Options {
    Options {
        values: Some(vec![
            value(values[0], "Annuaire", "Directory"),
            value(values[1], "Messagerie", "Mail"),
            value(values[2], "ERP", "ERP"),
        ]),
        ..Options::default()
    }
}

/// Adds the field Application on `types` to `version`.
async fn add_application(conn: &mut PgConnection, version: Uuid, types: &[Uuid], values: &[Uuid; 3]) -> Uuid {
    let item = new_id();
    let options = application(values);
    let fields = FieldFields {
        label_translations: &labels("Application", "Application"),
        options: &options,
        all_types: false,
        issue_type_ids: types,
    };
    fields::insert(conn, version, item, "application", Kind::SingleSelect, &fields)
        .await
        .unwrap()
        .unwrap();
    item
}

fn find(all: &[Field], key: &str) -> Field {
    all.iter().find(|f| f.key == key).unwrap().clone()
}

#[tokio::test]
async fn every_configuration_starts_with_the_system_fields() {
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
        let types = issue_types::list(&mut tx, version).await.unwrap();
        let all = fields::list(&mut tx, version).await.unwrap();
        // In step with the domain's list.
        assert_eq!(all.len(), SYSTEM_FIELDS.len());
        for (stored, expected) in all.iter().zip(SYSTEM_FIELDS) {
            assert_eq!(stored.key, expected.key);
            assert_eq!(
                stored.label_translations,
                labels(expected.label_fr, expected.label_en)
            );
            assert_eq!(stored.kind, expected.kind);
            assert_eq!(stored.options.decimals, Some(expected.decimals));
            assert_eq!(stored.options.unit, expected.unit);
            assert!(stored.is_system);
            assert_eq!(stored.all_types, expected.all_types);
            let keys: Vec<&str> = stored
                .issue_type_ids
                .iter()
                .map(|id| types.iter().find(|t| t.item_id == *id).unwrap().key.as_str())
                .collect();
            assert_eq!(keys, expected.type_keys);
            assert!(stored.rules.is_empty(), "visible everywhere");
        }
        assert_eq!(find(&all, "remaining").options.unit, Some(Unit::Hours));
    }
}

#[tokio::test]
async fn a_draft_and_a_duplicate_keep_the_fields_with_their_rules() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (id, published) = config(&mut tx, "Software").await;
    let types = issue_types::list(&mut tx, published).await.unwrap();
    let all_workflows = workflows::load(&mut tx, published).await.unwrap();
    let done = all_workflows
        .statuses
        .iter()
        .find(|s| s.key == "done")
        .unwrap()
        .item_id;
    let (bug, task) = (type_id(&types, "bug"), type_id(&types, "task"));
    let values = [new_id(), new_id(), new_id()];
    let item = add_application(&mut tx, published, &[bug, task], &values).await;
    // Mockup 12: Bug required at creation, Task hidden there, Bug required when done.
    let rules = [
        Rule {
            issue_type: bug.as_u128(),
            step: Step::Creation,
            mode: Mode::Required,
        },
        Rule {
            issue_type: task.as_u128(),
            step: Step::Creation,
            mode: Mode::Hidden,
        },
        Rule {
            issue_type: bug.as_u128(),
            step: Step::Status(done.as_u128()),
            mode: Mode::Required,
        },
        // Not stored.
        Rule {
            issue_type: task.as_u128(),
            step: Step::Status(done.as_u128()),
            mode: Mode::Visible,
        },
    ];
    fields::set_rules(&mut tx, published, item, &rules).await.unwrap();
    let original = fields::find(&mut tx, published, item).await.unwrap().unwrap();
    assert_eq!(original.rules.len(), 3);
    // In the order of the types (Task before Bug), the creation first.
    assert_eq!(original.rules[..2], [rules[1], rules[0]]);

    let draft = draft(&mut tx, id).await;
    let copied = fields::list(&mut tx, draft).await.unwrap();
    assert_eq!(find(&copied, "application"), original);
    assert_eq!(copied.len(), SYSTEM_FIELDS.len() + 1);

    let duplicate = configs::duplicate(&mut tx, id, new_id(), &labels("Copie", "Copy"), None, None)
        .await
        .unwrap()
        .unwrap();
    let duplicated = fields::list(&mut tx, duplicate.published.id).await.unwrap();
    assert_eq!(find(&duplicated, "application").rules, original.rules);
}

#[tokio::test]
async fn identifiers_and_cells_are_unique_per_version() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Software").await;
    let (_, other) = config(&mut tx, "Support").await;
    add_application(&mut tx, published, &[], &[new_id(), new_id(), new_id()]).await;
    // The same identifier in another configuration.
    add_application(&mut tx, other, &[], &[new_id(), new_id(), new_id()]).await;
    let types = issue_types::list(&mut tx, published).await.unwrap();
    let remaining = find(&fields::list(&mut tx, published).await.unwrap(), "remaining");
    tx.commit().await.unwrap();

    // A refused write ends its transaction: one each.
    let options = Options::default();
    let fields = FieldFields {
        label_translations: &labels("Autre", "Other"),
        options: &options,
        all_types: true,
        issue_type_ids: &[],
    };
    for key in ["application", "story_points"] {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        assert_eq!(
            fields::insert(&mut tx, published, new_id(), key, Kind::ShortText, &fields)
                .await
                .unwrap(),
            Err(WriteConflict::KeyTaken),
            "{key}"
        );
    }

    // One rule per cell, creation included.
    let cell = Rule {
        issue_type: type_id(&types, "bug").as_u128(),
        step: Step::Creation,
        mode: Mode::Hidden,
    };
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let twice = fields::set_rules(&mut tx, published, remaining.item_id, &[cell, cell]).await;
    assert!(twice.is_err());
}

#[tokio::test]
async fn deactivated_list_values_stay_readable() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Software").await;
    let values = [new_id(), new_id(), new_id()];
    let item = add_application(&mut tx, published, &[], &values).await;
    let mut options = application(&values);
    options.values.as_mut().unwrap()[2].active = false;
    let fields = FieldFields {
        label_translations: &labels("Application", "Application"),
        options: &options,
        all_types: true,
        issue_type_ids: &[],
    };
    fields::update(&mut tx, published, item, Some(&[1]), &fields)
        .await
        .unwrap()
        .unwrap();
    let stored = fields::find(&mut tx, published, item).await.unwrap().unwrap();
    assert_eq!(stored.version, 2);
    let erp = stored.options.value(values[2].as_u128()).unwrap();
    assert!(!erp.active);
    assert_eq!(erp.labels, labels("ERP", "ERP"));
    assert!(stored.options.value(values[0].as_u128()).unwrap().active);
    // A stale version is refused.
    assert_eq!(
        fields::update(&mut tx, published, item, Some(&[1]), &fields)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
}

#[tokio::test]
async fn rules_follow_the_types_and_statuses_they_name() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, version) = config(&mut tx, "Software").await;
    let types = issue_types::list(&mut tx, version).await.unwrap();
    let all = workflows::load(&mut tx, version).await.unwrap();
    let standard = all.workflows[0].item_id;
    let status = |key: &str| all.statuses.iter().find(|s| s.key == key).unwrap().item_id;
    let (bug, task) = (type_id(&types, "bug"), type_id(&types, "task"));
    let item = add_application(&mut tx, version, &[bug, task], &[new_id(), new_id(), new_id()]).await;
    let cell = |issue_type: Uuid, status: Option<Uuid>| Rule {
        issue_type: issue_type.as_u128(),
        step: status.map_or(Step::Creation, |s| Step::Status(s.as_u128())),
        mode: Mode::Required,
    };
    fields::set_rules(
        &mut tx,
        version,
        item,
        &[
            cell(bug, None),
            cell(bug, Some(status("in_review"))),
            cell(bug, Some(status("done"))),
            cell(task, Some(status("done"))),
        ],
    )
    .await
    .unwrap();

    // A status removed takes its cells.
    workflows::delete_status(&mut tx, version, status("in_review"), None)
        .await
        .unwrap()
        .unwrap();
    let field = fields::find(&mut tx, version, item).await.unwrap().unwrap();
    assert_eq!(field.rules.len(), 3);

    // Bug moves to another workflow: its cells on the standard statuses go,
    // the creation stays.
    let other = new_id();
    workflows::insert_workflow(&mut tx, version, other, "bug_flow", &labels("Bugs", "Bugs"))
        .await
        .unwrap()
        .unwrap();
    workflows::insert_status(
        &mut tx,
        version,
        other,
        new_id(),
        "open",
        &StatusFields {
            label_translations: &labels("Ouvert", "Open"),
            portal_label_translations: &BTreeMap::new(),
            category: "todo",
            is_initial: true,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        fields::prune_rules(&mut tx, version, bug, other).await.unwrap(),
        [item]
    );
    assert!(
        fields::prune_rules(&mut tx, version, task, standard)
            .await
            .unwrap()
            .is_empty()
    );
    let field = fields::find(&mut tx, version, item).await.unwrap().unwrap();
    assert_eq!(field.rules, [cell(task, Some(status("done"))), cell(bug, None)]);

    // A type removed takes its cells and leaves the types of the field.
    issue_types::delete(&mut tx, version, task, None)
        .await
        .unwrap()
        .unwrap();
    // Application and Points listed it.
    let points = find(&fields::list(&mut tx, version).await.unwrap(), "story_points");
    let mut changed = fields::forget_type(&mut tx, version, task).await.unwrap();
    changed.sort_unstable();
    let mut expected = vec![item, points.item_id];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    let field = fields::find(&mut tx, version, item).await.unwrap().unwrap();
    assert_eq!(field.issue_type_ids, [bug]);
    assert_eq!(field.rules, [cell(bug, None)]);

    // A field removed takes its rules.
    fields::delete(&mut tx, version, item, Some(&[2]))
        .await
        .unwrap()
        .unwrap();
    assert!(fields::find(&mut tx, version, item).await.unwrap().is_none());
    assert_eq!(
        fields::delete(&mut tx, version, item, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
}

#[tokio::test]
async fn fields_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (_, published) = config(&mut tx, "Software").await;
    let item = add_application(&mut tx, published, &[], &[new_id(), new_id(), new_id()]).await;
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(fields::list(&mut tx, published).await.unwrap().is_empty());
    let options = Options::default();
    let stolen = FieldFields {
        label_translations: &labels("Volé", "Stolen"),
        options: &options,
        all_types: true,
        issue_type_ids: &[],
    };
    assert_eq!(
        fields::update(&mut tx, published, item, None, &stolen)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        fields::delete(&mut tx, published, item, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    // Not even into another tenant's version: its foreign keys are checked with the tenant.
    let foreign = fields::insert(&mut tx, published, new_id(), "stolen", Kind::Checkbox, &stolen).await;
    assert!(foreign.is_err());
}
