//! Integration test: every erasure (L02-US09) is logged with the `audit`
//! target, its actor and the pseudonym. Alone in its test binary: `tracing`
//! caches per process which events a subscriber wants (see `logs.rs`).

#![allow(clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::{json, multi, start};
use infra::accounts::TenantRole;
use infra::config::LogFormat;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{TEST_PASSWORD, TestDb};
use tracing_subscriber::layer::SubscriberExt;

/// Captures what the log layer writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    /// The `audit` events, as JSON.
    fn audit(&self) -> Vec<Value> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event["target"] == "audit")
            .collect()
    }
}

// Current-thread runtime: the server tasks run on this thread, where the
// capturing subscriber is installed.
#[tokio::test(flavor = "current_thread")]
async fn every_erasure_is_audited_with_its_actor_and_pseudonym() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .with(infra::telemetry::log_layer(LogFormat::Json, move || {
            writer.clone()
        }));
    let _guard = tracing::subscriber::set_default(subscriber);

    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let admin = api.session(&db.tenant_a).member.account_id;

    // A tenant admin erases a member.
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let res = api
        .request(
            Method::POST,
            &db.tenant_a,
            &format!("/api/v1/tenant/members/{}/erasure", member.account_id),
        )
        .header("if-match", "*")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    // A member erases their account.
    let leaving = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: common::session_settings().idle,
            max_age: common::session_settings().max_age,
        },
    )
    .create(leaving.account_id, db.tenant_a.id)
    .await
    .unwrap();
    let (status, body) = json(
        api.anonymous(Method::POST, &db.tenant_a, "/api/v1/me/erasure")
            .header("cookie", format!("mlbpp_session={}", opened.token))
            .header("x-csrf-token", &opened.session.csrf_token)
            .json(&json!({"confirmation": leaving.email, "password": TEST_PASSWORD}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 204, "{body}");

    let audit = captured.audit();
    let event = |message: &str| {
        audit
            .iter()
            .find(|event| event["message"] == message)
            .unwrap_or_else(|| panic!("no audit event {message:?} in {audit:#?}"))
    };
    let tenant = event("member erased from the tenant");
    assert_eq!(tenant["actor"], admin.to_string());
    assert_eq!(tenant["account"], member.account_id.to_string());
    assert_eq!(tenant["scope"], "tenant");
    assert!(tenant["pseudonym"].as_i64().unwrap() > 0);
    let itself = event("account erased by itself");
    assert_eq!(itself["actor"], leaving.account_id.to_string());
    assert_eq!(itself["scope"], "account");
    assert!(itself["pseudonym"].as_i64().unwrap() > 0);
    // No personal data in the audit trail.
    let trail = serde_json::to_string(&audit).unwrap();
    assert!(!trail.contains(&member.email) && !trail.contains(&leaving.email));
}
