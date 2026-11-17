//! Sessions of the web front end, stored in Valkey (specification §14, §17).
//!
//! The browser holds a random token in an `httpOnly` cookie; Valkey holds
//! the session under the token's SHA-256, so a copy of Valkey reveals no
//! usable cookie. A session belongs to one account in one tenant. It ends
//! after `idle` without use, or `max_age` after sign-in at the latest; any
//! instance of the API can serve it.
//!
//! The member sees their sessions (L02-US08) with the device and the last
//! activity, recorded by [`SessionStore::touch`] at most once a minute, and
//! revokes them by a public id: a hash of the storage key, so that the list
//! gives nothing that opens or finds a session in Valkey.

use std::time::Duration;

pub use domain::auth_policy::Proof;
pub use domain::mfa::SessionStage;
use fred::prelude::{KeysInterface, SetsInterface};
use fred::types::Expiration;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
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
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Absolute end of the session, whatever its use.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// Only a `Complete` session reaches the application; the others wait
    /// for two-step verification. Sessions stored before stages existed
    /// were complete.
    #[serde(default = "complete", with = "stage_serde")]
    pub stage: SessionStage,
    /// What the sign-in proved, carried over when switching tenants.
    #[serde(default)]
    pub second_factor: bool,
    #[serde(default)]
    pub sso: bool,
    /// `User-Agent` of the latest request, for the list of sessions.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Latest request, to the minute ([`ACTIVITY_STEP`]); `None` for
    /// sessions stored before it was recorded.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
}

impl Session {
    pub fn proof(&self) -> Proof {
        Proof {
            second_factor: self.second_factor,
            sso: self.sso,
        }
    }
}

fn complete() -> SessionStage {
    SessionStage::Complete
}

/// `SessionStage` as its name (`domain` has no serde).
mod stage_serde {
    use domain::mfa::SessionStage;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(stage: &SessionStage, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(stage.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SessionStage, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "complete" => Ok(SessionStage::Complete),
            "mfa_challenge" => Ok(SessionStage::MfaChallenge),
            "mfa_enrollment" => Ok(SessionStage::MfaEnrollment),
            other => Err(serde::de::Error::custom(format!("unknown session stage {other}"))),
        }
    }
}

/// How often the last activity of a session is written: a request within
/// this time of the previous record only extends the session.
pub const ACTIVITY_STEP: Duration = Duration::from_secs(60);

