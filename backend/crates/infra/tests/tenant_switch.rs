//! Integration tests on a real Valkey: pending tenant switches and the
//! proofs a session carries.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use fred::prelude::KeysInterface;
use infra::id::new_id;
use infra::sessions::{Proof, SessionLifetime, SessionStage, SessionStore};
use infra::tenant_switch::{PendingSwitch, SWITCH_LIFETIME, SwitchStore};

fn pending() -> PendingSwitch {
    PendingSwitch {
        account_id: new_id(),
        target_tenant_id: new_id(),
        second_factor: true,
        sso: false,
    }
}

#[tokio::test]
async fn a_switch_is_taken_once() {
    let (valkey, prefix) = testkit::valkey().await;
    let store = SwitchStore::new(valkey.clone(), prefix.clone());
    let token = infra::credentials::random_token();
    let switch = pending();
    store.save(&token, &switch).await.unwrap();

    // Stored under the token's hash, for a minute.
    let key = format!(
        "{prefix}tenant-switch:{}",
        infra::credentials::token_hash_hex(&token)
    );
    let ttl: i64 = valkey.ttl(&key).await.unwrap();
    assert!(
        ttl > 0 && ttl <= i64::try_from(SWITCH_LIFETIME.as_secs()).unwrap(),
        "{ttl}"
    );
    let raw: i64 = valkey
        .exists(format!("{prefix}tenant-switch:{token}"))
        .await
        .unwrap();
    assert_eq!(raw, 0);

    assert_eq!(store.take(&token).await.unwrap(), Some(switch));
    assert_eq!(store.take(&token).await.unwrap(), None);
    assert_eq!(store.take("unknown").await.unwrap(), None);
}

#[tokio::test]
async fn a_session_keeps_what_it_proved() {
    let (valkey, prefix) = testkit::valkey().await;
    let store = SessionStore::new(
        valkey.clone(),
        prefix.clone(),
        SessionLifetime {
            idle: Duration::from_secs(600),
            max_age: Duration::from_secs(3600),
        },
    );
    let proof = Proof {
        second_factor: true,
        sso: true,
    };
    let opened = store
        .create_proven(new_id(), new_id(), SessionStage::Complete, proof)
        .await
        .unwrap();
    let found = store.get(&opened.token).await.unwrap().unwrap();
    assert_eq!(found.proof(), proof);

    // A password sign-in proves nothing more than the password.
    let plain = store.create(new_id(), new_id()).await.unwrap();
    assert_eq!(plain.session.proof(), Proof::default());

    // Sessions stored before proofs existed proved nothing.
    let key = format!(
        "{prefix}session:{}",
        infra::credentials::token_hash_hex(&opened.token)
    );
    let mut json: serde_json::Value =
        serde_json::from_str(&valkey.get::<String, _>(&key).await.unwrap()).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("second_factor");
    object.remove("sso");
    let _: () = valkey
        .set(&key, json.to_string(), None, None, false)
        .await
        .unwrap();
    assert_eq!(
        store.get(&opened.token).await.unwrap().unwrap().proof(),
        Proof::default()
    );
}
