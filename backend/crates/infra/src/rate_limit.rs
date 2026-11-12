//! Rate limits counted in Valkey, shared by every API instance
//! (specification §14). Fixed windows: a counter that expires.

use std::time::Duration;

use fred::prelude::KeysInterface;
use fred::types::ExpireOptions;

use crate::valkey::{Valkey, ValkeyError};

/// Counters in Valkey, under keys starting with `prefix`.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    valkey: Valkey,
    prefix: String,
}

impl RateLimiter {
    pub fn new(valkey: Valkey, prefix: impl Into<String>) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
        }
    }

    fn key(&self, name: &str) -> String {
        format!("{}rate:{name}", self.prefix)
    }

    /// Current count of `name` in its window.
    pub async fn count(&self, name: &str) -> Result<u32, ValkeyError> {
        let count: Option<u32> = self.valkey.get(self.key(name)).await?;
        Ok(count.unwrap_or(0))
    }

    /// Adds one to `name`; the window of `window` starts with the first hit.
    /// Returns the new count.
    pub async fn hit(&self, name: &str, window: Duration) -> Result<u32, ValkeyError> {
        let key = self.key(name);
        let count: u32 = self.valkey.incr(&key).await?;
        let seconds = i64::try_from(window.as_secs()).unwrap_or(i64::MAX).max(1);
        // NX: only the first hit sets the expiry, so the window is fixed.
        let _: () = self.valkey.expire(&key, seconds, Some(ExpireOptions::NX)).await?;
        Ok(count)
    }

    /// Time left before the window of `name` ends.
    pub async fn retry_after(&self, name: &str) -> Result<Duration, ValkeyError> {
        let ttl: i64 = self.valkey.ttl(self.key(name)).await?;
        Ok(Duration::from_secs(u64::try_from(ttl).unwrap_or(0)))
    }

    /// Forgets `name` (a successful sign-in clears its failures).
    pub async fn clear(&self, name: &str) -> Result<(), ValkeyError> {
        let _: () = self.valkey.del(self.key(name)).await?;
        Ok(())
    }
}
