//! The real-time event log (specification §12, L05-US08): a bounded Valkey
//! Stream per tenant, `<prefix>events:<tenant>`.
//!
//! The worker appends a signal once a change is committed and dispatched
//! (`crate::notifications`): which ticket changed, how, by whom, and who has
//! new notifications. Every API instance reads the streams of the tenants
//! its browsers follow and pushes the signals they may see over
//! Server-Sent Events; a browser that comes back after a cut gives the last
//! id it saw and gets exactly what it missed, as long as the stream still
//! holds it ([`MAX_LEN`]); otherwise it reads everything again. Signals
//! carry no data: the browser reads it through the API, with its rights.

use std::collections::HashMap;

use fred::prelude::StreamsInterface;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::valkey::{Valkey, ValkeyError};

/// Signals kept per tenant: well over an hour of activity.
pub const MAX_LEN: i64 = 10_000;
/// The field of a stream entry holding the signal (JSON).
const FIELD: &str = "signal";

/// What changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signal {
    /// `issue`: a ticket changed; `notifications`: some members have new ones.
    pub topic: Topic,
    /// The outbox event kind: `issue.updated`, `comment.created`…
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub issue_id: Option<Uuid>,
    #[serde(default)]
    pub issue_key: Option<String>,
    /// The project of the ticket: who may see the signal.
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub actor_id: Option<Uuid>,
    /// The members with new notifications (`notifications` only).
    #[serde(default)]
    pub recipients: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Issue,
    Notifications,
}

/// A signal with its stream id (`1700000000000-0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub signal: Signal,
}

/// What [`EventLog::since`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Since {
    pub entries: Vec<Entry>,
    /// Some entries after the id asked for were trimmed already: the reader
    /// must read everything again.
    pub gap: bool,
}

/// Compares two stream ids (`ms-seq`); unparsable ids sort first.
pub fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |id: &str| -> (u64, u64) {
        let (ms, seq) = id.split_once('-').unwrap_or((id, "0"));
        (ms.parse().unwrap_or(0), seq.parse().unwrap_or(0))
    };
    parse(a).cmp(&parse(b))
}

/// Whether `id` looks like a stream id, as a browser sends it back.
pub fn is_stream_id(id: &str) -> bool {
    let mut parts = id.split('-');
    let ok = |part: Option<&str>| {
        part.is_some_and(|p| !p.is_empty() && p.len() <= 20 && p.bytes().all(|b| b.is_ascii_digit()))
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

type RawEntries = Vec<(String, HashMap<String, String>)>;

fn entries_of(raw: RawEntries) -> Vec<Entry> {
    raw.into_iter()
        .filter_map(|(id, fields)| {
            let signal = serde_json::from_str(fields.get(FIELD)?).ok()?;
            Some(Entry { id, signal })
        })
        .collect()
}

/// The event log of every tenant.
#[derive(Clone)]
pub struct EventLog {
    valkey: Valkey,
    prefix: String,
}

impl std::fmt::Debug for EventLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLog")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl EventLog {
    pub fn new(valkey: Valkey, prefix: impl Into<String>) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
        }
    }

    pub fn key(&self, tenant: Uuid) -> String {
        format!("{}events:{tenant}", self.prefix)
    }

    /// Appends `signal` to the log of `tenant`; its id.
    pub async fn append(&self, tenant: Uuid, signal: &Signal) -> Result<String, ValkeyError> {
        let json = serde_json::to_string(signal).unwrap_or_default();
        self.valkey
            .xadd(
                self.key(tenant),
                false,
                ("MAXLEN", "~", MAX_LEN),
                "*",
                vec![(FIELD, json)],
            )
            .await
    }

    /// The id of the last signal of `tenant` (`0-0` when none): reading
    /// starts after it.
    pub async fn last_id(&self, tenant: Uuid) -> Result<String, ValkeyError> {
        let last: RawEntries = self.valkey.xrevrange(self.key(tenant), "+", "-", Some(1)).await?;
        Ok(last
            .into_iter()
            .next()
            .map_or_else(|| "0-0".to_owned(), |(id, _)| id))
    }

    /// The signals of `tenant` after `after`, oldest first, at most
    /// `MAX_LEN`; whether some were trimmed meanwhile.
    pub async fn since(&self, tenant: Uuid, after: &str) -> Result<Since, ValkeyError> {
        let key = self.key(tenant);
        let first: RawEntries = self.valkey.xrange(key.as_str(), "-", "+", Some(1)).await?;
        // The oldest kept is after the last one seen: that one was trimmed,
        // and maybe others after it.
        let gap = after != "0-0"
            && first
                .first()
                .is_some_and(|(oldest, _)| id_order(oldest, after) == std::cmp::Ordering::Greater);
        let raw: RawEntries = self
            .valkey
            .xrange(
                key.as_str(),
                format!("({after}"),
                "+",
                Some(u64::try_from(MAX_LEN).unwrap_or(10_000)),
            )
            .await?;
        Ok(Since {
            entries: entries_of(raw),
            gap,
        })
    }

    /// A connection of its own for [`EventLog::wait`], connecting in the
    /// background.
    pub fn reader(&self) -> Valkey {
        let reader = self.valkey.clone_new();
        drop(fred::prelude::ClientLike::connect(&reader));
        reader
    }

    /// Closes a connection made by [`EventLog::reader`].
    pub async fn close(reader: &Valkey) {
        drop(fred::prelude::ClientLike::quit(reader).await);
    }

    /// Waits up to `block_ms` for signals of `tenant` after `after`. Blocks
    /// its connection: use a client of its own ([`Valkey::clone_new`]).
    pub async fn wait(
        reader: &Valkey,
        key: &str,
        after: &str,
        block_ms: u64,
    ) -> Result<Vec<Entry>, ValkeyError> {
        let mut read: fred::types::streams::XReadResponse<String, String, String, String> =
            reader.xread_map(Some(100), Some(block_ms), key, after).await?;
        Ok(read.remove(key).map(entries_of).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_ids_compare_as_numbers() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(id_order("1700000000000-1", "1700000000000-0"), Greater);
        assert_eq!(id_order("999-9", "1000-0"), Less);
        assert_eq!(id_order("5-0", "5-0"), Equal);
        assert!(is_stream_id("1700000000000-12"));
        assert!(!is_stream_id("1700000000000"));
        assert!(!is_stream_id("abc-1"));
        assert!(!is_stream_id("1-2-3"));
        assert!(!is_stream_id(""));
    }

    #[test]
    fn signals_keep_no_more_than_they_need() {
        let signal = Signal {
            topic: Topic::Notifications,
            kind: None,
            issue_id: None,
            issue_key: None,
            project_id: None,
            actor_id: None,
            recipients: vec![Uuid::from_u128(1)],
        };
        let json = serde_json::to_value(&signal).unwrap();
        assert_eq!(json["topic"], "notifications");
        assert_eq!(serde_json::from_value::<Signal>(json).unwrap(), signal);
    }
}
