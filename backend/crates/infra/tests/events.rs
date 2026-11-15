//! Integration tests against a real Valkey: the real-time event log
//! (L05-US08). Signals appended per tenant and read back in order, after a
//! given id; a reader waiting for the next ones; a tenant's log apart from
//! another's; a resume after trimmed signals telling the gap.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use fred::prelude::StreamsInterface;
use infra::events::{EventLog, Signal, Topic};
use infra::id::new_id;
use infra::uuid::Uuid;

fn ticket(key: &str, project: Uuid) -> Signal {
    Signal {
        topic: Topic::Issue,
        kind: Some("issue.updated".to_owned()),
        issue_id: Some(new_id()),
        issue_key: Some(key.to_owned()),
        project_id: Some(project),
        actor_id: Some(new_id()),
        recipients: Vec::new(),
    }
}

#[tokio::test]
async fn signals_are_kept_per_tenant_and_read_back_after_an_id() {
    let (valkey, prefix) = testkit::valkey().await;
    let log = EventLog::new(valkey, prefix);
    let (alpha, beta, project) = (new_id(), new_id(), new_id());
    assert_eq!(log.last_id(alpha).await.unwrap(), "0-0", "nothing yet");

    let first = log.append(alpha, &ticket("PLAT-1", project)).await.unwrap();
    let second = log.append(alpha, &ticket("PLAT-2", project)).await.unwrap();
    log.append(beta, &ticket("OTHER-1", project)).await.unwrap();
    assert_eq!(log.last_id(alpha).await.unwrap(), second);

    let all = log.since(alpha, "0-0").await.unwrap();
    assert!(!all.gap);
    let keys: Vec<_> = all
        .entries
        .iter()
        .map(|e| e.signal.issue_key.clone().unwrap())
        .collect();
    assert_eq!(keys, ["PLAT-1", "PLAT-2"], "beta's are apart");
    let after_first = log.since(alpha, &first).await.unwrap();
    assert_eq!(after_first.entries.len(), 1);
    assert_eq!(after_first.entries[0].id, second);
    assert!(log.since(alpha, &second).await.unwrap().entries.is_empty());
}

#[tokio::test]
async fn a_reader_waits_for_the_next_signals() {
    let (valkey, prefix) = testkit::valkey().await;
    let log = EventLog::new(valkey, prefix);
    let tenant = new_id();
    let start = log.last_id(tenant).await.unwrap();
    let reader = log.reader();
    let key = log.key(tenant);
    let waiting = {
        let (reader, key, start) = (reader.clone(), key.clone(), start.clone());
        tokio::spawn(async move {
            for _ in 0..10 {
                let entries = EventLog::wait(&reader, &key, &start, 500).await.unwrap();
                if !entries.is_empty() {
                    return entries;
                }
            }
            Vec::new()
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let notified = Signal {
        topic: Topic::Notifications,
        kind: None,
        issue_id: None,
        issue_key: None,
        project_id: None,
        actor_id: None,
        recipients: vec![new_id()],
    };
    let id = log.append(tenant, &notified).await.unwrap();
    let entries = tokio::time::timeout(Duration::from_secs(6), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].id.as_str(), &entries[0].signal),
        (id.as_str(), &notified)
    );
    EventLog::close(&reader).await;
}

#[tokio::test]
async fn a_resume_past_trimmed_signals_tells_the_gap() {
    let (valkey, prefix) = testkit::valkey().await;
    let log = EventLog::new(valkey.clone(), prefix);
    let (tenant, project) = (new_id(), new_id());
    let first = log.append(tenant, &ticket("PLAT-1", project)).await.unwrap();
    let second = log.append(tenant, &ticket("PLAT-2", project)).await.unwrap();
    log.append(tenant, &ticket("PLAT-3", project)).await.unwrap();
    // As MAXLEN trims a busy log: only the last signal stays.
    let _: i64 = valkey.xtrim(log.key(tenant), ("MAXLEN", 1)).await.unwrap();
    let resumed = log.since(tenant, &first).await.unwrap();
    assert!(resumed.gap, "PLAT-2 was lost");
    assert_eq!(resumed.entries.len(), 1);
    let after_second = log.since(tenant, &second).await.unwrap();
    assert!(after_second.gap, "{second} itself is gone: maybe more");
}
