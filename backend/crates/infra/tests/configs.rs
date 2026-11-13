//! Integration tests on a real PostgreSQL: project configurations
//! (L04-US01). The tenant's default configuration and the one
//! configuration of every project, the draft and its publication in one
//! transaction, the counting of changes, duplication and detachment, and
//! tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::configuration::{Item, ItemKind};
use domain::hierarchy::{Health, NodeType};
use infra::accounts::TenantRole;
use infra::configs::{self, WriteConflict};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

fn name(fr: &str, en: &str) -> BTreeMap<String, String> {
    [("fr".to_owned(), fr.to_owned()), ("en".to_owned(), en.to_owned())].into()
}

async fn admin(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    db.add_member(tenant, TenantRole::TenantAdmin).await.account_id
}

async fn node(
    conn: &mut PgConnection,
    node_type: NodeType,
    name: &str,
    key: Option<&str>,
    creator: Uuid,
) -> Uuid {
    let id = new_id();
    nodes::insert(
        conn,
        &NewNode {
            id,
            node_type,
            key,
            fields: NodeFields {
                name,
                description: "",
                objectives: "",
                start_date: None,
                end_date: None,
                health: Health::OnTrack,
                owners: &[],
            },
            parent: None,
            created_by: creator,
        },
    )
    .await
    .unwrap()
    .unwrap();
    id
}

/// The states of the versions of `config`, by number.
async fn versions(conn: &mut PgConnection, config: Uuid) -> Vec<(i32, String)> {
    sqlx::query_as("SELECT number, state FROM project_config_version WHERE config_id = $1 ORDER BY number")
        .bind(config)
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_tenant_has_a_default_configuration_that_new_projects_get() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();

    // One per tenant, published, version 1, and listed first.
    let default = configs::default_id(&mut tx).await.unwrap().unwrap();
    let page = configs::list(&mut tx, None, 50).await.unwrap();
    assert_eq!(page.items.len(), 1);
    let config = &page.items[0];
    assert_eq!(config.id, default);
    assert!(config.is_default);
    assert_eq!(
        config.name_translations,
        name("Configuration par défaut", "Default configuration")
    );
    assert_eq!(config.published.number, 1);
    assert_eq!(config.draft, None);

    // A project gets it; a portfolio or a program has none.
    let plat = node(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        creator,
    )
    .await;
    let program = node(&mut tx, NodeType::Program, "IT Modernization", None, creator).await;
    assert_eq!(configs::of_project(&mut tx, plat).await.unwrap(), Some(default));
    assert_eq!(configs::of_project(&mut tx, program).await.unwrap(), None);
    assert_eq!(
        configs::find(&mut tx, default)
            .await
            .unwrap()
            .unwrap()
            .project_count,
        1
    );

    // Exactly one: never none for a project, never one for a program.
    let none = sqlx::query("UPDATE node SET config_id = NULL WHERE id = $1")
        .bind(plat)
        .execute(&mut *tx)
        .await;
    assert!(none.is_err(), "a project keeps a configuration");
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let program = node(&mut tx, NodeType::Program, "IT Modernization", None, creator).await;
    let some = sqlx::query("UPDATE node SET config_id = $2 WHERE id = $1")
        .bind(program)
        .bind(default)
        .execute(&mut *tx)
        .await;
    assert!(some.is_err(), "a program has none");
    tx.rollback().await.unwrap();

    // A new tenant is seeded too.
    let tenant = new_id();
    db.exec_as_owner(&format!(
        "INSERT INTO tenant (id, slug, name) VALUES ('{tenant}', 'gamma', 'Gamma')"
    ))
    .await;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    assert!(configs::default_id(&mut tx).await.unwrap().is_some());
}

