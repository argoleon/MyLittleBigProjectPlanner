//! Shared helpers for integration tests (dev-dependency only).
//!
//! [`TestDb::new`] gives each test its own empty PostgreSQL database with
//! every migration applied and two tenants seeded (`alpha` and `beta`).
//! Tests can therefore run in parallel without seeing each other's data.
//! The database is dropped when the `TestDb` goes out of scope.
//!
//! Run the tests with `just test`: in the dev container,
//! `TEST_DATABASE_ADMIN_URL` points to the `postgres` service of
//! `deploy/compose.dev.yaml`. To use another server, set it to a superuser
//! URL, and
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

    /// Connection URL of the runtime role on this database, for tests that
    /// start a binary (`api add-member`).
    pub fn app_url(&self) -> String {
        format!(
            "postgres://{APP_ROLE}:{}@{}:{}/{}",
            app_password(),
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
        // The development server of deploy/compose.dev.yaml, seen from the host.
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
        panic!(
            "cannot reach the test PostgreSQL server ({e}); run the tests with `just test`, which starts it"
        )
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

// --- Members ----------------------------------------------------------------

/// Password of the members added by [`TestDb::add_member`].
pub const TEST_PASSWORD: &str = "correct horse battery staple";

/// A member created for a test, with the password it signs in with.
#[derive(Debug, Clone)]
pub struct TestMember {
    pub account_id: Uuid,
    pub email: String,
    pub password: String,
}

impl TestDb {
    /// Adds an internal member to `tenant`, with a fresh account (unique
    /// email) and the password `correct horse battery staple`.
    pub async fn add_member(&self, tenant: &SeedTenant, role: infra::accounts::TenantRole) -> TestMember {
        self.add_member_with(tenant, role, infra::accounts::AccountType::Internal)
            .await
    }

    /// Adds a member of the given account type and role to `tenant`.
    pub async fn add_member_with(
        &self,
        tenant: &SeedTenant,
        role: infra::accounts::TenantRole,
        account_type: infra::accounts::AccountType,
    ) -> TestMember {
        let email = format!("member-{}@{}.test", new_id().simple(), tenant.slug);
        let password = TEST_PASSWORD.to_owned();
        // Hashed once per test binary: Argon2id is slow on purpose.
        static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let hash = HASH.get_or_init(|| infra::credentials::hash_password(TEST_PASSWORD).expect("hash"));
        let mut tx = infra::db::begin_tenant_tx(&self.app_pool, tenant.id)
            .await
            .expect("tenant transaction");
        let added = infra::accounts::add_member(
            &mut tx,
            &infra::accounts::NewMember {
                email: &email,
                display_name: "Test Member",
                password_hash: Some(hash),
                account_type,
                tenant_role: role,
                local_account: false,
            },
        )
        .await
        .expect("add member");
        tx.commit().await.expect("commit");
        TestMember {
            account_id: added.account_id,
            email,
            password,
        }
    }
}

impl TestDb {
    /// Makes `account_id` a platform super admin (the bootstrap's job in
    /// production).
    pub async fn make_platform_admin(&self, account_id: Uuid) {
        sqlx::query("INSERT INTO platform_admin (account_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(account_id)
            .execute(&self.owner_pool)
            .await
            .expect("make platform admin");
    }
}

// --- Valkey -----------------------------------------------------------------

/// Valkey of the integration tests, with a key prefix unique to the test so
/// that parallel tests never share keys. `TEST_VALKEY_URL` (default
/// `redis://127.0.0.1:56379`, the port of `deploy/compose.dev.yaml`).
pub async fn valkey() -> (infra::valkey::Valkey, String) {
    let url = std::env::var("TEST_VALKEY_URL").unwrap_or_else(|_| "redis://127.0.0.1:56379".to_owned());
    let client = infra::valkey::connect(&infra::config::Secret::new(url))
        .await
        .expect("connect to the test Valkey");
    (client, format!("test:{}:", new_id().simple()))
}

// --- Mailpit ----------------------------------------------------------------

/// SMTP URL of the test mail catcher (`TEST_SMTP_URL`, default the Mailpit
/// port of `deploy/compose.dev.yaml`).
pub fn smtp_url() -> String {
    std::env::var("TEST_SMTP_URL").unwrap_or_else(|_| "smtp://127.0.0.1:11025".to_owned())
}

/// An email caught by Mailpit.
#[derive(Debug, Clone)]
pub struct CaughtEmail {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Waits (up to 10 s) for the email sent to `recipient` and returns it.
/// Recipients are unique per test, so tests never read each other's emails.
pub async fn caught_email(recipient: &str) -> CaughtEmail {
    let base = std::env::var("TEST_MAILPIT_URL").unwrap_or_else(|_| "http://127.0.0.1:18025".to_owned());
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let search: serde_json::Value = client
            .get(format!("{base}/api/v1/search"))
            .query(&[("query", format!("to:\"{recipient}\""))])
            .send()
            .await
            .expect("query Mailpit")
            .json()
            .await
            .expect("Mailpit answer");
        if let Some(id) = search["messages"][0]["ID"].as_str() {
            let message: serde_json::Value = client
                .get(format!("{base}/api/v1/message/{id}"))
                .send()
                .await
                .expect("read the message")
                .json()
                .await
                .expect("message JSON");
            return CaughtEmail {
                subject: message["Subject"].as_str().unwrap_or_default().to_owned(),
                text: message["Text"].as_str().unwrap_or_default().to_owned(),
                html: message["HTML"].as_str().unwrap_or_default().to_owned(),
            };
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no email to {recipient} after 10 s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Deletes the emails Mailpit caught for `recipient`.
pub async fn delete_caught_emails(recipient: &str) {
    let base = std::env::var("TEST_MAILPIT_URL").unwrap_or_else(|_| "http://127.0.0.1:18025".to_owned());
    let res = reqwest::Client::new()
        .delete(format!("{base}/api/v1/search"))
        .query(&[("query", format!("to:\"{recipient}\""))])
        .send()
        .await
        .expect("delete in Mailpit");
    assert!(res.status().is_success(), "Mailpit answered {}", res.status());
}

// --- OIDC provider (Keycloak) -------------------------------------------------

/// Issuer of the test OIDC provider: the realm `mlbpp-test` of the Keycloak
/// of `deploy/compose.dev.yaml` (`TEST_OIDC_ISSUER`).
pub fn oidc_issuer() -> String {
    std::env::var("TEST_OIDC_ISSUER").unwrap_or_else(|_| "http://keycloak:8080/realms/mlbpp-test".to_owned())
}

/// Client of the test realm (`deploy/dev/keycloak/mlbpp-test-realm.json`).
pub const OIDC_CLIENT_ID: &str = "mlbpp";
pub const OIDC_CLIENT_SECRET: &str = "test-only-client-secret"; // gitleaks:allow (test realm)

/// A user of the test realm.
#[derive(Debug, Clone, Copy)]
pub struct OidcUser {
    pub username: &'static str,
    pub password: &'static str,
    pub email: &'static str,
}

/// Email verified by the provider.
pub const ALICE: OidcUser = OidcUser {
    username: "alice",
    password: "alice-test-password", // gitleaks:allow (test realm)
    email: "alice@acme.test",
};
/// Email not verified by the provider.
pub const BOB: OidcUser = OidcUser {
    username: "bob",
    password: "bob-test-password", // gitleaks:allow (test realm)
    email: "bob@acme.test",
};
/// Email verified, member of no test tenant.
pub const CAROL: OidcUser = OidcUser {
    username: "carol",
    password: "carol-test-password", // gitleaks:allow (test realm)
    email: "carol@outsider.test",
};

/// Signs `user` in on the provider's page at `authorization_url`, as a
/// browser would (cookies, login form), and returns where the provider sends
/// the browser back: the redirect URI with `code` and `state`.
pub async fn provider_login(authorization_url: &str, user: OidcUser) -> String {
    let browser = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("browser");
    let page = browser
        .get(authorization_url)
        .send()
        .await
        .expect("the provider's login page");
    assert_eq!(page.status(), 200, "login page of {authorization_url}");
    let html = page.text().await.expect("login page");
    let form = html
        .find("id=\"kc-form-login\"")
        .map(|at| &html[at..])
        .expect("Keycloak's login form");
    let start = form.find("action=\"").expect("form action") + "action=\"".len();
    let end = start + form[start..].find('"').expect("end of the form action");
    let action = form[start..end].replace("&amp;", "&");
    let answer = browser
        .post(action)
        .form(&[
            ("username", user.username),
            ("password", user.password),
            ("credentialId", ""),
        ])
        .send()
        .await
        .expect("login form submission");
    assert_eq!(answer.status(), 302, "the provider accepts {}", user.username);
    answer.headers()["location"]
        .to_str()
        .expect("redirect URL")
        .to_owned()
}
