//! The signed-in account's memberships across tenants (specification §3),
//! for the tenant switcher. Row-level security shows one tenant at a time:
//! `app_my_memberships()` is the only path across them, and answers for the
//! account the request layer set ([`crate::db::set_account`]) only.

use sqlx::postgres::PgConnection;
use uuid::Uuid;

pub use domain::account::{AccountType, TenantRole};

/// A tenant the signed-in account belongs to (active memberships of active
/// tenants), by tenant name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyMembership {
    pub tenant_id: Uuid,
    pub slug: String,
    pub name: String,
    pub account_type: AccountType,
    pub tenant_role: TenantRole,
}

pub async fn mine(conn: &mut PgConnection) -> Result<Vec<MyMembership>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT tenant_id AS "tenant_id!", slug AS "slug!", name AS "name!",
               account_type AS "account_type!", tenant_role AS "tenant_role!"
        FROM app_my_memberships()
        "#
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| MyMembership {
            tenant_id: row.tenant_id,
            slug: row.slug,
            name: row.name,
            // The database checks these values.
            account_type: AccountType::parse(&row.account_type).unwrap_or(AccountType::External),
            tenant_role: TenantRole::parse(&row.tenant_role).unwrap_or(TenantRole::Member),
        })
        .collect())
}
