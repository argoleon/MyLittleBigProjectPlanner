//! Watchers of tickets (`issue_watcher`, L05-US07): those the rules naming
//! "watchers" notify. A ticket's reporter, its assignee and those who
//! comment on it start watching it; anyone who sees it may watch or stop.

use sqlx::postgres::PgConnection;
use uuid::Uuid;

/// `account` watches `issue`; nothing if it does already.
pub async fn watch(conn: &mut PgConnection, issue: Uuid, account: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO issue_watcher (tenant_id, issue_id, account_id)
        VALUES (app_current_tenant(), $1, $2)
        ON CONFLICT DO NOTHING
        "#,
        issue,
        account
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// `account` stops watching `issue`.
pub async fn unwatch(conn: &mut PgConnection, issue: Uuid, account: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM issue_watcher WHERE issue_id = $1 AND account_id = $2",
        issue,
        account
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// The watchers of `issue`.
pub async fn of_issue(conn: &mut PgConnection, issue: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT account_id FROM issue_watcher WHERE issue_id = $1 ORDER BY created_at, account_id",
        issue
    )
    .fetch_all(conn)
    .await
}

/// How many watch `issue`, and whether `account` does.
pub async fn summary(
    conn: &mut PgConnection,
    issue: Uuid,
    account: Uuid,
) -> Result<(i64, bool), sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT count(*) AS "count!", coalesce(bool_or(account_id = $2), false) AS "watching!"
        FROM issue_watcher WHERE issue_id = $1
        "#,
        issue,
        account
    )
    .fetch_one(conn)
    .await?;
    Ok((row.count, row.watching))
}
