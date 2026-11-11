//! Integration tests on a real PostgreSQL: tenant settings, the platform
//! functions of the super admin, tenant deletion and purge, bootstrap.

#![allow(clippy::unwrap_used)]

use domain::tenant::{OverloadMode, TenantSettingsInput};
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::bootstrap::BootstrapOutcome;
use infra::config::{BootstrapConfig, Secret};
use infra::db::{self, begin_tenant_tx};
use infra::id::new_id;
use infra::platform::{self, PlatformConflict, TenantSettings, TenantStatus};
use infra::sqlx;
use infra::uuid::Uuid;
use testkit::{TestDb, TestMember};

/// A super admin, member of tenant alpha.
async fn super_admin(db: &TestDb) -> TestMember {
    let admin = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    db.make_platform_admin(admin.account_id).await;
    admin
}

fn settings(name: &str) -> TenantSettings {
    TenantSettings::from_valid(&TenantSettingsInput::defaults(name)).unwrap()
}

async fn conn(db: &TestDb) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    db.app_pool.acquire().await.unwrap()
}

async fn count(db: &TestDb, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn existing_tenants_get_the_default_settings() {
    let db = TestDb::new().await;
    let admin = super_admin(&db).await;
    let tenant = platform::get(&mut *conn(&db).await, admin.account_id, db.tenant_a.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tenant.slug, "alpha");
    assert_eq!(tenant.default_capacity_minutes, [420, 420, 420, 420, 420, 0, 0]);
    assert_eq!(tenant.overload_tolerance_percent, 0);
    assert_eq!(tenant.overload_mode, OverloadMode::Blocking);
    assert_eq!(
        (tenant.storage_quota_bytes, tenant.api_rate_limit_per_minute),
        (None, None)
    );
    assert_eq!(tenant.status, TenantStatus::Active);
}

#[tokio::test]
async fn a_super_admin_creates_lists_and_updates_tenants() {
    let db = TestDb::new().await;
    let admin = super_admin(&db).await;
    let mut conn = conn(&db).await;

    let mut acme = settings("Acme Industrie");
    acme.default_locale = "en".to_owned();
    acme.time_zone = "America/New_York".to_owned();
    acme.default_capacity_minutes = vec![480, 480, 480, 480, 240, 0, 0];
    acme.overload_tolerance_percent = 10;
    acme.overload_mode = OverloadMode::Warning;
    let id = new_id();
    let created = platform::create(&mut conn, admin.account_id, id, "acme", &acme)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (created.id, created.slug.as_str(), created.version),
        (id, "acme", 1)
    );
    assert_eq!(created.default_capacity_minutes, [480, 480, 480, 480, 240, 0, 0]);
    assert_eq!(created.overload_mode, OverloadMode::Warning);

    // Reachable by its host name right away.
    let resolved = db::resolve_tenant(&mut conn, "acme").await.unwrap().unwrap();
    assert_eq!((resolved.id, resolved.default_locale.as_str()), (id, "en"));

    let page = platform::list(&mut conn, admin.account_id, None, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.slug.as_str()).collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    let rest = platform::list(&mut conn, admin.account_id, page.next, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rest.items.iter().map(|t| t.slug.as_str()).collect::<Vec<_>>(),
        ["acme"]
    );
    assert_eq!(rest.next, None);

    // Quotas are stored and handed to the services through resolution.
    acme.storage_quota_bytes = Some(5 * 1024 * 1024 * 1024);
    acme.api_rate_limit_per_minute = Some(600);
    let updated = platform::update(&mut conn, admin.account_id, id, Some(&[1]), &acme)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.version, 2);
    let quotas = db::resolve_tenant(&mut conn, "acme")
        .await
        .unwrap()
        .unwrap()
        .quotas;
    assert_eq!(quotas.storage_bytes, Some(5 * 1024 * 1024 * 1024));
    assert_eq!(quotas.api_requests_per_minute, Some(600));

    assert_eq!(
        platform::update(&mut conn, admin.account_id, id, Some(&[1]), &acme)
            .await
            .unwrap(),
        Err(PlatformConflict::VersionMismatch)
    );
    assert_eq!(
        platform::update(&mut conn, admin.account_id, new_id(), None, &acme)
            .await
            .unwrap(),
        Err(PlatformConflict::NotFound)
    );
}

#[tokio::test]
async fn tenant_slugs_are_unique() {
    let db = TestDb::new().await;
    let admin = super_admin(&db).await;
    let result = platform::create(
        &mut *conn(&db).await,
        admin.account_id,
        new_id(),
        "beta",
        &settings("Other"),
    )
    .await
    .unwrap();
    assert_eq!(result, Err(PlatformConflict::SlugTaken));
}

