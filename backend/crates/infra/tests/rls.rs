//! Integration tests on a real PostgreSQL: migrations, tenant context and
//! row-level security. Each test gets its own database (`testkit::TestDb`).

#![allow(clippy::unwrap_used)]

use infra::db::{self, MIGRATOR};
use infra::id::new_id;
use infra::sqlx::{self, Row};
use infra::uuid::Uuid;
use testkit::TestDb;

/// A tenant-scoped table built with the standard pattern, like every
/// business table will be.
async fn with_sample_table(db: &TestDb) {
    db.exec_as_owner(
        "CREATE TABLE sample_note (
            id uuid PRIMARY KEY,
            tenant_id uuid NOT NULL REFERENCES tenant (id),
            body text NOT NULL,
            created_at timestamptz NOT NULL DEFAULT now(),
            updated_at timestamptz NOT NULL DEFAULT now(),
            version bigint NOT NULL DEFAULT 1
        );
        CREATE INDEX sample_note_tenant_idx ON sample_note (tenant_id);
        SELECT app_setup_business_table('sample_note');",
    )
    .await;
}

async fn insert_note(db: &TestDb, tenant_id: Uuid, body: &str) -> Uuid {
    let id = new_id();
    let mut tx = db::begin_tenant_tx(&db.app_pool, tenant_id).await.unwrap();
    sqlx::query("INSERT INTO sample_note (id, tenant_id, body) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(tenant_id)
        .bind(body)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn migrations_apply_once_and_re_run_idempotently() {
    let db = TestDb::new().await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.owner_pool)
            .await
            .unwrap()
    };
    let before = count().await;
    assert_eq!(before, i64::try_from(MIGRATOR.iter().count()).unwrap());

    MIGRATOR.run(&db.owner_pool).await.unwrap();
    assert_eq!(count().await, before, "re-running applies nothing");
}

