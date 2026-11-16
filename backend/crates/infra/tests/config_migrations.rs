//! Integration tests on a real PostgreSQL: the migrations of a
//! configuration change (L04-US08). A migration is recorded with its job,
//! one at a time per configuration; its run publishes the draft once, even
//! when a first attempt dies midway; it fails without publishing when the
//! draft changed; tickets move batch by batch with their history, a run
//! killed midway resumes without moving one twice; tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::hierarchy::NodeType;
use domain::migration::State;
use infra::accounts::TenantRole;
use infra::config_migrations::{self, BATCH_SIZE, MIGRATION_JOB, Mapping, NewMigration, Outcome};
use infra::configs;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx::{self, PgConnection};
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

/// A project `key` on the configuration `config`; its id.
async fn project_on(db: &TestDb, key: &str, config: Uuid) -> Uuid {
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key,
            fields: NodeFields {
                key: None,
                name: key,
                description: "",
                objectives: "",
                start_date: None,
                end_date: None,
                health: domain::hierarchy::Health::OnTrack,
                owners: &[],
            },
            parent: None,
            created_by: admin,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(configs::assign(&mut tx, node.id, config).await.unwrap());
    tx.commit().await.unwrap();
    node.id
}

/// Tickets `from..=to` of the project `key` in `status`.
async fn tickets(conn: &mut PgConnection, project: Uuid, key: &str, (from, to): (i32, i32), status: Uuid) {
    sqlx::query(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary)
        SELECT gen_random_uuid(), app_current_tenant(), $1, n, $2 || '-' || n, $3, $4, 'Ticket ' || n
        FROM generate_series($5::integer, $6::integer) AS n
        "#,
    )
    .bind(project)
    .bind(key)
    .bind(Uuid::from_u128(1))
    .bind(status)
    .bind(from)
    .bind(to)
    .execute(conn)
    .await
    .unwrap();
}

async fn scalar(conn: &mut PgConnection, sql: &'static str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql).bind(id).fetch_one(conn).await.unwrap()
}

async fn in_status(conn: &mut PgConnection, status: Uuid) -> i64 {
    scalar(conn, "SELECT count(*) FROM issue WHERE status_id = $1", status).await
}

/// Where the migration `id` of `config` stands: tickets migrated and total.
async fn progress(conn: &mut PgConnection, config: Uuid, id: Uuid) -> (State, i64, i64) {
    let m = config_migrations::find(conn, config, id).await.unwrap().unwrap();
    (m.state, m.tickets_migrated, m.ticket_total)
}

#[tokio::test]
async fn tickets_move_batch_by_batch_and_a_job_killed_midway_resumes_without_moving_one_twice() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (config, draft) = with_draft(&mut tx).await;
    tx.commit().await.unwrap();
    let plat = project_on(&db, "PLAT", config).await;
    let mail = project_on(&db, "MAIL", config).await;
    let (in_test, accepted, done) = (new_id(), new_id(), new_id());
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    tickets(&mut tx, plat, "PLAT", (1, 700), in_test).await;
    tickets(&mut tx, mail, "MAIL", (1, 400), in_test).await;
    // Not concerned: left alone.
    tickets(&mut tx, mail, "MAIL", (401, 410), done).await;
    let counts = config_migrations::ticket_counts(&mut tx, config).await.unwrap();
    assert_eq!(counts.len(), 2);
    assert!(counts.contains(&(in_test, Uuid::from_u128(1), 1100)));
    let id = new_id();
    let mapping = [Mapping {
        from: in_test,
        to: accepted,
    }];
    let new = NewMigration {
        id,
        config_id: config,
        draft_id: draft,
        status_mappings: &mapping,
        type_mappings: &[],
        ticket_total: 1100,
        created_by: new_id(),
    };
    assert!(config_migrations::start(&mut tx, &new).await.unwrap());
    tx.commit().await.unwrap();
    let queued =
        "SELECT count(*) FROM job WHERE kind = 'config.migration' AND payload ->> 'migration_id' = $1::text";

    // A first batch: moved, progress counted, the next run queued.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Continues
    );
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        progress(&mut tx, config, id).await,
        (State::Running, BATCH_SIZE, 1100)
    );
    assert_eq!(in_status(&mut tx, accepted).await, BATCH_SIZE);
    assert_eq!(scalar(&mut tx, queued, id).await, 2);
    // A ticket comes into the removed status meanwhile: it moves too.
    tickets(&mut tx, plat, "PLAT", (701, 701), in_test).await;
    tx.commit().await.unwrap();

    // The second run is killed midway (PostgreSQL rolls back the
    // transaction of a dead connection): its batch rolls back whole.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Continues
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        progress(&mut tx, config, id).await,
        (State::Running, BATCH_SIZE, 1100)
    );
    assert_eq!(in_status(&mut tx, accepted).await, BATCH_SIZE);
    assert_eq!(in_status(&mut tx, in_test).await, 601);
    tx.commit().await.unwrap();

    // Retried, then the last batch publishes.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Continues
    );
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        progress(&mut tx, config, id).await,
        (State::Running, 2 * BATCH_SIZE, 1100)
    );
    assert!(
        configs::find(&mut tx, config)
            .await
            .unwrap()
            .unwrap()
            .draft
            .is_some(),
        "not published yet"
    );
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::Published
    );
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(progress(&mut tx, config, id).await, (State::Done, 1101, 1101));
    assert_eq!(
        configs::find(&mut tx, config)
            .await
            .unwrap()
            .unwrap()
            .published
            .id,
        draft
    );
    assert_eq!(in_status(&mut tx, in_test).await, 0);
    assert_eq!(in_status(&mut tx, accepted).await, 1101);
    assert_eq!(in_status(&mut tx, done).await, 10);
    // Each ticket moved once: one history entry each, by the system, its
    // version bumped once.
    let (entries, tickets_with_one, bumped): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT count(*) FROM audit_log WHERE field = 'status' AND origin = 'system' AND actor_id IS NULL
                AND old_value ->> 'id' = $1::text AND new_value ->> 'id' = $2::text),
               (SELECT count(*) FROM (SELECT entity_id FROM audit_log GROUP BY entity_id HAVING count(*) = 1) AS one),
               (SELECT count(*) FROM issue WHERE version = 2)
        "#,
    )
    .bind(in_test)
    .bind(accepted)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!((entries, tickets_with_one, bumped), (1101, 1101, 1101));
    // Run again (the job ran twice): nothing more.
    assert_eq!(
        config_migrations::run(&mut tx, id).await.unwrap(),
        Outcome::AlreadyOver
    );
    tx.commit().await.unwrap();

    // Another tenant counts none of them.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        config_migrations::ticket_counts(&mut tx, config)
            .await
            .unwrap()
            .is_empty()
    );
}
