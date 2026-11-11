//! Switches between tenants (L02-US05). A session belongs to one tenant and
//! its cookie to that tenant's host: to switch, the current tenant issues a
//! one-time token that the browser brings to the target's host, which opens
//! a session there under its own policy.

use std::time::Duration;

use fred::prelude::KeysInterface;
use fred::types::Expiration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::credentials::token_hash_hex;
use crate::valkey::{Valkey, ValkeyError};

/// Time to bring the token to the target tenant.
pub const SWITCH_LIFETIME: Duration = Duration::from_secs(60);

/// A switch waiting for the browser at the target tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSwitch {
    pub account_id: Uuid,
    pub target_tenant_id: Uuid,
    /// What the session being left proved.
    pub second_factor: bool,
    pub sso: bool,
}

/// Pending switches, in Valkey under the hash of their token, taken once.
#[derive(Debug, Clone)]
pub struct SwitchStore {
    valkey: Valkey,
    prefix: String,
}

impl SwitchStore {
    pub fn new(valkey: Valkey, prefix: impl Into<String>) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
        }
    }

    fn entry(&self, token: &str) -> String {
        format!("{}tenant-switch:{}", self.prefix, token_hash_hex(token))
    }

    pub async fn save(&self, token: &str, switch: &PendingSwitch) -> Result<(), ValkeyError> {
        let json = serde_json::to_string(switch)
            .map_err(|e| ValkeyError::new(fred::error::ErrorKind::Parse, e.to_string()))?;
        let seconds = i64::try_from(SWITCH_LIFETIME.as_secs()).unwrap_or(60);
        self.valkey
            .set(
                self.entry(token),
                json,
                Some(Expiration::EX(seconds)),
                None,
                false,
            )
            .await
    }

    /// The switch of `token`, removed: a token works once.
    pub async fn take(&self, token: &str) -> Result<Option<PendingSwitch>, ValkeyError> {
        let json: Option<String> = self.valkey.getdel(self.entry(token)).await?;
        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }
}