#[tokio::test]
async fn tenant_setting_is_applied_first_and_ends_with_the_transaction() {
    let db = TestDb::new().await;

    let mut tx = db::begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let current: Option<Uuid> = sqlx::query_scalar("SELECT app_current_tenant()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(current, Some(db.tenant_a.id));
    tx.commit().await.unwrap();

    // Same pool, likely the same connection: the setting must not leak.
    let after: Option<Uuid> = sqlx::query_scalar("SELECT app_current_tenant()")
        .fetch_one(&db.app_pool)
        .await
        .unwrap();
    assert_eq!(after, None);
}

#[tokio::test]
async fn queries_without_tenant_filter_only_see_the_current_tenant() {
    let db = TestDb::new().await;
    with_sample_table(&db).await;
    insert_note(&db, db.tenant_a.id, "alpha note").await;
    insert_note(&db, db.tenant_b.id, "beta note").await;

    for (tenant, expected) in [(&db.tenant_a, "alpha note"), (&db.tenant_b, "beta note")] {
        let mut tx = db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
        let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM sample_note")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(bodies, vec![expected.to_owned()], "tenant {}", tenant.slug);
    }

    let outside: i64 = sqlx::query_scalar("SELECT count(*) FROM sample_note")
        .fetch_one(&db.app_pool)
        .await
        .unwrap();
    assert_eq!(outside, 0, "no row is visible without a tenant context");
}

#[tokio::test]
async fn other_tenant_rows_cannot_be_updated_deleted_or_inserted() {
    let db = TestDb::new().await;
    with_sample_table(&db).await;
    let beta_note = insert_note(&db, db.tenant_b.id, "beta note").await;

    let mut tx = db::begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let updated = sqlx::query("UPDATE sample_note SET body = 'hacked' WHERE id = $1")
        .bind(beta_note)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 0);
    let deleted = sqlx::query("DELETE FROM sample_note WHERE id = $1")
        .bind(beta_note)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(deleted.rows_affected(), 0);
    let moved = sqlx::query("UPDATE sample_note SET tenant_id = $1")
        .bind(db.tenant_b.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(moved.rows_affected(), 0);
    tx.commit().await.unwrap();

    let mut tx = db::begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let forged = sqlx::query("INSERT INTO sample_note (id, tenant_id, body) VALUES ($1, $2, 'forged')")
        .bind(new_id())
        .bind(db.tenant_b.id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(forged.to_string().contains("row-level security"), "{forged}");
    drop(tx);

    let mut tx = db::begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let body: String = sqlx::query_scalar("SELECT body FROM sample_note WHERE id = $1")
        .bind(beta_note)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(body, "beta note", "beta's row is untouched");
}

#[tokio::test]
async fn application_role_cannot_bypass_row_level_security() {
    let db = TestDb::new().await;
    with_sample_table(&db).await;
    insert_note(&db, db.tenant_a.id, "alpha note").await;

    let role = sqlx::query("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
        .fetch_one(&db.app_pool)
        .await
        .unwrap();
    assert!(!role.get::<bool, _>("rolsuper"));
    assert!(!role.get::<bool, _>("rolbypassrls"));

    let mut conn = db.app_pool.acquire().await.unwrap();
    sqlx::query("SET row_security = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    let err = sqlx::query("SELECT count(*) FROM sample_note")
        .execute(&mut *conn)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("row-level security"), "{err}");
    sqlx::query("RESET row_security")
        .execute(&mut *conn)
        .await
        .unwrap();

    for ddl in [
        "ALTER TABLE sample_note DISABLE ROW LEVEL SECURITY",
        "ALTER TABLE sample_note NO FORCE ROW LEVEL SECURITY",
        "DROP POLICY tenant_isolation ON sample_note",
    ] {
        let err = sqlx::raw_sql(ddl).execute(&mut *conn).await.unwrap_err();
        assert!(err.to_string().contains("must be owner"), "{ddl}: {err}");
    }
}

#[tokio::test]
async fn tenant_table_is_resolvable_by_slug_but_only_readable_in_context() {
    let db = TestDb::new().await;
    let mut conn = db.app_pool.acquire().await.unwrap();

    assert_eq!(
        db::resolve_tenant(&mut conn, "alpha").await.unwrap(),
        Some(db::ResolvedTenant {
            id: db.tenant_a.id,
            default_locale: "fr".to_owned(),
            // Unlimited until the super admin sets quotas.
            quotas: db::TenantQuotas::default(),
        })
    );
    assert_eq!(db::resolve_tenant(&mut conn, "nobody").await.unwrap(), None);

    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM tenant")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(visible, 0, "the tenant list is not readable without context");
    drop(conn);

    let mut tx = db::begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let slugs: Vec<String> = sqlx::query_scalar("SELECT slug FROM tenant")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(slugs, vec!["beta".to_owned()]);
}

#[tokio::test]
async fn every_migrated_table_follows_the_rls_rules() {
    let db = TestDb::new().await;
    with_sample_table(&db).await;
    let offenders = db::tables_breaking_rls_rules(&db.owner_pool).await.unwrap();
    assert!(
        offenders.is_empty(),
        "tables without proper row-level security: {offenders:?}; see backend/migrations/README.md"
    );
}

#[tokio::test]
async fn rls_checker_reports_unprotected_tables() {
    let db = TestDb::new().await;
    db.exec_as_owner(
        "CREATE TABLE no_rls (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);
         CREATE TABLE no_policy (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);
         ALTER TABLE no_policy ENABLE ROW LEVEL SECURITY;
         ALTER TABLE no_policy FORCE ROW LEVEL SECURITY;
         CREATE TABLE not_forced (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);
         ALTER TABLE not_forced ENABLE ROW LEVEL SECURITY;
         CREATE POLICY p ON not_forced USING (tenant_id = app_current_tenant());
         CREATE TABLE no_tenant_column (id uuid PRIMARY KEY);
         ALTER TABLE no_tenant_column ENABLE ROW LEVEL SECURITY;
         ALTER TABLE no_tenant_column FORCE ROW LEVEL SECURITY;
         CREATE POLICY p ON no_tenant_column USING (true);",
    )
    .await;
    let offenders = db::tables_breaking_rls_rules(&db.owner_pool).await.unwrap();
    assert_eq!(
        offenders,
        vec!["no_policy", "no_rls", "no_tenant_column", "not_forced"]
    );
}

#[tokio::test]
async fn updated_at_is_maintained_by_the_standard_trigger() {
    let db = TestDb::new().await;
    with_sample_table(&db).await;
    let id = insert_note(&db, db.tenant_a.id, "first").await;

    let mut tx = db::begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    sqlx::query(
        "UPDATE sample_note SET body = 'second', created_at = now() - interval '1 day' WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let newer: bool = sqlx::query_scalar("SELECT updated_at > created_at FROM sample_note WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(newer);
}
