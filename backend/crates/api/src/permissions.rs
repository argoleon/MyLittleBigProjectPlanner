//! The permission check of handlers (specification §14, L02-US07).
//!
//! ```ignore
//! async fn comment(State(state): State<AppState>, mut tx: TenantTx, …) -> Result<…, Problem> {
//!     permissions::require(&state, &mut tx, Permission::Comment, Some(node_id)).await?;
//!     …
//! }
//! ```
//!
//! A member's effective permissions on a node are the union of the roles
//! that reach them there ([`infra::permissions::GrantSource`]: the grants of
//! `node_role_grant`, inherited through the hierarchy). They are cached in
//! Valkey per tenant epoch: any change that can alter them calls [`changed`]
//! once committed, and applies to the next request.

use domain::permissions::{Permission, PermissionChange, PermissionSet};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::problem::{ErrorCode, Problem};
use crate::tenant::{AppState, TenantTx};

/// A permission of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = Permission)]
pub enum PermissionView {
    View,
    CreateTicket,
    Edit,
    Comment,
    LogTime,
    AssignResources,
    ForceOverload,
    ManageSprints,
    ManageMembers,
    AdministerNode,
}

impl From<PermissionView> for Permission {
    fn from(view: PermissionView) -> Self {
        match view {
            PermissionView::View => Self::View,
            PermissionView::CreateTicket => Self::CreateTicket,
            PermissionView::Edit => Self::Edit,
            PermissionView::Comment => Self::Comment,
            PermissionView::LogTime => Self::LogTime,
            PermissionView::AssignResources => Self::AssignResources,
            PermissionView::ForceOverload => Self::ForceOverload,
            PermissionView::ManageSprints => Self::ManageSprints,
            PermissionView::ManageMembers => Self::ManageMembers,
            PermissionView::AdministerNode => Self::AdministerNode,
        }
    }
}

impl From<Permission> for PermissionView {
    fn from(permission: Permission) -> Self {
        match permission {
            Permission::View => Self::View,
            Permission::CreateTicket => Self::CreateTicket,
            Permission::Edit => Self::Edit,
            Permission::Comment => Self::Comment,
            Permission::LogTime => Self::LogTime,
            Permission::AssignResources => Self::AssignResources,
            Permission::ForceOverload => Self::ForceOverload,
            Permission::ManageSprints => Self::ManageSprints,
            Permission::ManageMembers => Self::ManageMembers,
            Permission::AdministerNode => Self::AdministerNode,
        }
    }
}

/// The views of `set`, in catalog order.
pub fn views(set: PermissionSet) -> Vec<PermissionView> {
    set.iter().map(PermissionView::from).collect()
}

/// The set of `views`.
pub fn set_of(views: &[PermissionView]) -> PermissionSet {
    views.iter().map(|v| Permission::from(*v)).collect()
}

/// The signed-in member's effective permissions on `node` (`None`: the
/// tenant itself), from the cache or computed and cached.
pub async fn effective(
    state: &AppState,
    tx: &mut TenantTx,
    node: Option<Uuid>,
) -> Result<PermissionSet, Problem> {
    let cache = &state.auth.permissions;
    let (tenant, account) = (tx.tenant_id(), tx.account_id());
    // Read before computing: an invalidation in between yields a new epoch,
    // and what is computed now is cached under the old one, unused.
    let epoch = cache.epoch(tenant).await?;
    if let Some(cached) = cache.get(tenant, epoch, account, node).await? {
        return Ok(cached);
    }
    let roles = state.grants.roles_of(tx, account, node).await?;
    let permissions = infra::roles::permissions_of(tx, &roles).await?;
    cache.put(tenant, epoch, account, node, permissions).await?;
    Ok(permissions)
}

/// 403 `permission.denied` unless the signed-in member has `permission` on
/// `node`.
pub async fn require(
    state: &AppState,
    tx: &mut TenantTx,
    permission: Permission,
    node: Option<Uuid>,
) -> Result<(), Problem> {
    let permissions = effective(state, tx, node).await?;
    check(permissions, permission)
}

/// 403 `permission.denied` unless `permissions` has `permission`; the
/// missing permission in `detail`.
pub fn check(permissions: PermissionSet, permission: Permission) -> Result<(), Problem> {
    if permissions.contains(permission) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::PermissionDenied)
            .with_detail(format!("missing permission: {}", permission.as_str())))
    }
}

/// Tells the engine that `change` is committed in the tenant: the cached
/// permissions are forgotten if it can alter them
/// ([`PermissionChange::alters_permissions`]), and apply to the next request.
/// A cache failure is logged, not reported: the change itself is done, and
/// entries expire on their own.
pub async fn changed(state: &AppState, tenant_id: Uuid, change: PermissionChange) {
    if !change.alters_permissions() {
        return;
    }
    if let Err(err) = state.auth.permissions.invalidate(tenant_id).await {
        tracing::warn!(error = %err, ?change, "cannot invalidate the permission cache");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_permission_is_a_stable_403() {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        assert!(check(reader, Permission::View).is_ok());
        let problem = check(reader, Permission::Comment).unwrap_err();
        assert_eq!(problem.code, ErrorCode::PermissionDenied);
        assert_eq!(problem.code.status().as_u16(), 403);
        assert_eq!(problem.code.as_str(), "permission.denied");
        assert_eq!(problem.detail.as_deref(), Some("missing permission: comment"));
    }

    #[test]
    fn views_follow_the_catalog() {
        for permission in Permission::ALL {
            assert_eq!(Permission::from(PermissionView::from(permission)), permission);
        }
        let all = PermissionSet::all();
        assert_eq!(set_of(&views(all)), all);
        let json = serde_json::to_value(views(all)).unwrap();
        let ids: Vec<&str> = Permission::ALL.iter().map(|p| p.as_str()).collect();
        assert_eq!(json, serde_json::json!(ids));
    }
}