#[tokio::test]
async fn configurations_are_named_once_per_tenant_and_language() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let software = configs::insert(
        &mut tx,
        new_id(),
        &name(" Développement logiciel ", "Software development"),
        Some(creator),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        software.name_translations,
        name("Développement logiciel", "Software development")
    );
    assert_eq!((software.project_count, software.published.number), (0, 1));
    assert!(!software.is_default);
    let support = configs::insert(&mut tx, new_id(), &name("Support", "Support"), None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    // Whatever the case, in either language. A conflict ends the
    // transaction, as it ends the request.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        configs::insert(&mut tx, new_id(), &name("Support", "SOFTWARE DEVELOPMENT"), None)
            .await
            .unwrap(),
        Err(WriteConflict::NameTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        configs::rename(
            &mut tx,
            support.id,
            None,
            &name("développement logiciel", "Help desk")
        )
        .await
        .unwrap(),
        Err(WriteConflict::NameTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let renamed = configs::rename(
        &mut tx,
        support.id,
        Some(&[support.version]),
        &name("Support", "Help desk"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(renamed.version, support.version + 1);
    assert_eq!(
        configs::rename(
            &mut tx,
            support.id,
            Some(&[support.version]),
            &name("Support", "Desk")
        )
        .await
        .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        configs::rename(&mut tx, new_id(), None, &name("Support", "Desk"))
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    tx.commit().await.unwrap();

    // Another tenant may use the same name.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        configs::insert(
            &mut tx,
            new_id(),
            &name("Développement logiciel", "Software development"),
            None
        )
        .await
        .unwrap()
        .is_ok()
    );
}

#[tokio::test]
async fn a_draft_counts_each_changed_item_once_and_is_published_in_one_go() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let config = configs::insert(
        &mut tx,
        new_id(),
        &name("Développement", "Software"),
        Some(creator),
    )
    .await
    .unwrap()
    .unwrap();

    // No draft: nothing to publish or discard.
    assert_eq!(
        configs::publish(&mut tx, config.id, None).await.unwrap(),
        Err(WriteConflict::NoDraft)
    );
    assert_eq!(
        configs::discard_draft(&mut tx, config.id, None).await.unwrap(),
        Err(WriteConflict::NoDraft)
    );

    // Opened once, shared: opening again returns the same draft.
    let opened = configs::open_draft(&mut tx, config.id, Some(creator))
        .await
        .unwrap()
        .unwrap();
    let draft = opened.draft.clone().unwrap();
    assert_eq!((draft.number, draft.change_count), (2, 0));
    assert_eq!(opened.version, config.version + 1);
    let again = configs::open_draft(&mut tx, config.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.draft.as_ref().map(|d| d.id), Some(draft.id));
    assert_eq!(again.version, opened.version);

    // Each item once, however many edits; each new change is a new version.
    let bug = Item {
        kind: ItemKind::IssueType,
        id: 1,
    };
    let standard = Item {
        kind: ItemKind::Workflow,
        id: 1,
    };
    assert!(configs::record_change(&mut tx, draft.id, bug).await.unwrap());
    assert!(!configs::record_change(&mut tx, draft.id, bug).await.unwrap());
    assert!(configs::record_change(&mut tx, draft.id, standard).await.unwrap());
    let edited = configs::find(&mut tx, config.id).await.unwrap().unwrap();
    assert_eq!(edited.draft.as_ref().map(|d| d.change_count), Some(2));
    assert_eq!(edited.version, opened.version + 2);
    // Not on a published version.
    assert!(
        !configs::record_change(&mut tx, config.published.id, bug)
            .await
            .unwrap()
    );

    // Published with a stale version: nothing moves.
    assert_eq!(
        configs::publish(&mut tx, config.id, Some(&[opened.version]))
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        versions(&mut tx, config.id).await,
        [(1, "published".to_owned()), (2, "draft".to_owned())]
    );

    // Published: the draft is the version of every project at once.
    let published = configs::publish(&mut tx, config.id, Some(&[edited.version]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(published.published.id, draft.id);
    assert_eq!(published.published.number, 2);
    assert_eq!(published.draft, None);
    assert_eq!(
        versions(&mut tx, config.id).await,
        [(1, "retired".to_owned()), (2, "published".to_owned())]
    );

    // A new draft starts with no change; discarded, the published one stays.
    let reopened = configs::open_draft(&mut tx, config.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reopened.draft.as_ref().map(|d| (d.number, d.change_count)),
        Some((3, 0))
    );
    let discarded = configs::discard_draft(&mut tx, config.id, Some(&[reopened.version]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(discarded.draft, None);
    assert_eq!(discarded.published.number, 2);
    assert_eq!(
        versions(&mut tx, config.id).await,
        [(1, "retired".to_owned()), (2, "published".to_owned())]
    );

    // A failure in the transaction leaves the published version as it was.
    configs::open_draft(&mut tx, config.id, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    configs::publish(&mut tx, config.id, None).await.unwrap().unwrap();
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let after = configs::find(&mut tx, config.id).await.unwrap().unwrap();
    assert_eq!(after.published.number, 2);
    // A discarded draft leaves no trace: the next one takes its number.
    assert_eq!(after.draft.as_ref().map(|d| d.number), Some(3));
}

#[tokio::test]
async fn a_project_detaches_by_duplicating_its_configuration() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let shared = configs::insert(
        &mut tx,
        new_id(),
        &name("Développement", "Software"),
        Some(creator),
    )
    .await
    .unwrap()
    .unwrap();
    let plat = node(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        creator,
    )
    .await;
    let cyber = node(
        &mut tx,
        NodeType::Project,
        "Cyber permissions",
        Some("CYBP"),
        creator,
    )
    .await;
    for project in [plat, cyber] {
        assert!(configs::assign(&mut tx, project, shared.id).await.unwrap());
    }
    let program = node(&mut tx, NodeType::Program, "IT Modernization", None, creator).await;
    assert!(!configs::assign(&mut tx, program, shared.id).await.unwrap());
    let names: Vec<String> = configs::projects(&mut tx, shared.id)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Cyber permissions", "Identity platform"]);

    // The draft of the original stays with it.
    configs::open_draft(&mut tx, shared.id, None)
        .await
        .unwrap()
        .unwrap();
    let copy = configs::duplicate(
        &mut tx,
        shared.id,
        new_id(),
        &name("Développement (PLAT)", "Software (PLAT)"),
        Some(plat),
        Some(creator),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!((copy.project_count, copy.published.number), (1, 1));
    assert_eq!(copy.draft, None);
    assert!(!copy.is_default);
    assert_eq!(configs::of_project(&mut tx, plat).await.unwrap(), Some(copy.id));
    assert_eq!(
        configs::of_project(&mut tx, cyber).await.unwrap(),
        Some(shared.id)
    );
    let original = configs::find(&mut tx, shared.id).await.unwrap().unwrap();
    assert_eq!(original.project_count, 1);
    assert!(original.draft.is_some());

    // Publishing the original no longer reaches the detached project.
    configs::publish(&mut tx, shared.id, None).await.unwrap().unwrap();
    assert_eq!(
        configs::find(&mut tx, copy.id)
            .await
            .unwrap()
            .unwrap()
            .published
            .number,
        1
    );

    assert_eq!(
        configs::duplicate(&mut tx, new_id(), new_id(), &name("Copie", "Copy"), None, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    tx.commit().await.unwrap();

    // A duplicate without a project; names stay unique.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        configs::duplicate(
            &mut tx,
            shared.id,
            new_id(),
            &name("Développement", "Copy"),
            None,
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::NameTaken)
    );
    tx.rollback().await.unwrap();

    // A configuration in use cannot go, even past the API's check.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let in_use = configs::delete(&mut tx, shared.id, None).await;
    assert!(in_use.is_err(), "the foreign key of node refuses it");
}

#[tokio::test]
async fn an_unused_configuration_is_deleted_with_its_versions() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let config = configs::insert(&mut tx, new_id(), &name("Éphémère", "Ephemeral"), None)
        .await
        .unwrap()
        .unwrap();
    let opened = configs::open_draft(&mut tx, config.id, None)
        .await
        .unwrap()
        .unwrap();
    let draft = opened.draft.unwrap();
    configs::record_change(
        &mut tx,
        draft.id,
        Item {
            kind: ItemKind::Field,
            id: 9,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        configs::delete(&mut tx, config.id, Some(&[config.version]))
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    configs::delete(&mut tx, config.id, None).await.unwrap().unwrap();
    assert_eq!(configs::find(&mut tx, config.id).await.unwrap(), None);
    assert!(versions(&mut tx, config.id).await.is_empty());
    let changes: i64 = sqlx::query_scalar("SELECT count(*) FROM project_config_change WHERE version_id = $1")
        .bind(draft.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(changes, 0);
    assert_eq!(
        configs::delete(&mut tx, config.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
}

#[tokio::test]
async fn configurations_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let config = configs::insert(
        &mut tx,
        new_id(),
        &name("Développement", "Software"),
        Some(creator),
    )
    .await
    .unwrap()
    .unwrap();
    let opened = configs::open_draft(&mut tx, config.id, None)
        .await
        .unwrap()
        .unwrap();
    let plat = node(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        creator,
    )
    .await;
    let default_a = configs::default_id(&mut tx).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let page = configs::list(&mut tx, None, 50).await.unwrap();
    assert_eq!(page.items.len(), 1, "its own default only");
    assert_ne!(page.items[0].id, default_a);
    assert_eq!(configs::find(&mut tx, config.id).await.unwrap(), None);
    assert_eq!(
        configs::rename(&mut tx, config.id, None, &name("Volé", "Stolen"))
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        configs::open_draft(&mut tx, config.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        configs::publish(&mut tx, config.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    let draft = opened.draft.unwrap().id;
    let kind = Item {
        kind: ItemKind::Screen,
        id: 1,
    };
    assert!(!configs::record_change(&mut tx, draft, kind).await.unwrap());
    assert_eq!(configs::of_project(&mut tx, plat).await.unwrap(), None);
    assert!(!configs::assign(&mut tx, plat, page.items[0].id).await.unwrap());
    assert!(configs::projects(&mut tx, config.id).await.unwrap().is_empty());
    // Not even through a raw write: row-level security.
    let moved = sqlx::query("UPDATE node SET config_id = $1")
        .bind(config.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(moved.rows_affected(), 0);
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM project_config_version")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(versions, 1, "only the published version of its default");
}
