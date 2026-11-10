//! Accounts and memberships (specification §3), always in a tenant
//! transaction: row-level security shows the accounts of the current
//! tenant's members only.

use chrono::{DateTime, Utc};
pub use domain::account::{AccountType, TenantRole};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::id::new_id;

/// A member of the current tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub account_id: Uuid,
    pub email: String,
    pub display_name: String,
    /// Preferred language; the tenant's default when `None`.
    pub locale: Option<String>,
    pub account_type: AccountType,
    pub tenant_role: TenantRole,
    /// A suspended member cannot sign in.
    pub active: bool,
    /// Outside the organization's directory: signs in with a password even
    /// when the tenant requires SSO.
    pub local_account: bool,
}

/// A member with its password hash, for sign-in.
#[derive(Debug, Clone)]
pub struct MemberWithPassword {
    pub member: Member,
    pub password_hash: Option<String>,
}

struct Row {
    id: Uuid,
    email: String,
    display_name: String,
    locale: Option<String>,
    password_hash: Option<String>,
    account_type: String,
    tenant_role: String,
    status: String,
    local_account: bool,
}

impl Row {
    fn into_member(self) -> MemberWithPassword {
        MemberWithPassword {
            member: Member {
                account_id: self.id,
                email: self.email,
                display_name: self.display_name,
                locale: self.locale,
                // The database checks these values.
                account_type: AccountType::parse(&self.account_type).unwrap_or(AccountType::External),
                tenant_role: TenantRole::parse(&self.tenant_role).unwrap_or(TenantRole::Member),
                active: self.status == "active",
                local_account: self.local_account,
            },
            password_hash: self.password_hash,
        }
    }
}

/// The member of the current tenant with this (normalized) email.
pub async fn find_by_email(
    conn: &mut PgConnection,
    email: &str,
) -> Result<Option<MemberWithPassword>, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT a.id, a.email, a.display_name, a.locale, a.password_hash,
               m.account_type, m.tenant_role, m.status, m.local_account
        FROM account a
        JOIN membership m ON m.account_id = a.id
        WHERE a.email = $1
        "#,
        email
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(Row::into_member))
}

/// The member of the current tenant holding `account_id`.
pub async fn find(conn: &mut PgConnection, account_id: Uuid) -> Result<Option<Member>, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT a.id, a.email, a.display_name, a.locale, a.password_hash,
               m.account_type, m.tenant_role, m.status, m.local_account
        FROM account a
        JOIN membership m ON m.account_id = a.id
        WHERE a.id = $1
        "#,
        account_id
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|row| row.into_member().member))
}

/// Replaces the password of a member of the current tenant.
pub async fn set_password(
    conn: &mut PgConnection,
    account_id: Uuid,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE account
        SET password_hash = $2, password_changed_at = now(), version = version + 1
        WHERE id = $1
        "#,
        account_id,
        password_hash
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A member to add to the current tenant.
#[derive(Debug, Clone)]
pub struct NewMember<'a> {
    pub email: &'a str,
    pub display_name: &'a str,
    /// Set on a new account only: an existing account keeps its password.
    pub password_hash: Option<&'a str>,
    pub account_type: AccountType,
    pub tenant_role: TenantRole,
    /// See [`Member::local_account`].
    pub local_account: bool,
}

/// What [`add_member`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddedMember {
    pub account_id: Uuid,
    /// The account did not exist in any tenant.
    pub new_account: bool,
}

/// Adds a member to the current tenant: links the existing account of this
/// email, or creates it. An existing membership is updated (type, role) and
/// reactivated.
pub async fn add_member(conn: &mut PgConnection, new: &NewMember<'_>) -> Result<AddedMember, sqlx::Error> {
    let existing = sqlx::query_scalar!(r#"SELECT app_account_id_by_email($1) AS "id""#, new.email)
        .fetch_one(&mut *conn)
        .await?;
    let (account_id, new_account) = match existing {
        Some(id) => (id, false),
        None => {
            let id = new_id();
            // No RETURNING: the new account is not visible before its membership.
            sqlx::query!(
                r#"
                INSERT INTO account (id, email, display_name, password_hash, password_changed_at)
                VALUES ($1, $2, $3, $4, CASE WHEN $4::text IS NULL THEN NULL ELSE now() END)
                "#,
                id,
                new.email,
                new.display_name,
                new.password_hash
            )
            .execute(&mut *conn)
            .await?;
            (id, true)
        }
    };
    sqlx::query!(
        r#"
        INSERT INTO membership (id, tenant_id, account_id, account_type, tenant_role, local_account)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5)
        ON CONFLICT (tenant_id, account_id) DO UPDATE
            SET account_type = EXCLUDED.account_type,
                tenant_role = EXCLUDED.tenant_role,
                local_account = EXCLUDED.local_account,
                status = 'active',
                version = membership.version + 1
        "#,
        new_id(),
        account_id,
        new.account_type.as_str(),
        new.tenant_role.as_str(),
        new.local_account
    )
    .execute(conn)
    .await?;
    Ok(AddedMember {
        account_id,
        new_account,
    })
}

/// Records a password reset of `account_id`, valid until `expires_at`.
pub async fn create_password_reset(
    conn: &mut PgConnection,
    account_id: Uuid,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO password_reset (id, tenant_id, account_id, token_hash, expires_at)
        VALUES ($1, app_current_tenant(), $2, $3, $4)
        "#,
        new_id(),
        account_id,
        token_hash,
        expires_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Uses the password reset of `token_hash`: the account it was for, or
/// `None` if it is unknown in this tenant, expired or already used.
pub async fn consume_password_reset(
    conn: &mut PgConnection,
    token_hash: &[u8],
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        UPDATE password_reset
        SET used_at = now(), version = version + 1
        WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()
        RETURNING account_id
        "#,
        token_hash
    )
    .fetch_optional(conn)
    .await
}

/// Ends the unused password resets of `account_id` (its password just
/// changed: older links must not work any more).
pub async fn revoke_password_resets(conn: &mut PgConnection, account_id: Uuid) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE password_reset
        SET used_at = now(), version = version + 1
        WHERE account_id = $1 AND used_at IS NULL
        "#,
        account_id
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}
