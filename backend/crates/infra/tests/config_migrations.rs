//! Integration tests on a real PostgreSQL: the migrations of a
//! configuration change (L04-US08). A migration is recorded with its job,
//! one at a time per configuration; its run publishes the draft once, even
//! when a first attempt dies midway; it fails without publishing when the
//! draft changed; tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::migration::State;
use infra::config_migrations::{self, MIGRATION_JOB, Mapping, NewMigration, Outcome};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use testkit::TestDb;

fn labels(en: &str) -> BTreeMap<String, String> {
    [("en".to_owned(), en.to_owned())].into()
}

/// A configuration with a draft; its id and the draft's.
async fn with_draft(conn: &mut PgConnection) -> (Uuid, Uuid) {
    let config = configs::insert(conn, new_id(), &labels(&format!("Software {}", new_id())), None)
        .await
        .unwrap()
        .unwrap();
    let draft = configs::open_draft(conn, config.id, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    (config.id, draft)
}

async fn start(conn: &mut PgConnection, config: Uuid, draft: Uuid, account: Uuid) -> (Uuid, bool) {
    let id = new_id();
    let mapping = [Mapping {
        from: new_id(),
        to: new_id(),
    }];
    let started = config_migrations::start(
        conn,
        &NewMigration {
            id,
            config_id: config,
            draft_id: draft,
            status_mappings: &mapping,
            type_mappings: &[],
            ticket_total: 0,
            created_by: account,
        },
    )
    .await
    .unwrap();
    (id, started)
}

#[tokio::test]
async fn a_migration_is_queued_with_its_job_one_at_a_time() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (config, draft) = with_draft(&mut tx).await;
    let (id, started) = start(&mut tx, config, draft, new_id()).await;
    assert!(started);
    let migration = config_migrations::find(&mut tx, config, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((migration.state, migration.draft_id), (State::Pending, draft));
    assert_eq!(migration.status_mappings.len(), 1);
    let jobs: i64 = infra::sqlx::query_scalar(
        "SELECT count(*) FROM job WHERE kind = $1 AND payload ->> 'migration_id' = $2",
    )
    .bind(MIGRATION_JOB)
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(jobs, 1);
    tx.commit().await.unwrap();

    // A second one while the first is under way: refused.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(!start(&mut tx, config, draft, new_id()).await.1);
}

#[tokio::test]
async fn a_run_publishes_the_draft_once_even_after_dying_midway() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (config, draft) = with_draft(&mut tx).await;
    let (id, _) = start(&mut tx, config, draft, new_id()).await;
    let before = configs::find(&mut tx, config)
        .await
        .unwrap()
        .unwrap()
        .published
        .number;
    tx.commit().await.unwrap();

    // A first attempt dies before committing: nothing happened.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Published
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let still = configs::find(&mut tx, config).await.unwrap().unwrap();
    assert_eq!(
        (still.published.number, still.draft.map(|d| d.id)),
        (before, Some(draft))
    );
    assert_eq!(
        config_migrations::find(&mut tx, config, id)
            .await
            .unwrap()
            .unwrap()
            .state,
        State::Pending
    );
    tx.commit().await.unwrap();

    // Retried: published once.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Published
    );
    tx.commit().await.unwrap();
    // Retried again (the job ran twice): nothing more.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::AlreadyOver
    );
    let after = configs::find(&mut tx, config).await.unwrap().unwrap();
    assert_eq!(after.published.number, before + 1);
    assert_eq!(after.published.id, draft);
    assert!(after.draft.is_none());
    let done = config_migrations::find(&mut tx, config, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.state, State::Done);
    assert!(done.finished_at.is_some());
    assert_eq!(
        config_migrations::run(&mut tx, new_id()).await.unwrap(),
        Outcome::Missing
    );
}

#[tokio::test]
async fn a_run_fails_without_publishing_when_the_draft_changed() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (config, draft) = with_draft(&mut tx).await;
    let (id, _) = start(&mut tx, config, draft, new_id()).await;
    // The draft is discarded, then another one opened.
    configs::discard_draft(&mut tx, config, None)
        .await
        .unwrap()
        .unwrap();
    let other = configs::open_draft(&mut tx, config, None)
        .await
        .unwrap()
        .unwrap()
        .draft
        .unwrap()
        .id;
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::DraftChanged
    );
    let failed = config_migrations::find(&mut tx, config, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, State::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("the draft changed since the mapping")
    );
    let config_now = configs::find(&mut tx, config).await.unwrap().unwrap();
    assert_eq!(config_now.draft.map(|d| d.id), Some(other), "nothing published");
    // Over: another migration may start.
    assert!(start(&mut tx, config, other, new_id()).await.1);
}

#[tokio::test]
async fn migrations_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (config, draft) = with_draft(&mut tx).await;
    let (id, _) = start(&mut tx, config, draft, new_id()).await;
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        config_migrations::find(&mut tx, config, id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Missing
    );
    // Not even for another tenant's configuration: its foreign key is checked with the tenant.
    let mapping: [Mapping; 0] = [];
    let foreign = config_migrations::start(
        &mut tx,
        &NewMigration {
            id: new_id(),
            config_id: config,
            draft_id: draft,
            status_mappings: &mapping,
            type_mappings: &mapping,
            ticket_total: 0,
            created_by: new_id(),
        },
    )
    .await;
    assert!(foreign.is_err());
}