#[tokio::test]
async fn the_database_refuses_invalid_settings_whoever_writes_them() {
    let db = TestDb::new().await;
    let admin = super_admin(&db).await;
    let mut conn = conn(&db).await;
    assert!(
        !platform::time_zone_exists(&mut conn, "Mars/Olympus_Mons")
            .await
            .unwrap()
    );
    assert!(
        platform::time_zone_exists(&mut conn, "Europe/Paris")
            .await
            .unwrap()
    );

    let mut bad = settings("Acme");
    bad.time_zone = "Mars/Olympus_Mons".to_owned();
    let err = platform::create(&mut conn, admin.account_id, new_id(), "acme", &bad)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown time zone"), "{err}");

    let mut bad = settings("Acme");
    bad.default_capacity_minutes = vec![420; 6];
    assert!(
        platform::create(&mut conn, admin.account_id, new_id(), "acme", &bad)
            .await
            .is_err()
    );
    let mut bad = settings("Acme");
    bad.overload_tolerance_percent = 101;
    assert!(
        platform::create(&mut conn, admin.account_id, new_id(), "acme", &bad)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn only_platform_super_admins_manage_tenants() {
    let db = TestDb::new().await;
    // A tenant admin is not a platform super admin.
    let tenant_admin = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    let actor = tenant_admin.account_id;
    let mut conn = conn(&db).await;

    assert_eq!(
        platform::list(&mut conn, actor, None, 10).await.unwrap().err(),
        Some(PlatformConflict::Forbidden)
    );
    assert_eq!(
        platform::get(&mut conn, actor, db.tenant_a.id)
            .await
            .unwrap()
            .err(),
        Some(PlatformConflict::Forbidden)
    );
    assert_eq!(
        platform::create(&mut conn, actor, new_id(), "acme", &settings("Acme"))
            .await
            .unwrap(),
        Err(PlatformConflict::Forbidden)
    );
    assert_eq!(
        platform::update(&mut conn, actor, db.tenant_b.id, None, &settings("B"))
            .await
            .unwrap(),
        Err(PlatformConflict::Forbidden)
    );
    assert_eq!(
        platform::delete(&mut conn, actor, db.tenant_b.id, None, new_id())
            .await
            .unwrap(),
        Err(PlatformConflict::Forbidden)
    );
    assert!(!platform::is_platform_admin(&mut conn, actor).await.unwrap());
    assert_eq!(
        count(&db, "SELECT count(*) FROM tenant WHERE status = 'active'").await,
        2
    );

    // The runtime role can neither read the marker nor write tenants itself.
    assert!(
        sqlx::query("SELECT * FROM platform_admin")
            .fetch_all(&mut *conn)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO platform_admin (account_id) VALUES ($1)")
            .bind(actor)
            .execute(&mut *conn)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE tenant SET name = 'x'")
            .execute(&mut *conn)
            .await
            .is_err()
    );
    // Purging needs a tenant marked for deletion, which only a super admin can do.
    assert!(platform::purge(&mut conn, db.tenant_b.id).await.is_err());
}

/// Fills tenant beta with data of every kind there is today.
async fn fill_beta(db: &TestDb) -> (TestMember, TestMember) {
    let only_beta = db.add_member(&db.tenant_b, TenantRole::Member).await;
    let both = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &both.email,
            display_name: "Both",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    infra::accounts::create_password_reset(&mut tx, only_beta.account_id, b"hash", chrono::Utc::now())
        .await
        .unwrap();
    infra::jobs::enqueue(&mut tx, "test.job", &serde_json::json!({}))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reference_item (id, tenant_id, code, label, quantity)
         VALUES ($1, app_current_tenant(), 'REF-1', '{\"fr\": \"x\"}', 1)",
    )
    .bind(new_id())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO idempotency_key (id, tenant_id, key, request_hash, expires_at)
         VALUES ($1, app_current_tenant(), 'k', '\\x00', now() + interval '1 day')",
    )
    .bind(new_id())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (only_beta, both)
}

/// Rows of `tenant` in every table having a tenant_id column.
async fn rows_of(db: &TestDb, tenant: Uuid) -> i64 {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         JOIN pg_attribute a ON a.attrelid = c.oid
         WHERE n.nspname = 'public' AND c.relkind = 'r' AND a.attname = 'tenant_id'",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    for expected in [
        "membership",
        "password_reset",
        "job",
        "reference_item",
        "idempotency_key",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "{expected} missing from {tables:?}"
        );
    }
    let mut total = 0;
    for table in tables {
        // The owner sees every row through set_config, table by table.
        let mut tx = begin_tenant_tx(&db.owner_pool, tenant).await.unwrap();
        let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {table} WHERE tenant_id = $1"
        )))
        .bind(tenant)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        total += n;
    }
    total
}

