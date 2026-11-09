//! Shared helpers for integration tests (dev-dependency only).
//!
//! [`TestDb::new`] gives each test its own empty PostgreSQL database with
//! every migration applied and two tenants seeded (`alpha` and `beta`).
//! Tests can therefore run in parallel without seeing each other's data.
//! The database is dropped when the `TestDb` goes out of scope.
//!
//! It connects to the server started by `just db-up`. To use another server,
//! set `TEST_DATABASE_ADMIN_URL` to a superuser URL, and
//! `TEST_DATABASE_OWNER_PASSWORD` / `TEST_DATABASE_APP_PASSWORD` if the
//! `mlbpp_owner` / `mlbpp_app` roles already exist there with other passwords.
//!
//! ```ignore
//! let db = testkit::TestDb::new().await;
//! let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id).await?;
//! ```

use std::str::FromStr;

use infra::db::MIGRATOR;
use infra::id::new_id;
use infra::sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use infra::sqlx::{self, AssertSqlSafe, Connection, PgConnection};
use infra::uuid::Uuid;

const OWNER_ROLE: &str = "mlbpp_owner";
const APP_ROLE: &str = "mlbpp_app";
/// Arbitrary key serializing role and database creation across parallel tests.
const SETUP_LOCK_KEY: i64 = 0x6d6c_6270_7074;

/// A tenant seeded in every test database.
#[derive(Debug, Clone)]
pub struct SeedTenant {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

/// An isolated, migrated database for one test.
#[derive(Debug)]
pub struct TestDb {
    /// Database name, `mlbpp_test_<uuid>`.
    pub name: String,
    /// Pool of the runtime role: row-level security applies.
    pub app_pool: PgPool,
    /// Pool of the owner role, for test setup that the application cannot do
    /// (creating sample tables, seeding tenants). Never use it for assertions
    /// about isolation.
    pub owner_pool: PgPool,
    pub tenant_a: SeedTenant,
    pub tenant_b: SeedTenant,
    admin: PgConnectOptions,
}

impl TestDb {
    pub async fn new() -> Self {
        let admin = admin_options();
        let name = format!("mlbpp_test_{}", new_id().simple());
        create_database(&admin, &name).await;

        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(
                admin
                    .clone()
                    .username(OWNER_ROLE)
                    .password(&owner_password())
                    .database(&name),
            )
            .await
            .expect("connect to the test database as mlbpp_owner");
        MIGRATOR.run(&owner_pool).await.expect("apply migrations");

        let app_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(
                admin
                    .clone()
                    .username(APP_ROLE)
                    .password(&app_password())
                    .database(&name),
            )
            .await
            .expect("connect to the test database as mlbpp_app");

        let tenant_a = seed_tenant(&owner_pool, "alpha", "Alpha Corp").await;
        let tenant_b = seed_tenant(&owner_pool, "beta", "Beta Ltd").await;

        Self {
            name,
            app_pool,
            owner_pool,
            tenant_a,
            tenant_b,
            admin,
        }
    }

    /// Connection URL of the owner role on this database, for tests that
    /// start a binary (`api migrate`).
    pub fn owner_url(&self) -> String {
        format!(
            "postgres://{OWNER_ROLE}:{}@{}:{}/{}",
            owner_password(),
            self.admin.get_host(),
            self.admin.get_port(),
            self.name
        )
    }

    /// Runs `sql` with the owner role (DDL, seeding).
    pub async fn exec_as_owner(&self, sql: &str) {
        sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
            .execute(&self.owner_pool)
            .await
            .unwrap_or_else(|e| panic!("owner SQL failed: {e}\n{sql}"));
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let admin = self.admin.clone();
        let name = self.name.clone();
        // Drop runs inside the test's runtime, which cannot be blocked on:
        // use a short-lived runtime on its own thread.
        let handle = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            runtime.block_on(async move {
                if let Ok(mut conn) = PgConnection::connect_with(&admin).await {
                    let sql = format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#);
                    let _ = sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut conn).await;
                    let _ = conn.close().await;
                }
            });
        });
        let _ = handle.join();
    }
}

fn admin_options() -> PgConnectOptions {
    match std::env::var("TEST_DATABASE_ADMIN_URL") {
        Ok(url) => {
            PgConnectOptions::from_str(&url).expect("TEST_DATABASE_ADMIN_URL is a valid PostgreSQL URL")
        }
        // The development server of deploy/compose.dev.yaml.
        Err(_) => PgConnectOptions::new()
            .host("127.0.0.1")
            .port(54329)
            .username("postgres")
            .password("postgres")
            .database("postgres"),
    }
}

fn owner_password() -> String {
    std::env::var("TEST_DATABASE_OWNER_PASSWORD").unwrap_or_else(|_| "mlbpp_owner_dev".to_owned())
}

fn app_password() -> String {
    std::env::var("TEST_DATABASE_APP_PASSWORD").unwrap_or_else(|_| "mlbpp_app_dev".to_owned())
}

/// Creates the roles if needed (same attributes as deploy/postgres/init-roles.sh),
/// then an empty database owned by `mlbpp_owner`.
async fn create_database(admin: &PgConnectOptions, name: &str) {
    let mut conn = PgConnection::connect_with(admin).await.unwrap_or_else(|e| {
        panic!("cannot reach the test PostgreSQL server ({e}); start it with `just db-up` or set TEST_DATABASE_ADMIN_URL")
    });
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(&mut conn)
        .await
        .expect("take the setup lock");

    let roles = format!(
        r"DO $$
        BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{OWNER_ROLE}') THEN
                CREATE ROLE {OWNER_ROLE} LOGIN PASSWORD {owner};
            END IF;
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{APP_ROLE}') THEN
                CREATE ROLE {APP_ROLE} LOGIN PASSWORD {app} NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
            END IF;
        END
        $$",
        owner = quote_literal(&owner_password()),
        app = quote_literal(&app_password()),
    );
    sqlx::raw_sql(AssertSqlSafe(roles))
        .execute(&mut conn)
        .await
        .expect("create test roles");
    sqlx::raw_sql(AssertSqlSafe(format!(
        r#"CREATE DATABASE "{name}" OWNER {OWNER_ROLE}"#
    )))
    .execute(&mut conn)
    .await
    .expect("create the test database");

    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(&mut conn)
        .await
        .expect("release the setup lock");
    let _ = conn.close().await;
}

async fn seed_tenant(owner_pool: &PgPool, slug: &str, name: &str) -> SeedTenant {
    let id = new_id();
    sqlx::query("INSERT INTO tenant (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .execute(owner_pool)
        .await
        .expect("seed tenant");
    SeedTenant {
        id,
        slug: slug.to_owned(),
        name: name.to_owned(),
    }
}

/// SQL string literal, for the few statements that cannot take parameters.
fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