/// A session of the account, as its list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedSession {
    /// Public id: revokes it through [`SessionStore::delete_by_id`].
    pub id: String,
    pub session: Session,
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

    /// Lifetime of a session at `stage`.
    pub fn lifetime_of(&self, stage: SessionStage) -> SessionLifetime {
        let pending = match stage {
            SessionStage::Complete => return self.lifetime,
            SessionStage::MfaChallenge => domain::mfa::CHALLENGE_LIFETIME,
            SessionStage::MfaEnrollment => domain::mfa::ENROLLMENT_LIFETIME,
        };
        SessionLifetime {
            idle: pending,
            max_age: pending,
        }
    }

    fn session_key(&self, hash: &str) -> String {
        format!("{}session:{hash}", self.prefix)
    }

    fn account_key(&self, account_id: Uuid) -> String {
        format!("{}account-sessions:{account_id}", self.prefix)
    }

    /// Opens a complete session for `account_id` in `tenant_id`.
    pub async fn create(&self, account_id: Uuid, tenant_id: Uuid) -> Result<NewSession, ValkeyError> {
        self.create_staged(account_id, tenant_id, SessionStage::Complete)
            .await
    }

    /// Opens a session at `stage`. A session waiting for two-step
    /// verification lasts `domain::mfa` lifetimes, not the configured ones.
    pub async fn create_staged(
        &self,
        account_id: Uuid,
        tenant_id: Uuid,
        stage: SessionStage,
    ) -> Result<NewSession, ValkeyError> {
        self.create_proven(account_id, tenant_id, stage, Proof::default())
            .await
    }

    /// Opens a session at `stage` that remembers what its sign-in `proof`.
    pub async fn create_proven(
        &self,
        account_id: Uuid,
        tenant_id: Uuid,
        stage: SessionStage,
        proof: Proof,
    ) -> Result<NewSession, ValkeyError> {
        let lifetime = self.lifetime_of(stage);
        let token = random_token();
        let now = OffsetDateTime::now_utc();
        let max_age = lifetime.max_age;
        let session = Session {
            account_id,
            tenant_id,
            csrf_token: random_token(),
            created_at: now,
            expires_at: now + max_age,
            stage,
            second_factor: proof.second_factor,
            sso: proof.sso,
            user_agent: None,
            last_seen_at: Some(now),
        };
        let hash = token_hash_hex(&token);
        let json = serde_json::to_string(&session)
            .map_err(|e| ValkeyError::new(fred::error::ErrorKind::Parse, e.to_string()))?;
        let _: () = self
            .valkey
            .set(
                self.session_key(&hash),
                json,
                Some(Expiration::EX(secs(lifetime.idle))),
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
        let remaining =
            std::time::Duration::try_from(session.expires_at - OffsetDateTime::now_utc()).unwrap_or_default();
        if remaining.is_zero() {
            self.forget(&hash, session.account_id).await?;
            return Ok(None);
        }
        let ttl = remaining.min(self.lifetime_of(session.stage).idle);
        let _: () = self.valkey.expire(&key, secs(ttl).max(1), None).await?;
        Ok(Some(session))
    }

    /// The session of `token`, like [`Self::get`], recording the request:
    /// its `User-Agent` and, at most every [`ACTIVITY_STEP`], the time.
    pub async fn touch(&self, token: &str, user_agent: Option<&str>) -> Result<Option<Session>, ValkeyError> {
        let hash = token_hash_hex(token);
        let key = self.session_key(&hash);
        let Some(mut session) = self.get(token).await? else {
            return Ok(None);
        };
        let now = OffsetDateTime::now_utc();
        let user_agent = user_agent.and_then(domain::devices::kept_user_agent);
        let step = ACTIVITY_STEP;
        let stale = session.last_seen_at.is_none_or(|seen| now - seen >= step);
        let moved = user_agent.is_some() && user_agent != session.user_agent;
        if !stale && !moved {
            return Ok(Some(session));
        }
        session.last_seen_at = Some(now);
        if user_agent.is_some() {
            session.user_agent = user_agent;
        }
        let remaining = std::time::Duration::try_from(session.expires_at - now).unwrap_or_default();
        let ttl = remaining.min(self.lifetime_of(session.stage).idle);
        let json = serde_json::to_string(&session)
            .map_err(|e| ValkeyError::new(fred::error::ErrorKind::Parse, e.to_string()))?;
        // `XX`: a session revoked in the meantime stays revoked.
        let _: Option<String> = self
            .valkey
            .set(
                &key,
                json,
                Some(Expiration::EX(secs(ttl).max(1))),
                Some(fred::types::SetOptions::XX),
                false,
            )
            .await?;
        Ok(Some(session))
    }

    /// Public id of the session of `token`, as [`Self::list`] gives it.
    pub fn id_of(token: &str) -> String {
        public_id(&token_hash_hex(token))
    }

    /// The open sessions of `account_id`, in every tenant, in no particular
    /// order. Forgets the expired ones the index still names.
    pub async fn list(&self, account_id: Uuid) -> Result<Vec<ListedSession>, ValkeyError> {
        let account_key = self.account_key(account_id);
        let hashes: Vec<String> = self.valkey.smembers(&account_key).await?;
        let mut sessions = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let json: Option<String> = self.valkey.get(self.session_key(&hash)).await?;
            match json.and_then(|json| serde_json::from_str::<Session>(&json).ok()) {
                Some(session) if session.expires_at > OffsetDateTime::now_utc() => {
                    sessions.push(ListedSession {
                        id: public_id(&hash),
                        session,
                    });
                }
                _ => {
                    let _: () = self.valkey.srem(&account_key, &hash).await?;
                }
            }
        }
        Ok(sessions)
    }

    /// Ends the session of `account_id` whose public id is `id`; `false`
    /// when there is none (another account's id included).
    pub async fn delete_by_id(&self, account_id: Uuid, id: &str) -> Result<bool, ValkeyError> {
        let hashes: Vec<String> = self.valkey.smembers(self.account_key(account_id)).await?;
        let Some(hash) = hashes.into_iter().find(|hash| public_id(hash) == id) else {
            return Ok(false);
        };
        let removed: i64 = self.valkey.del(self.session_key(&hash)).await?;
        let _: () = self.valkey.srem(self.account_key(account_id), &hash).await?;
        Ok(removed > 0)
    }

    /// Ends every session of `account_id`, in every tenant, but the one of
    /// `kept_token` (password change). Returns how many were open.
    pub async fn delete_others(&self, account_id: Uuid, kept_token: &str) -> Result<usize, ValkeyError> {
        let kept = token_hash_hex(kept_token);
        let hashes: Vec<String> = self.valkey.smembers(self.account_key(account_id)).await?;
        let mut deleted = 0;
        for hash in hashes.iter().filter(|hash| **hash != kept) {
            let removed: i64 = self.valkey.del(self.session_key(hash)).await?;
            deleted += usize::try_from(removed).unwrap_or(0);
            let _: () = self.valkey.srem(self.account_key(account_id), hash).await?;
        }
        Ok(deleted)
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

    /// Ends the sessions of `account_id` in `tenant_id` only (removed from
    /// that tenant, or no longer allowed in its application). Returns how
    /// many were open.
    pub async fn delete_in_tenant(&self, account_id: Uuid, tenant_id: Uuid) -> Result<usize, ValkeyError> {
        let hashes: Vec<String> = self.valkey.smembers(self.account_key(account_id)).await?;
        let mut deleted = 0;
        for hash in &hashes {
            let json: Option<String> = self.valkey.get(self.session_key(hash)).await?;
            let session = json.and_then(|json| serde_json::from_str::<Session>(&json).ok());
            match session {
                Some(session) if session.tenant_id == tenant_id => {
                    self.forget(hash, account_id).await?;
                    deleted += 1;
                }
                Some(_) => {}
                // Expired: only the index still names it.
                None => {
                    let _: () = self.valkey.srem(self.account_key(account_id), hash).await?;
                }
            }
        }
        Ok(deleted)
    }

    async fn forget(&self, hash: &str, account_id: Uuid) -> Result<(), ValkeyError> {
        let _: () = self.valkey.del(self.session_key(hash)).await?;
        let _: () = self.valkey.srem(self.account_key(account_id), hash).await?;
        Ok(())
    }
}