#[tokio::test]
async fn a_deleted_tenant_stops_resolving_then_its_purge_leaves_no_row() {
    let db = TestDb::new().await;
    let admin = super_admin(&db).await;
    let (only_beta, both) = fill_beta(&db).await;
    assert!(rows_of(&db, db.tenant_b.id).await >= 6);
    let alpha_rows = rows_of(&db, db.tenant_a.id).await;
    let mut conn = conn(&db).await;

    let job_id = new_id();
    let deleting = platform::delete(&mut conn, admin.account_id, db.tenant_b.id, Some(&[1]), job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleting.status, TenantStatus::Deleting);
    assert_eq!(
        db::resolve_tenant(&mut conn, "beta").await.unwrap(),
        None,
        "no longer resolves"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT count(*) FROM job WHERE id = '{job_id}' AND kind = 'tenant.purge'")
        )
        .await,
        1
    );
    // A tenant being deleted can be neither changed nor deleted again.
    assert_eq!(
        platform::update(&mut conn, admin.account_id, db.tenant_b.id, None, &settings("B"))
            .await
            .unwrap(),
        Err(PlatformConflict::NotFound)
    );
    assert_eq!(
        platform::delete(&mut conn, admin.account_id, db.tenant_b.id, None, new_id())
            .await
            .unwrap(),
        Err(PlatformConflict::NotFound)
    );

    // The purge job runs in the tenant's context.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let removed = platform::purge(&mut tx, db.tenant_b.id).await.unwrap();
    tx.commit().await.unwrap();
    assert!(removed >= 8, "{removed}");

    assert_eq!(rows_of(&db, db.tenant_b.id).await, 0);
    assert_eq!(
        count(
            &db,
            &format!("SELECT count(*) FROM tenant WHERE id = '{}'", db.tenant_b.id)
        )
        .await,
        0
    );
    let accounts = |id: Uuid| format!("SELECT count(*) FROM account WHERE id = '{id}'");
    assert_eq!(
        count(&db, &accounts(only_beta.account_id)).await,
        0,
        "its own accounts go"
    );
    assert_eq!(
        count(&db, &accounts(both.account_id)).await,
        1,
        "shared accounts stay"
    );
    assert_eq!(count(&db, &accounts(admin.account_id)).await, 1);
    assert_eq!(
        rows_of(&db, db.tenant_a.id).await,
        alpha_rows,
        "other tenants are untouched"
    );
}

fn bootstrap_config(slug: &str) -> BootstrapConfig {
    BootstrapConfig {
        tenant_slug: slug.to_owned(),
        tenant_name: "Bootstrap Corp".to_owned(),
        admin_email: "root@bootstrap.test".to_owned(),
        admin_name: "Root".to_owned(),
        admin_password: Secret::new("bootstrap passphrase"),
    }
}

#[tokio::test]
async fn bootstrap_creates_the_first_tenant_and_super_admin_once() {
    let db = TestDb::new().await;
    let owner = Secret::new(db.owner_url());
    let config = bootstrap_config("first");

    let outcome = infra::bootstrap::run(&owner, &config).await.unwrap();
    let BootstrapOutcome::Done {
        tenant_id,
        account_id,
        tenant_created: true,
        account_created: true,
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    let mut conn = conn(&db).await;
    assert!(platform::is_platform_admin(&mut conn, account_id).await.unwrap());
    let resolved = db::resolve_tenant(&mut conn, "first").await.unwrap().unwrap();
    assert_eq!(resolved.id, tenant_id);
    let mut tx = begin_tenant_tx(&db.app_pool, tenant_id).await.unwrap();
    let member = infra::accounts::find_by_email(&mut tx, "root@bootstrap.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member.member.tenant_role, TenantRole::TenantAdmin);
    assert!(
        member.member.local_account,
        "a local account: a way in if the SSO breaks"
    );
    assert!(infra::credentials::verify_password(
        "bootstrap passphrase",
        member.password_hash.as_deref().unwrap()
    ));
    tx.rollback().await.unwrap();

    // Later starts change nothing, even with other values.
    let mut other = bootstrap_config("second");
    other.admin_email = "other@bootstrap.test".to_owned();
    assert_eq!(
        infra::bootstrap::run(&owner, &other).await.unwrap(),
        BootstrapOutcome::AlreadyDone
    );
    assert_eq!(db::resolve_tenant(&mut conn, "second").await.unwrap(), None);
    assert_eq!(count(&db, "SELECT count(*) FROM platform_admin").await, 1);
}

#[tokio::test]
async fn bootstrap_uses_an_existing_tenant_and_concurrent_starts_bootstrap_once() {
    let db = TestDb::new().await;
    let owner = Secret::new(db.owner_url());
    let config = bootstrap_config("alpha");
    let (a, b) = tokio::join!(
        infra::bootstrap::run(&owner, &config),
        infra::bootstrap::run(&owner, &config)
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == BootstrapOutcome::AlreadyDone)
            .count(),
        1,
        "{outcomes:?}"
    );
    assert!(outcomes.iter().any(|o| matches!(
        o,
        BootstrapOutcome::Done { tenant_created: false, tenant_id, .. } if *tenant_id == db.tenant_a.id
    )));
    assert_eq!(count(&db, "SELECT count(*) FROM tenant").await, 2);
}
