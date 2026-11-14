//! The roles, groups and members of the tenant that transition rules name
//! (L04-US05), by id: to check them when an admin sets the rules, and to
//! show their names. Row-level security keeps them to the current tenant;
//! one deleted since is simply not found.

use std::collections::BTreeMap;

use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

type Labels = BTreeMap<String, String>;

/// The roles among `ids`, with their labels.
pub async fn roles(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<(Uuid, Labels)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, label AS "label: Json<Labels>" FROM role WHERE id = ANY($1) ORDER BY created_at, id"#,
        ids
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.label.0)).collect())
}

/// The groups among `ids`, with their names.
pub async fn groups(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, name FROM "group" WHERE id = ANY($1) ORDER BY lower(name), id"#,
        ids
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.name)).collect())
}

/// The active members among the accounts `ids`, with their names.
pub async fn members(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT a.id, a.display_name
        FROM membership m JOIN account a ON a.id = m.account_id
        WHERE m.status = 'active' AND a.id = ANY($1)
        ORDER BY lower(a.display_name), a.id
        "#,
        ids
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.display_name)).collect())
}