/// Public id of the session stored under `hash`: 32 hexadecimal digits of
/// its SHA-256.
fn public_id(hash: &str) -> String {
    let mut id = token_hash_hex(hash);
    id.truncate(32);
    id
}

fn secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    /// A session as written by the versions before `time` (chrono, RFC 3339
    /// with microseconds), still in Valkey after an upgrade.
    const STORED_BEFORE: &str = r#"{
        "account_id": "01940000-0000-7000-8000-000000000001",
        "tenant_id": "01940000-0000-7000-8000-000000000002",
        "csrf_token": "csrf",
        "created_at": "2026-11-11T08:00:00.123456Z",
        "expires_at": "2026-11-18T08:00:00.123456Z",
        "stage": "complete",
        "second_factor": true,
        "sso": false,
        "user_agent": "Firefox",
        "last_seen_at": "2026-11-11T09:30:00Z"
    }"#;

    #[test]
    fn sessions_stored_by_earlier_versions_still_read() {
        let session: Session = serde_json::from_str(STORED_BEFORE).unwrap();
        assert_eq!(session.created_at, datetime!(2026-11-11 08:00:00.123456 UTC));
        assert_eq!(session.expires_at, datetime!(2026-11-18 08:00:00.123456 UTC));
        assert_eq!(session.last_seen_at, Some(datetime!(2026-11-11 09:30 UTC)));
    }

    #[test]
    fn sessions_without_activity_still_read() {
        let mut json: serde_json::Value = serde_json::from_str(STORED_BEFORE).unwrap();
        json.as_object_mut().unwrap().remove("last_seen_at");
        let session: Session = serde_json::from_value(json).unwrap();
        assert_eq!(session.last_seen_at, None);
    }

    #[test]
    fn sessions_are_stored_with_rfc_3339_times() {
        let session: Session = serde_json::from_str(STORED_BEFORE).unwrap();
        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["created_at"], "2026-11-11T08:00:00.123456Z");
        assert_eq!(json["last_seen_at"], "2026-11-11T09:30:00Z");
        assert_eq!(serde_json::from_value::<Session>(json).unwrap(), session);
    }
}
