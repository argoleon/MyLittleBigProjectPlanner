//! Exports of a member's personal data (right of access, specification §14,
//! L02-US09).
//!
//! The API opens an export ([`ExportStore::open`]) and queues an
//! [`EXPORT_JOB`]; the worker [`collect`]s the data and completes the export
//! in Valkey, where it stays [`EXPORT_LIFETIME`] at most. The member
//! downloads it with the export's id: a random token, valid for them only.

use std::time::Duration;

use chrono::{DateTime, Utc};
use fred::prelude::KeysInterface;
use fred::types::{Expiration, SetOptions};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::credentials::{random_token, token_hash_hex};
use crate::valkey::{Valkey, ValkeyError};

/// Job collecting the personal data of an export.
pub const EXPORT_JOB: &str = "gdpr.export";

/// How long an export is kept, ready or not.
pub const EXPORT_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// Format of the exported document, for whoever reads it later.
pub const EXPORT_FORMAT: &str = "mlbpp.personal-data.v1";

/// Payload of [`EXPORT_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub export_id: String,
    pub account_id: Uuid,
}

/// An export in Valkey.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub account_id: Uuid,
    pub expires_at: DateTime<Utc>,
    /// `None` until the worker has collected the data.
    pub content: Option<serde_json::Value>,
}

/// Exports in Valkey, under keys starting with `prefix` and the SHA-256 of
/// their id.
#[derive(Debug, Clone)]
pub struct ExportStore {
    valkey: Valkey,
    prefix: String,
}

impl ExportStore {
    pub fn new(valkey: Valkey, prefix: impl Into<String>) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
        }
    }

    fn key(&self, id: &str) -> String {
        format!("{}export:{}", self.prefix, token_hash_hex(id))
    }

    /// Opens an export of `account_id`'s data: its id, and when it expires.
    pub async fn open(&self, account_id: Uuid) -> Result<(String, Export), ValkeyError> {
        let id = random_token();
        let export = Export {
            account_id,
            expires_at: Utc::now()
                + chrono::Duration::from_std(EXPORT_LIFETIME).unwrap_or(chrono::Duration::minutes(15)),
            content: None,
        };
        let _: () = self
            .valkey
            .set(
                self.key(&id),
                to_json(&export)?,
                Some(Expiration::EX(secs(EXPORT_LIFETIME))),
                None,
                false,
            )
            .await?;
        Ok((id, export))
    }

    /// The export `id`, if it has not expired.
    pub async fn get(&self, id: &str) -> Result<Option<Export>, ValkeyError> {
        let json: Option<String> = self.valkey.get(self.key(id)).await?;
        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// Stores `content` in the export `id`, which keeps its expiry; `false`
    /// when it has expired meanwhile.
    pub async fn complete(&self, id: &str, content: serde_json::Value) -> Result<bool, ValkeyError> {
        let Some(mut export) = self.get(id).await? else {
            return Ok(false);
        };
        export.content = Some(content);
        let stored: Option<String> = self
            .valkey
            .set(
                self.key(id),
                to_json(&export)?,
                Some(Expiration::KEEPTTL),
                Some(SetOptions::XX),
                false,
            )
            .await?;
        Ok(stored.is_some())
    }
}

/// The personal data of the account set as `app.account_id` in this
/// transaction, as the export document; `None` when it has no data (erased).
pub async fn collect(conn: &mut PgConnection) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let data = sqlx::query_scalar!(r#"SELECT app_my_personal_data() AS "data""#)
        .fetch_one(conn)
        .await?;
    Ok(data.map(|mut data| {
        if let Some(object) = data.as_object_mut() {
            object.insert("format".to_owned(), EXPORT_FORMAT.into());
            object.insert("exportedAt".to_owned(), Utc::now().to_rfc3339().into());
        }
        data
    }))
}

fn to_json(export: &Export) -> Result<String, ValkeyError> {
    serde_json::to_string(export).map_err(|e| ValkeyError::new(fred::error::ErrorKind::Parse, e.to_string()))
}

fn secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}
