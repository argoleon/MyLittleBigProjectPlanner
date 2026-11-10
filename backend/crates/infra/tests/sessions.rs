//! Integration tests on a real Valkey: sessions and rate limits. Each test
//! uses its own key prefix (`testkit::valkey`).

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use fred::prelude::KeysInterface;
use infra::id::new_id;
use infra::rate_limit::RateLimiter;
use infra::sessions::{SessionLifetime, SessionStage, SessionStore};

const LONG: SessionLifetime = SessionLifetime {
    idle: Duration::from_secs(600),
    max_age: Duration::from_secs(3600),
};

async fn store(lifetime: SessionLifetime) -> (SessionStore, infra::valkey::Valkey, String) {
    let (valkey, prefix) = testkit::valkey().await;
    (
        SessionStore::new(valkey.clone(), prefix.clone(), lifetime),
        valkey,
        prefix,
    )
}

#[tokio::test]
async fn a_session_is_found_by_its_token_and_stored_under_its_hash() {
    let (store, valkey, prefix) = store(LONG).await;
    let (account, tenant) = (new_id(), new_id());
    let created = store.create(account, tenant).await.unwrap();

    let found = store.get(&created.token).await.unwrap().unwrap();
    assert_eq!(found, created.session);
    assert_eq!((found.account_id, found.tenant_id), (account, tenant));
    assert_ne!(found.csrf_token, created.token);
    assert!(store.get("not-a-token").await.unwrap().is_none());

    let raw: i64 = valkey
        .exists(format!("{prefix}session:{}", created.token))
        .await
        .unwrap();
    assert_eq!(raw, 0, "the token itself is never a key");
    let hashed: i64 = valkey
        .exists(format!(
            "{prefix}session:{}",
            infra::credentials::token_hash_hex(&created.token)
        ))
        .await
        .unwrap();
    assert_eq!(hashed, 1);
}

#[tokio::test]
async fn a_session_ends_after_its_idle_time_unless_used() {
    let (store, _, _) = store(SessionLifetime {
        idle: Duration::from_secs(2),
        max_age: Duration::from_secs(3600),
    })
    .await;
    let used = store.create(new_id(), new_id()).await.unwrap();
    let unused = store.create(new_id(), new_id()).await.unwrap();

    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert!(store.get(&used.token).await.unwrap().is_some(), "use extends it");
    }
    assert!(
        store.get(&unused.token).await.unwrap().is_none(),
        "idle for 2.7 s"
    );
}

#[tokio::test]
async fn a_session_ends_at_its_maximum_age_even_when_used() {
    let (store, _, _) = store(SessionLifetime {
        idle: Duration::from_secs(600),
        max_age: Duration::from_secs(1),
    })
    .await;
    let created = store.create(new_id(), new_id()).await.unwrap();
    assert!(store.get(&created.token).await.unwrap().is_some());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(store.get(&created.token).await.unwrap().is_none());
}

#[tokio::test]
async fn sign_out_ends_one_session_and_a_reset_ends_all_of_the_account() {
    let (store, _, _) = store(LONG).await;
    let (account, other) = (new_id(), new_id());
    let first = store.create(account, new_id()).await.unwrap();
    let second = store.create(account, new_id()).await.unwrap();
    let third = store.create(account, new_id()).await.unwrap();
    let others = store.create(other, new_id()).await.unwrap();

    store.delete(&first.token).await.unwrap();
    assert!(store.get(&first.token).await.unwrap().is_none());
    assert!(store.get(&second.token).await.unwrap().is_some());
    store.delete(&first.token).await.unwrap();

    assert_eq!(store.delete_all(account).await.unwrap(), 2);
    assert!(store.get(&second.token).await.unwrap().is_none());
    assert!(store.get(&third.token).await.unwrap().is_none());
    assert!(
        store.get(&others.token).await.unwrap().is_some(),
        "other accounts keep their sessions"
    );
    assert_eq!(store.delete_all(account).await.unwrap(), 0);
}

#[tokio::test]
async fn rate_limits_count_hits_in_a_fixed_window() {
    let (valkey, prefix) = testkit::valkey().await;
    let limiter = RateLimiter::new(valkey, prefix);
    let window = Duration::from_secs(60);

    assert_eq!(limiter.count("sign-in:a").await.unwrap(), 0);
    for expected in 1..=5 {
        assert_eq!(limiter.hit("sign-in:a", window).await.unwrap(), expected);
    }
    assert_eq!(limiter.count("sign-in:a").await.unwrap(), 5);
    assert_eq!(
        limiter.count("sign-in:b").await.unwrap(),
        0,
        "counters are separate"
    );
    let retry = limiter.retry_after("sign-in:a").await.unwrap();
    assert!(retry > Duration::from_secs(50) && retry <= window, "{retry:?}");

    limiter.clear("sign-in:a").await.unwrap();
    assert_eq!(limiter.count("sign-in:a").await.unwrap(), 0);
}

#[tokio::test]
async fn a_rate_limit_window_does_not_slide_with_new_hits() {
    let (valkey, prefix) = testkit::valkey().await;
    let limiter = RateLimiter::new(valkey, prefix);
    let window = Duration::from_secs(1);
    limiter.hit("ip", window).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    limiter.hit("ip", window).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        limiter.count("ip").await.unwrap(),
        0,
        "the window started at the first hit"
    );
}

#[tokio::test]
async fn a_session_waiting_for_its_second_factor_is_short_lived() {
    let (store, valkey, prefix) = store(LONG).await;
    let pending = store
        .create_staged(new_id(), new_id(), SessionStage::MfaChallenge)
        .await
        .unwrap();
    assert_eq!(pending.session.stage, SessionStage::MfaChallenge);
    let found = store.get(&pending.token).await.unwrap().unwrap();
    assert_eq!(found.stage, SessionStage::MfaChallenge);
    let lifetime = (found.expires_at - found.created_at).num_seconds();
    assert_eq!(lifetime, 300, "5 minutes, not the configured hour");
    let key = format!(
        "{prefix}session:{}",
        infra::credentials::token_hash_hex(&pending.token)
    );
    let ttl: i64 = valkey.ttl(&key).await.unwrap();
    assert!((1..=300).contains(&ttl), "{ttl}");

    let enrolling = store
        .create_staged(new_id(), new_id(), SessionStage::MfaEnrollment)
        .await
        .unwrap();
    assert_eq!(
        (enrolling.session.expires_at - enrolling.session.created_at).num_seconds(),
        900
    );
    let complete = store.create(new_id(), new_id()).await.unwrap();
    assert_eq!(complete.session.stage, SessionStage::Complete);
}

#[tokio::test]
async fn sessions_stored_before_stages_existed_are_complete() {
    let (store, valkey, prefix) = store(LONG).await;
    let created = store.create(new_id(), new_id()).await.unwrap();
    let key = format!(
        "{prefix}session:{}",
        infra::credentials::token_hash_hex(&created.token)
    );
    let mut json: serde_json::Value =
        serde_json::from_str(&valkey.get::<String, _>(&key).await.unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("stage");
    let _: () = valkey
        .set(&key, json.to_string(), None, None, false)
        .await
        .unwrap();
    assert_eq!(
        store.get(&created.token).await.unwrap().unwrap().stage,
        SessionStage::Complete
    );
}
