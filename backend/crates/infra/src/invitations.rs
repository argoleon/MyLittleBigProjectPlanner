//! SQL of the invitations of new members (L02-US06). Row-level security
//! scopes every query to the current tenant; only the SHA-256 of a token is
//! stored.

use chrono::{DateTime, Utc};
use domain::members::{Access, InvitationStatus};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::pagination::{Keyset, Page, page_from_rows};

pub use domain::account::{AccountType, TenantRole};

/// Name of the unique index of open invitations per email.
const PENDING_UNIQUE: &str = "invitation_tenant_pending_email_idx";

/// An invitation as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    pub id: Uuid,
    pub email: String,
    pub account_type: AccountType,
    pub tenant_role: TenantRole,
    /// As stored: `pending`, `accepted` or `revoked` ([`Invitation::status`]
    /// tells expired ones apart).
    pub stored_status: String,
    pub expires_at: DateTime<Utc>,
    pub invited_by_name: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl Invitation {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }

    pub fn status(&self, now: DateTime<Utc>) -> InvitationStatus {
        InvitationStatus::at(&self.stored_status, self.expires_at.timestamp(), now.timestamp())
    }

    pub fn access(&self) -> Access {
        Access {
            account_type: self.account_type,
            tenant_role: self.tenant_role,
        }
    }
}

struct Row {
    id: Uuid,
    email: String,
    account_type: String,
    tenant_role: String,
    status: String,
    expires_at: DateTime<Utc>,
    invited_by_name: Option<String>,
    version: i64,
    created_at: DateTime<Utc>,
}

impl From<Row> for Invitation {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            email: row.email,
            // The database checks these values.
            account_type: AccountType::parse(&row.account_type).unwrap_or(AccountType::External),
            tenant_role: TenantRole::parse(&row.tenant_role).unwrap_or(TenantRole::Member),
            stored_status: row.status,
            expires_at: row.expires_at,
            invited_by_name: row.invited_by_name,
            version: row.version,
            created_at: row.created_at,
        }
    }
}

/// A new invitation.
#[derive(Debug, Clone, Copy)]
pub struct NewInvitation<'a> {
    pub id: Uuid,
    /// Normalized.
    pub email: &'a str,
    pub access: Access,
    pub token_hash: &'a [u8],
    pub expires_at: DateTime<Utc>,
    pub invited_by: Uuid,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// An open invitation exists for this email (send it again instead).
    AlreadyInvited,
    /// The invitation is accepted or revoked: it cannot change any more.
    Closed,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

pub async fn insert(conn: &mut PgConnection, new: &NewInvitation<'_>) -> WriteResult<Invitation> {
    let inserted = sqlx::query!(
        r#"
        INSERT INTO invitation (id, tenant_id, email, account_type, tenant_role, token_hash, expires_at, invited_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7)
        "#,
        new.id,
        new.email,
        new.access.account_type.as_str(),
        new.access.tenant_role.as_str(),
        new.token_hash,
        new.expires_at,
        new.invited_by
    )
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => Ok(find(conn, new.id).await?.ok_or(WriteConflict::NotFound)),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(PENDING_UNIQUE) => {
            Ok(Err(WriteConflict::AlreadyInvited))
        }
        Err(err) => Err(err),
    }
}

pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Invitation>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.email, i.account_type, i.tenant_role, i.status, i.expires_at,
               a.display_name AS "invited_by_name?", i.version, i.created_at
        FROM invitation i
        LEFT JOIN account a ON a.id = i.invited_by
        WHERE i.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Invitation::from))
}

/// Filter of the open invitations list.
#[derive(Debug, Clone, Default)]
pub struct InvitationFilter<'a> {
    /// Part of the email, any case.
    pub search: Option<&'a str>,
    /// Only these account types; all when `None`.
    pub account_type: Option<AccountType>,
}

/// One page of open (`pending`, expired included) invitations, in creation order.
pub async fn list_open(
    conn: &mut PgConnection,
    filter: &InvitationFilter<'_>,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<Invitation>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let pattern = filter.search.map(like_pattern);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.email, i.account_type, i.tenant_role, i.status, i.expires_at,
               a.display_name AS "invited_by_name?", i.version, i.created_at
        FROM invitation i
        LEFT JOIN account a ON a.id = i.invited_by
        WHERE i.status = 'pending'
          AND ($1::text IS NULL OR i.email LIKE $1)
          AND ($2::text IS NULL OR i.account_type = $2)
          AND ($3::timestamptz IS NULL OR (i.created_at, i.id) > ($3, $4::uuid))
        ORDER BY i.created_at, i.id
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
    let items = rows.into_iter().map(Invitation::from).collect();
    Ok(page_from_rows(items, limit, Invitation::keyset))
}

/// `%text%` for `LIKE`, `%`, `_` and `\` in `text` matched literally, lower case.
pub fn like_pattern(text: &str) -> String {
    let mut pattern = String::from("%");
    for c in text.trim().to_lowercase().chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// A new token and deadline for an open invitation (sending it again).
pub async fn renew(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> WriteResult<Invitation> {
    let renewed = sqlx::query_scalar!(
        r#"
        UPDATE invitation SET token_hash = $3, expires_at = $4, version = version + 1
        WHERE id = $1 AND status = 'pending' AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        token_hash,
        expires_at
    )
    .fetch_optional(&mut *conn)
    .await?;
    match renewed {
        Some(_) => Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound)),
        None => Ok(Err(why_not(conn, id).await?)),
    }
}

/// Revokes an open invitation: its link stops working.
pub async fn revoke(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let revoked = sqlx::query!(
        r#"
        UPDATE invitation SET status = 'revoked', version = version + 1
        WHERE id = $1 AND status = 'pending' AND ($2::bigint[] IS NULL OR version = ANY($2))
        "#,
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if revoked.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(why_not(conn, id).await?))
    }
}

async fn why_not(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let status = sqlx::query_scalar!("SELECT status FROM invitation WHERE id = $1", id)
        .fetch_optional(conn)
        .await?;
    Ok(match status.as_deref() {
        None => WriteConflict::NotFound,
        Some("pending") => WriteConflict::VersionMismatch,
        Some(_) => WriteConflict::Closed,
    })
}

/// The open, unexpired invitation of `token_hash`, if any.
pub async fn find_by_token(
    conn: &mut PgConnection,
    token_hash: &[u8],
) -> Result<Option<Invitation>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.email, i.account_type, i.tenant_role, i.status, i.expires_at,
               a.display_name AS "invited_by_name?", i.version, i.created_at
        FROM invitation i
        LEFT JOIN account a ON a.id = i.invited_by
        WHERE i.token_hash = $1 AND i.status = 'pending' AND i.expires_at > now()
        "#,
        token_hash
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Invitation::from))
}

/// Marks the invitation accepted, once: `false` if it no longer was open.
pub async fn mark_accepted(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE invitation SET status = 'accepted', accepted_at = now(), version = version + 1
        WHERE id = $1 AND status = 'pending' AND expires_at > now()
        "#,
        id
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Revokes the open invitations of `email` (it became a member).
pub async fn close_for_email(conn: &mut PgConnection, email: &str) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE invitation SET status = 'revoked', version = version + 1
        WHERE email = $1 AND status = 'pending'
        "#,
        email
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns_match_wildcards_literally() {
        assert_eq!(like_pattern(" Durand "), "%durand%");
        assert_eq!(like_pattern("50%_a\\b"), "%50\\%\\_a\\\\b%");
    }
}
