//! Pieces of the permission engine (specification §14, L02-US07): where
//! grants come from, and the Valkey cache of effective permissions.
//!
//! - [`GrantSource`] tells which roles reach a member on a node. L03 brings
//!   the grants (`node_role_grant`, groups, inheritance through the closure
//!   table); until then [`NoGrants`] grants nothing.
//! - [`PermissionCache`] keeps each member's effective permissions, keyed by
//!   a per-tenant *epoch*. Any change that can alter permissions (a role,
//!   a group, a grant, the hierarchy) bumps the epoch once committed: every
//!   cached entry of the tenant is then ignored, and expires on its own.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use domain::permissions::PermissionSet;
use fred::prelude::KeysInterface;
use fred::types::Expiration;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::valkey::{Valkey, ValkeyError};

/// What a [`GrantSource`] answers.
pub type GrantFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, sqlx::Error>> + Send + 'a>>;

/// Where the grants of roles come from, in the tenant transaction `conn`.
pub trait GrantSource: Send + Sync + fmt::Debug {
    /// Roles that reach `account_id` on `node` (`None`: the tenant itself),
    /// directly, through a group, or inherited.
    fn roles_of<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        account_id: Uuid,
        node: Option<Uuid>,
    ) -> GrantFuture<'a, Vec<Uuid>>;

    /// How many grants use `role_id`.
    fn grants_of_role<'a>(&'a self, conn: &'a mut PgConnection, role_id: Uuid) -> GrantFuture<'a, u64>;
}

/// No grant at all: the source until L03 brings `node_role_grant`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoGrants;

impl GrantSource for NoGrants {
    fn roles_of<'a>(
        &'a self,
        _: &'a mut PgConnection,
        _: Uuid,
        _: Option<Uuid>,
    ) -> GrantFuture<'a, Vec<Uuid>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn grants_of_role<'a>(&'a self, _: &'a mut PgConnection, _: Uuid) -> GrantFuture<'a, u64> {
        Box::pin(async { Ok(0) })
    }
}

/// How long an entry stays cached when nothing changes.
pub const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// Effective permissions in Valkey, per tenant, epoch, member and node.
#[derive(Debug, Clone)]
pub struct PermissionCache {
    valkey: Valkey,
    prefix: String,
}

/// Key of the epoch of `tenant_id`.
pub fn epoch_key(prefix: &str, tenant_id: Uuid) -> String {
    format!("{prefix}permissions-epoch:{tenant_id}")
}

/// Key of the effective permissions of `account_id` on `node` in
/// `tenant_id`, at `epoch`.
pub fn entry_key(prefix: &str, tenant_id: Uuid, epoch: u64, account_id: Uuid, node: Option<Uuid>) -> String {
    let scope = node.map_or_else(|| "tenant".to_owned(), |n| n.to_string());
    format!("{prefix}permissions:{tenant_id}:{epoch}:{account_id}:{scope}")
}

impl PermissionCache {
    pub fn new(valkey: Valkey, prefix: impl Into<String>) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
        }
    }

    /// The current epoch of `tenant_id` (0 before any change).
    pub async fn epoch(&self, tenant_id: Uuid) -> Result<u64, ValkeyError> {
        let epoch: Option<u64> = self.valkey.get(epoch_key(&self.prefix, tenant_id)).await?;
        Ok(epoch.unwrap_or(0))
    }

    /// Forgets every cached permission of `tenant_id`. Call it once the change
    /// is committed: a reader must not cache the old state under the new epoch.
    pub async fn invalidate(&self, tenant_id: Uuid) -> Result<(), ValkeyError> {
        let _: u64 = self.valkey.incr(epoch_key(&self.prefix, tenant_id)).await?;
        Ok(())
    }

    pub async fn get(
        &self,
        tenant_id: Uuid,
        epoch: u64,
        account_id: Uuid,
        node: Option<Uuid>,
    ) -> Result<Option<PermissionSet>, ValkeyError> {
        let bits: Option<u32> = self
            .valkey
            .get(entry_key(&self.prefix, tenant_id, epoch, account_id, node))
            .await?;
        Ok(bits.map(PermissionSet::from_bits))
    }

    pub async fn put(
        &self,
        tenant_id: Uuid,
        epoch: u64,
        account_id: Uuid,
        node: Option<Uuid>,
        permissions: PermissionSet,
    ) -> Result<(), ValkeyError> {
        let seconds = i64::try_from(CACHE_TTL.as_secs()).unwrap_or(300);
        self.valkey
            .set(
                entry_key(&self.prefix, tenant_id, epoch, account_id, node),
                permissions.bits(),
                Some(Expiration::EX(seconds)),
                None,
                false,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_name_the_tenant_epoch_member_and_node() {
        let tenant = Uuid::from_u128(1);
        let account = Uuid::from_u128(2);
        let node = Uuid::from_u128(3);
        assert_eq!(
            epoch_key("mlbpp:", tenant),
            "mlbpp:permissions-epoch:00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(
            entry_key("mlbpp:", tenant, 7, account, None),
            "mlbpp:permissions:00000000-0000-0000-0000-000000000001:7:00000000-0000-0000-0000-000000000002:tenant"
        );
        let on_node = entry_key("mlbpp:", tenant, 7, account, Some(node));
        assert!(
            on_node.ends_with(":00000000-0000-0000-0000-000000000003"),
            "{on_node}"
        );
        // A new epoch is a new key: older entries are ignored.
        assert_ne!(
            entry_key("p:", tenant, 7, account, None),
            entry_key("p:", tenant, 8, account, None)
        );
    }
}
