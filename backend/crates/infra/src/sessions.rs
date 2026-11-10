//! Sessions of the web front end, stored in Valkey (specification §14, §17).
//!
//! The browser holds a random token in an `httpOnly` cookie; Valkey holds
//! the session under the token's SHA-256, so a copy of Valkey reveals no
//! usable cookie. A session belongs to one account in one tenant. It ends
//! after `idle` without use, or `max_age` after sign-in at the latest; any
//! instance of the API can serve it.

use std::time::Duration;

use chrono::{DateTime, Utc};
use fred::prelude::{KeysInterface, SetsInterface};
use fred::types::Expiration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::credentials::{random_token, token_hash_hex};
use crate::valkey::{Valkey, ValkeyError};

/// A signed-in session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub account_id: Uuid,
    pub tenant_id: Uuid,
    /// Sent back by the front end in `X-CSRF-Token` on every change.
    pub csrf_token: String,
    pub created_at: DateTime<Utc>,
    /// Absolute end of the session, whatever its use.
    pub expires_at: DateTime<Utc>,
}

/// A session just created, with the token to give the browser.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub token: String,
    pub session: Session,
}

/// Lifetimes of the sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLifetime {
    /// Ends after this time without request.
    pub idle: Duration,
    /// Ends this long after sign-in at the latest.
    pub max_age: Duration,
}

/// Sessions in Valkey, under keys starting with `prefix`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    valkey: Valkey,
    prefix: String,
    lifetime: SessionLifetime,
}

impl SessionStore {
    pub fn new(valkey: Valkey, prefix: impl Into<String>, lifetime: SessionLifetime) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
            lifetime,
        }
    }

    pub fn lifetime(&self) -> SessionLifetime {
        self.lifetime
    }

    fn session_key(&self, hash: &str) -> String {
        format!("{}session:{hash}", self.prefix)
    }

    fn account_key(&self, account_id: Uuid) -> String {
        format!("{}account-sessions:{account_id}", self.prefix)
    }

    /// Opens a session for `account_id` in `tenant_id`.
    pub async fn create(&self, account_id: Uuid, tenant_id: Uuid) -> Result<NewSession, ValkeyError> {
        let token = random_token();
        let now = Utc::now();
        let max_age = chrono::Duration::from_std(self.lifetime.max_age).unwrap_or(chrono::Duration::days(7));
        let session = Session {
            account_id,
            tenant_id,
            csrf_token: random_token(),
            created_at: now,
            expires_at: now + max_age,
        };
        let hash = token_hash_hex(&token);
        let json = serde_json::to_string(&session)
            .map_err(|e| ValkeyError::new(fred::error::ErrorKind::Parse, e.to_string()))?;
        let _: () = self
            .valkey
            .set(
                self.session_key(&hash),
                json,
                Some(Expiration::EX(secs(self.lifetime.idle))),
                None,
                false,
            )
            .await?;
        // Index of the account's sessions, to revoke them all at once.
        let account_key = self.account_key(account_id);
        let _: () = self.valkey.sadd(&account_key, hash).await?;
        let _: () = self
            .valkey
            .expire(&account_key, secs(self.lifetime.max_age), None)
            .await?;
        Ok(NewSession { token, session })
    }

    /// The session of `token`, if it is still valid; extends its idle
    /// lifetime.
    pub async fn get(&self, token: &str) -> Result<Option<Session>, ValkeyError> {
        let hash = token_hash_hex(token);
        let key = self.session_key(&hash);
        let json: Option<String> = self.valkey.get(&key).await?;
        let Some(session) = json.and_then(|json| serde_json::from_str::<Session>(&json).ok()) else {
            return Ok(None);
        };
        let remaining = (session.expires_at - Utc::now()).to_std().unwrap_or_default();
        if remaining.is_zero() {
            self.forget(&hash, session.account_id).await?;
            return Ok(None);
        }
        let ttl = remaining.min(self.lifetime.idle);
        let _: () = self.valkey.expire(&key, secs(ttl).max(1), None).await?;
        Ok(Some(session))
    }

    /// Ends the session of `token` (sign-out).
    pub async fn delete(&self, token: &str) -> Result<(), ValkeyError> {
        let hash = token_hash_hex(token);
        let key = self.session_key(&hash);
        let json: Option<String> = self.valkey.get(&key).await?;
        match json.and_then(|json| serde_json::from_str::<Session>(&json).ok()) {
            Some(session) => self.forget(&hash, session.account_id).await,
            None => {
                let _: () = self.valkey.del(&key).await?;
                Ok(())
            }
        }
    }

    /// Ends every session of `account_id`, in every tenant (password reset,
    /// account erasure). Returns how many were open.
    pub async fn delete_all(&self, account_id: Uuid) -> Result<usize, ValkeyError> {
        let account_key = self.account_key(account_id);
        let hashes: Vec<String> = self.valkey.smembers(&account_key).await?;
        let mut deleted = 0;
        for hash in &hashes {
            let removed: i64 = self.valkey.del(self.session_key(hash)).await?;
            deleted += usize::try_from(removed).unwrap_or(0);
        }
        let _: () = self.valkey.del(&account_key).await?;
        Ok(deleted)
    }

    async fn forget(&self, hash: &str, account_id: Uuid) -> Result<(), ValkeyError> {
        let _: () = self.valkey.del(self.session_key(hash)).await?;
        let _: () = self.valkey.srem(self.account_key(account_id), hash).await?;
        Ok(())
    }
}

fn secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}
