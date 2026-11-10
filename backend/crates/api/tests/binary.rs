//! Integration tests: the compiled `api` binary, as Docker runs it.

// Test helpers sit outside `#[test]` functions; a panic there is a test failure.
#![allow(clippy::unwrap_used)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_api");

#[test]
fn unknown_command_prints_usage_and_exits_with_2() {
    let output = Command::new(BIN).env_clear().arg("migrat").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown command: migrat") && stderr.contains("Usage: api"),
        "{stderr}"
    );
}

#[test]
fn migrate_requires_the_owner_url() {
    let output = Command::new(BIN).env_clear().arg("migrate").output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("DATABASE_MIGRATION_URL"), "stderr was: {stderr}");
}

#[tokio::test]
async fn migrate_succeeds_on_an_up_to_date_database_and_changes_nothing() {
    let db = testkit::TestDb::new().await;
    let count = || async {
        infra::sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&db.owner_pool)
            .await
            .unwrap()
    };
    let before = count().await;

    let output = Command::new(BIN)
        .env_clear()
        .env("DATABASE_MIGRATION_URL", db.owner_url())
        .arg("migrate")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(count().await, before);
}

#[test]
fn migrate_reports_an_unreachable_database() {
    let output = Command::new(BIN)
        .env_clear()
        .env("DATABASE_MIGRATION_URL", "postgres://nobody@127.0.0.1:1/none")
        .arg("migrate")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("migration failed"), "stderr was: {stderr}");
}

#[test]
fn exits_with_an_error_naming_the_missing_variable() {
    let output = Command::new(BIN).env_clear().output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("API_BIND_ADDR"), "stderr was: {stderr}");
}

#[test]
fn exits_with_an_error_on_a_malformed_variable() {
    let output = Command::new(BIN)
        .env_clear()
        .env("API_BIND_ADDR", "127.0.0.1:1")
        .env("API_REQUEST_TIMEOUT_SECS", "soon")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("API_REQUEST_TIMEOUT_SECS"),
        "stderr was: {stderr}"
    );
}

/// The authentication settings `api serve` requires: the test Valkey (lazy:
/// the server starts without it) and a public URL.
fn with_auth_settings(command: &mut Command) -> &mut Command {
    command
        .env(
            "VALKEY_URL",
            std::env::var("TEST_VALKEY_URL").unwrap_or_else(|_| "redis://127.0.0.1:56379".to_owned()),
        )
        .env("PUBLIC_URL", "https://planner.example.test")
}

/// `api healthcheck` against a server bound to `port`: the exit status.
fn healthcheck(port: u16) -> std::process::ExitStatus {
    Command::new(BIN)
        .env_clear()
        .env("API_BIND_ADDR", format!("0.0.0.0:{port}"))
        .arg("healthcheck")
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn healthcheck_command_follows_the_running_server() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert!(!healthcheck(port).success(), "nothing listens yet");

    let mut child = with_auth_settings(Command::new(BIN).env_clear())
        .env("API_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("DATABASE_URL", "postgres://localhost/unused")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !healthcheck(port).success() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("healthcheck never succeeded");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let res = reqwest::get(format!("http://127.0.0.1:{port}/health/live"))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), r#"{"status":"ok"}"#);

    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!healthcheck(port).success(), "the server is gone");
}

/// Starts `api` on a free port with `database_url`; returns it and the port
/// once it answers its liveness probe.
async fn spawn_api(database_url: &str) -> (std::process::Child, u16) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = with_auth_settings(Command::new(BIN).env_clear())
        .env("API_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("DATABASE_URL", database_url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !healthcheck(port).success() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("api never became alive");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (child, port)
}

fn healthcheck_ready(port: u16) -> std::process::ExitStatus {
    Command::new(BIN)
        .env_clear()
        .env("API_BIND_ADDR", format!("0.0.0.0:{port}"))
        .args(["healthcheck", "--ready"])
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn healthcheck_ready_follows_the_database() {
    let db = testkit::TestDb::new().await;
    let (mut up, port) = spawn_api(&db.owner_url()).await;
    assert!(healthcheck_ready(port).success(), "the database answers");
    up.kill().unwrap();
    up.wait().unwrap();

    let (mut down, port) = spawn_api("postgres://app@127.0.0.1:1/mlbpp").await;
    assert!(healthcheck(port).success(), "alive without database");
    assert!(!healthcheck_ready(port).success(), "not ready without database");
    down.kill().unwrap();
    down.wait().unwrap();
}

#[test]
fn healthcheck_command_requires_the_bind_address() {
    let output = Command::new(BIN).env_clear().arg("healthcheck").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("API_BIND_ADDR"));
}

#[cfg(unix)]
#[tokio::test]
async fn serves_ping_and_stops_cleanly_on_sigterm() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = with_auth_settings(Command::new(BIN).env_clear())
        .env("API_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("SHUTDOWN_GRACE_SECS", "5")
        // Connections open lazily: ping and shutdown need no database.
        .env("DATABASE_URL", "postgres://localhost/unused")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let url = format!("http://127.0.0.1:{port}/api/v1/ping");
    let deadline = Instant::now() + Duration::from_secs(10);
    let res = loop {
        match reqwest::get(&url).await {
            Ok(res) => break res,
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(err) => {
                let _ = child.kill();
                panic!("api did not start: {err}");
            }
        }
    };
    assert_eq!(res.status(), 200);

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("api did not stop after SIGTERM");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(status.success(), "exit status: {status}");
}

/// Runs `api add-member` with `args`, the password on standard input.
fn add_member(database_url: &str, args: &[&str], password: &str) -> std::process::Output {
    use std::io::Write;
    let mut child = Command::new(BIN)
        .env_clear()
        .env("DATABASE_URL", database_url)
        .arg("add-member")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{password}").unwrap();
    child.wait_with_output().unwrap()
}

#[tokio::test]
async fn add_member_creates_an_internal_member_who_can_sign_in() {
    let db = testkit::TestDb::new().await;
    let output = add_member(
        &db.app_url(),
        &[
            "alpha",
            "Admin@Alpha.example",
            "Alice Admin",
            "--role",
            "tenant_admin",
        ],
        "a long enough passphrase",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("added"));

    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    let found = infra::accounts::find_by_email(&mut tx, "admin@alpha.example")
        .await
        .unwrap()
        .expect("the member exists, its email normalized");
    assert_eq!(found.member.display_name, "Alice Admin");
    assert_eq!(found.member.tenant_role, infra::accounts::TenantRole::TenantAdmin);
    assert_eq!(found.member.account_type, infra::accounts::AccountType::Internal);
    assert!(infra::credentials::verify_password(
        "a long enough passphrase",
        found.password_hash.as_deref().unwrap()
    ));
    tx.rollback().await.unwrap();

    // The same email in another tenant links the account, password kept.
    let output = add_member(
        &db.app_url(),
        &["beta", "admin@alpha.example", "Alice"],
        "another passphrase!",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("password is unchanged"));
}

#[tokio::test]
async fn add_member_rejects_weak_passwords_and_unknown_tenants() {
    let db = testkit::TestDb::new().await;
    let short = add_member(&db.app_url(), &["alpha", "a@alpha.example", "A"], "short");
    assert!(!short.status.success());
    assert!(String::from_utf8_lossy(&short.stderr).contains("password: too_short"));

    let unknown = add_member(
        &db.app_url(),
        &["gamma", "a@alpha.example", "A"],
        "a long enough passphrase",
    );
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no tenant with the slug gamma"));

    let count: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM account")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was created");
}
