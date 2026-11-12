//! SQL of the administration of a tenant's members (L02-US06): list and
//! search them, change their access, remove them. Row-level security scopes
//! every query to the current tenant; accounts are visible through their
//! membership only.

use chrono::{DateTime, Utc};
use domain::members::Access;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::invitations::like_pattern;
use crate::pagination::{Keyset, Page, page_from_rows};

pub use domain::account::{AccountType, TenantRole};

/// A member of the tenant, as its admins see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantMember {
    pub account_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub account_type: AccountType,
    pub tenant_role: TenantRole,
    /// Signs in with a password even when the tenant requires SSO.
    pub local_account: bool,
    /// Not suspended.
    pub active: bool,
    /// Two-step verification is on (global to the account).
    pub mfa_enabled: bool,
    /// Version of the membership.
    pub version: i64,
    membership_id: Uuid,
    created_at: DateTime<Utc>,
}

impl TenantMember {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.membership_id,
        }
    }

    pub fn access(&self) -> Access {
        Access {
            account_type: self.account_type,
            tenant_role: self.tenant_role,
        }
    }
}

struct Row {
    account_id: Uuid,
    email: String,
    display_name: String,
    account_type: String,
    tenant_role: String,
    local_account: bool,
    status: String,
    mfa_enabled: bool,
    version: i64,
    membership_id: Uuid,
    created_at: DateTime<Utc>,
}

impl From<Row> for TenantMember {
    fn from(row: Row) -> Self {
        Self {
            account_id: row.account_id,
            email: row.email,
            display_name: row.display_name,
            // The database checks these values.
            account_type: AccountType::parse(&row.account_type).unwrap_or(AccountType::External),
            tenant_role: TenantRole::parse(&row.tenant_role).unwrap_or(TenantRole::Member),
            local_account: row.local_account,
            active: row.status == "active",
            mfa_enabled: row.mfa_enabled,
            version: row.version,
            membership_id: row.membership_id,
            created_at: row.created_at,
        }
    }
}

/// Filter of the members list.
#[derive(Debug, Clone, Default)]
pub struct MemberFilter<'a> {
    /// Part of the name or email, any case.
    pub search: Option<&'a str>,
    /// Only this account type; all when `None`.
    pub account_type: Option<AccountType>,
}

/// One page of members, in the order they joined.
pub async fn list(
    conn: &mut PgConnection,
    filter: &MemberFilter<'_>,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<TenantMember>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let pattern = filter.search.map(like_pattern);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT a.id AS account_id, a.email, a.display_name, m.account_type, m.tenant_role,
               m.local_account, m.status, m.version, m.id AS membership_id, m.created_at,
               EXISTS (SELECT 1 FROM account_mfa f WHERE f.account_id = a.id AND f.enabled_at IS NOT NULL)
                   AS "mfa_enabled!"
        FROM membership m
        JOIN account a ON a.id = m.account_id
        WHERE ($1::text IS NULL OR a.email LIKE $1 OR lower(a.display_name) LIKE $1)
          AND ($2::text IS NULL OR m.account_type = $2)
          AND ($3::timestamptz IS NULL OR (m.created_at, m.id) > ($3, $4::uuid))
        ORDER BY m.created_at, m.id
        LIMIT $5
        "#,
        pattern,
        filter.account_type.map(AccountType::as_str),
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let items = rows.into_iter().map(TenantMember::from).collect();
    Ok(page_from_rows(items, limit, TenantMember::keyset))
}

pub async fn find(conn: &mut PgConnection, account_id: Uuid) -> Result<Option<TenantMember>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT a.id AS account_id, a.email, a.display_name, m.account_type, m.tenant_role,
               m.local_account, m.status, m.version, m.id AS membership_id, m.created_at,
               EXISTS (SELECT 1 FROM account_mfa f WHERE f.account_id = a.id AND f.enabled_at IS NOT NULL)
                   AS "mfa_enabled!"
        FROM membership m
        JOIN account a ON a.id = m.account_id
        WHERE m.account_id = $1
        "#,
        account_id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(TenantMember::from))
}

/// Active tenant admins other than `account_id`. Locks every admin
/// membership, in a fixed order, until the end of the transaction: two
/// admins demoting each other at the same time cannot leave none.
pub async fn other_active_admins(conn: &mut PgConnection, account_id: Uuid) -> Result<u64, sqlx::Error> {
    let admins = sqlx::query_scalar!(
        r#"
        SELECT account_id FROM membership
        WHERE tenant_role = 'tenant_admin' AND status = 'active' AND account_type = 'internal'
        ORDER BY id
        FOR UPDATE
        "#
    )
    .fetch_all(conn)
    .await?;
    let others = admins.iter().filter(|id| **id != account_id).count();
    Ok(u64::try_from(others).unwrap_or(u64::MAX))
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// Sets the member's account type, tenant role and local account flag, if
/// the membership's version is one of `expected_versions` (`None`: any).
pub async fn update_access(
    conn: &mut PgConnection,
    account_id: Uuid,
    expected_versions: Option<&[i64]>,
    access: Access,
    local_account: bool,
) -> WriteResult<()> {
    let updated = sqlx::query!(
        r#"
        UPDATE membership
        SET account_type = $3, tenant_role = $4, local_account = $5, version = version + 1
        WHERE account_id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        "#,
        account_id,
        expected_versions,
        access.account_type.as_str(),
        access.tenant_role.as_str(),
        local_account
    )
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(missing_or_stale(conn, account_id).await?))
    }
}

/// Removes the member from the tenant, with their group memberships. Their
/// global account stays (other tenants, erasure: L02-US09).
pub async fn remove(
    conn: &mut PgConnection,
    account_id: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let removed = sqlx::query!(
        "DELETE FROM membership WHERE account_id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        account_id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if removed.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(missing_or_stale(conn, account_id).await?))
    }
}

async fn missing_or_stale(conn: &mut PgConnection, account_id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM membership WHERE account_id = $1) AS "exists!""#,
        account_id
    )
    .fetch_one(conn)
    .await?;
    Ok(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    })
}
