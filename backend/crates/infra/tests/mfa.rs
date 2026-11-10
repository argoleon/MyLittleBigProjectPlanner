//! Integration tests on a real PostgreSQL: two-step verification state and
//! the tenant's authentication policy, under row-level security.

#![allow(clippy::unwrap_used)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use infra::accounts::TenantRole;
use infra::config::Secret;
use infra::db::begin_tenant_tx;
use infra::mfa::{self, MfaCipher};
use infra::sqlx;
use testkit::TestDb;

fn cipher() -> MfaCipher {
    MfaCipher::new(&Secret::new(STANDARD.encode([42_u8; 32]))).unwrap()
}

#[tokio::test]
async fn enrollment_is_enabled_by_its_first_code_with_backup_codes() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let account = member.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(!mfa::is_enabled(&mut tx, account).await.unwrap());

    let secret = mfa::generate_secret();
    let (ciphertext, nonce) = cipher().encrypt(account, &secret).unwrap();
    assert!(
        mfa::start_enrollment(&mut tx, account, &ciphertext, &nonce)
            .await
            .unwrap()
    );
    let stored = mfa::find(&mut tx, account).await.unwrap().unwrap();
    assert!(!stored.enabled, "pending until confirmed");
    assert_eq!(
        cipher()
            .decrypt(account, &stored.secret_ciphertext, &stored.secret_nonce)
            .unwrap(),
        secret
    );
    // Restarting replaces the pending secret.
    let (other, other_nonce) = cipher().encrypt(account, &mfa::generate_secret()).unwrap();
    assert!(
        mfa::start_enrollment(&mut tx, account, &other, &other_nonce)
            .await
            .unwrap()
    );

    let codes = mfa::generate_backup_codes();
    assert!(mfa::enable(&mut tx, account, 1000, &codes).await.unwrap());
    assert!(mfa::is_enabled(&mut tx, account).await.unwrap());
    assert_eq!(mfa::backup_codes_left(&mut tx, account).await.unwrap(), 8);
    assert!(
        !mfa::enable(&mut tx, account, 1001, &codes).await.unwrap(),
        "already enabled"
    );
    // Once enabled, a new enrollment cannot replace the secret.
    assert!(
        !mfa::start_enrollment(&mut tx, account, &ciphertext, &nonce)
            .await
            .unwrap()
    );

    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT code_hash FROM account_backup_code")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(
        stored
            .iter()
            .all(|hash| hash.len() == 32 && !codes.iter().any(|c| c.as_bytes() == hash.as_slice())),
        "only hashes are stored"
    );
}

#[tokio::test]
async fn a_backup_code_works_once_and_a_time_step_once() {
    let db = TestDb::new().await;
    let account = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (ciphertext, nonce) = cipher().encrypt(account, &mfa::generate_secret()).unwrap();
    mfa::start_enrollment(&mut tx, account, &ciphertext, &nonce)
        .await
        .unwrap();
    let codes = mfa::generate_backup_codes();
    mfa::enable(&mut tx, account, 1000, &codes).await.unwrap();

    assert!(mfa::use_backup_code(&mut tx, account, &codes[0]).await.unwrap());
    assert!(
        !mfa::use_backup_code(&mut tx, account, &codes[0]).await.unwrap(),
        "used"
    );
    assert!(
        !mfa::use_backup_code(&mut tx, account, "zzzzzzzzzz")
            .await
            .unwrap(),
        "unknown"
    );
    assert_eq!(mfa::backup_codes_left(&mut tx, account).await.unwrap(), 7);

    assert!(
        !mfa::record_step(&mut tx, account, 1000).await.unwrap(),
        "the enrollment code"
    );
    assert!(mfa::record_step(&mut tx, account, 1001).await.unwrap());
    assert!(
        !mfa::record_step(&mut tx, account, 1001).await.unwrap(),
        "no replay"
    );
    assert!(
        !mfa::record_step(&mut tx, account, 999).await.unwrap(),
        "nor an older code"
    );

    mfa::disable(&mut tx, account).await.unwrap();
    assert!(mfa::find(&mut tx, account).await.unwrap().is_none());
    assert_eq!(mfa::backup_codes_left(&mut tx, account).await.unwrap(), 0);
}

#[tokio::test]
async fn other_tenants_see_nothing_of_an_account_they_do_not_share() {
    let db = TestDb::new().await;
    let account = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let (ciphertext, nonce) = cipher().encrypt(account, &mfa::generate_secret()).unwrap();
    mfa::start_enrollment(&mut tx, account, &ciphertext, &nonce)
        .await
        .unwrap();
    mfa::enable(&mut tx, account, 1, &mfa::generate_backup_codes())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(mfa::find(&mut tx, account).await.unwrap().is_none());
    assert_eq!(mfa::backup_codes_left(&mut tx, account).await.unwrap(), 0);
    let (ciphertext, nonce) = cipher().encrypt(account, &mfa::generate_secret()).unwrap();
    assert!(
        mfa::start_enrollment(&mut tx, account, &ciphertext, &nonce)
            .await
            .is_err(),
        "nor can they write it"
    );
}

#[tokio::test]
async fn every_tenant_has_a_policy_that_its_admin_can_change() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let policy = mfa::auth_policy(&mut tx).await.unwrap();
    assert!(!policy.local_accounts_require_mfa, "off by default");

    let changed = mfa::set_auth_policy(&mut tx, true, Some(&[policy.version]))
        .await
        .unwrap()
        .unwrap();
    assert!(changed.local_accounts_require_mfa);
    assert_eq!(changed.version, policy.version + 1);
    assert_eq!(
        mfa::set_auth_policy(&mut tx, false, Some(&[policy.version]))
            .await
            .unwrap(),
        None,
        "stale version"
    );
    tx.commit().await.unwrap();

    // Each tenant has its own.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        !mfa::auth_policy(&mut tx)
            .await
            .unwrap()
            .local_accounts_require_mfa
    );
    tx.rollback().await.unwrap();

    // A tenant created later gets one too.
    let admin = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    db.make_platform_admin(admin.account_id).await;
    let mut conn = db.app_pool.acquire().await.unwrap();
    let settings =
        infra::platform::TenantSettings::from_valid(&domain::tenant::TenantSettingsInput::defaults("Acme"))
            .unwrap();
    let acme = infra::platform::create(
        &mut conn,
        admin.account_id,
        infra::id::new_id(),
        "acme",
        &settings,
    )
    .await
    .unwrap()
    .unwrap();
    drop(conn);
    let mut tx = begin_tenant_tx(&db.app_pool, acme.id).await.unwrap();
    assert!(
        !mfa::auth_policy(&mut tx)
            .await
            .unwrap()
            .local_accounts_require_mfa
    );
}
