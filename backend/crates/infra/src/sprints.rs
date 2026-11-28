//! SQL of sprints, their scope and the rank of a board's tickets (L09-US01).
//! Row-level security scopes every query to the current tenant; what a
//! member sees and may change is decided by the caller.
//!
//! - A ticket's stay in a sprint is a `sprint_issue` row; leaving an open
//!   sprint sets `removed_at`, so the scope's history stays for the
//!   burndown and the velocity. A ticket is in one open sprint at most.
//! - Ranks are lexicographic (`domain::rank`): a move writes one row. The
//!   writes of a board are serialized by locking its row ([`lock_board`]).

use std::collections::HashMap;

use domain::rank::{self, RankError};
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// A sprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sprint {
    pub id: Uuid,
    pub board_id: Uuid,
    pub board_name: String,
    pub sequence: i32,
    pub name: String,
    pub goal: Option<String>,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
    /// `future`, `active` or `closed`.
    pub state: String,
    pub version: i64,
    pub created_at: OffsetDateTime,
}

/// A sprint as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SprintInput<'a> {
    pub name: &'a str,
    pub goal: Option<&'a str>,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    board_id: Uuid,
    board_name: String,
    sequence: i32,
    name: String,
    goal: Option<String>,
    start_date: Option<Date>,
    end_date: Option<Date>,
    state: String,
    version: i64,
    created_at: OffsetDateTime,
}

impl From<Row> for Sprint {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            board_id: row.board_id,
            board_name: row.board_name,
            sequence: row.sequence,
            name: row.name,
            goal: row.goal,
            start_date: row.start_date,
            end_date: row.end_date,
            state: row.state,
            version: row.version,
            created_at: row.created_at,
        }
    }
}

/// The sprints `ids`, or the open ones of `board`: the active ones first,
/// then by their number on their board.
async fn select(
    conn: &mut PgConnection,
    ids: Option<&[Uuid]>,
    open_of_board: Option<Uuid>,
) -> Result<Vec<Sprint>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT s.id, s.board_id, b.name AS board_name, s.sequence, s.name, s.goal,
               s.start_date, s.end_date, s.state, s.version, s.created_at
        FROM sprint s JOIN board b ON b.id = s.board_id
        WHERE ($1::uuid[] IS NULL OR s.id = ANY($1))
          AND ($2::uuid IS NULL OR (s.board_id = $2 AND s.state <> 'closed'))
        ORDER BY (s.state = 'active') DESC, lower(b.name), s.board_id, s.sequence
        "#,
        ids,
        open_of_board
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(Sprint::from).collect())
}

/// The open sprints of `board`: active first, then by number.
pub async fn open_of_board(conn: &mut PgConnection, board: Uuid) -> Result<Vec<Sprint>, sqlx::Error> {
    select(conn, None, Some(board)).await
}

/// The sprints `ids`, in the same order.
pub async fn find_many(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<Sprint>, sqlx::Error> {
    select(conn, Some(ids), None).await
}

/// The sprint `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Sprint>, sqlx::Error> {
    Ok(select(conn, Some(&[id]), None).await?.pop())
}

/// Locks the board `id` until the end of the transaction: its sprints and
/// ranks are written one change at a time.
pub async fn lock_board(conn: &mut PgConnection, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT id FROM board WHERE id = $1 FOR NO KEY UPDATE", id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(())
}

/// The number the next sprint of `board` takes (lock the board first).
pub async fn next_sequence(conn: &mut PgConnection, board: Uuid) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT COALESCE(max(sequence), 0) + 1 AS "next!" FROM sprint WHERE board_id = $1"#,
        board
    )
    .fetch_one(conn)
    .await
}

/// Creates a future sprint on `board`, numbered `sequence`.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    board: Uuid,
    sequence: i32,
    created_by: Uuid,
    input: &SprintInput<'_>,
) -> Result<Sprint, sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO sprint (id, tenant_id, board_id, sequence, name, goal, start_date, end_date, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8)
        "#,
        id,
        board,
        sequence,
        input.name.trim(),
        input.goal.map(str::trim).filter(|g| !g.is_empty()),
        input.start_date,
        input.end_date,
        created_by
    )
    .execute(&mut *conn)
    .await?;
    find(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Replaces the name, goal and dates of the sprint `id`, if its version is
/// one of `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &SprintInput<'_>,
) -> WriteResult<Sprint> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE sprint
        SET name = $3, goal = $4, start_date = $5, end_date = $6, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        input.name.trim(),
        input.goal.map(str::trim).filter(|g| !g.is_empty()),
        input.start_date,
        input.end_date
    )
    .fetch_optional(&mut *conn)
    .await?;
    if updated.is_none() {
        return Ok(Err(missing_or_stale(conn, id).await?));
    }
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the sprint `id`, if its version is expected: its tickets go back
/// to the backlog (a sprint never started has no history worth keeping).
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM sprint WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(missing_or_stale(conn, id).await?))
    }
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM sprint WHERE id = $1) AS "exists!""#,
        id
    )
    .fetch_one(conn)
    .await?;
    Ok(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    })
}

/// The open sprint each of `issues` is in, if any.
pub async fn open_sprint_of(
    conn: &mut PgConnection,
    issues: &[Uuid],
) -> Result<HashMap<Uuid, Uuid>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT issue_id, sprint_id FROM sprint_issue
        WHERE issue_id = ANY($1) AND removed_at IS NULL AND sprint_open
        "#,
        issues
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.issue_id, r.sprint_id)).collect())
}

/// Puts `issue` in the open sprint `sprint`, taking it out of the open sprint
/// it was in. Nothing when it is there already; whether it moved.
pub async fn put(
    conn: &mut PgConnection,
    issue: Uuid,
    sprint: Uuid,
    actor: Uuid,
) -> Result<bool, sqlx::Error> {
    // One open sprint per ticket, even when two boards plan it at once.
    sqlx::query!("SELECT id FROM issue WHERE id = $1 FOR NO KEY UPDATE", issue)
        .fetch_optional(&mut *conn)
        .await?;
    if open_sprint_of(conn, &[issue]).await?.get(&issue) == Some(&sprint) {
        return Ok(false);
    }
    take_out(conn, issue, actor).await?;
    sqlx::query!(
        r#"
        INSERT INTO sprint_issue (id, tenant_id, sprint_id, issue_id, added_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4)
        "#,
        crate::id::new_id(),
        sprint,
        issue,
        actor
    )
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Takes `issue` out of the open sprint it is in; whether it was in one.
pub async fn take_out(conn: &mut PgConnection, issue: Uuid, actor: Uuid) -> Result<bool, sqlx::Error> {
    let taken = sqlx::query!(
        r#"
        UPDATE sprint_issue SET removed_at = greatest(now(), added_at), removed_by = $2
        WHERE issue_id = $1 AND removed_at IS NULL AND sprint_open
        "#,
        issue,
        actor
    )
    .execute(&mut *conn)
    .await?;
    Ok(taken.rows_affected() > 0)
}

/// A stay of a ticket in a sprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stay {
    pub sprint_id: Uuid,
    pub issue_id: Uuid,
    pub added_at: OffsetDateTime,
    pub removed_at: Option<OffsetDateTime>,
}

/// The history of the scope of `sprint`: every stay, by arrival.
pub async fn history(conn: &mut PgConnection, sprint: Uuid) -> Result<Vec<Stay>, sqlx::Error> {
    sqlx::query_as!(
        Stay,
        r#"
        SELECT sprint_id, issue_id, added_at, removed_at FROM sprint_issue
        WHERE sprint_id = $1 ORDER BY added_at, id
        "#,
        sprint
    )
    .fetch_all(conn)
    .await
}

/// The ranks of `issues` on `board`, for those ranked.
pub async fn ranks_of(
    conn: &mut PgConnection,
    board: Uuid,
    issues: &[Uuid],
) -> Result<HashMap<Uuid, String>, sqlx::Error> {
    let rows = sqlx::query!(
        "SELECT issue_id, rank FROM board_rank WHERE board_id = $1 AND issue_id = ANY($2)",
        board,
        issues
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.issue_id, r.rank)).collect())
}

/// The highest rank on `board`.
async fn last_rank(conn: &mut PgConnection, board: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT rank FROM board_rank WHERE board_id = $1 ORDER BY rank DESC LIMIT 1",
        board
    )
    .fetch_optional(conn)
    .await
}

/// The rank on `board` right after `rank`, if any.
async fn next_rank(conn: &mut PgConnection, board: Uuid, after: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT rank FROM board_rank WHERE board_id = $1 AND rank > $2 ORDER BY rank LIMIT 1",
        board,
        after
    )
    .fetch_optional(conn)
    .await
}

/// The rank on `board` right before `rank`, if any.
async fn previous_rank(
    conn: &mut PgConnection,
    board: Uuid,
    before: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT rank FROM board_rank WHERE board_id = $1 AND rank < $2 ORDER BY rank DESC LIMIT 1",
        board,
        before
    )
    .fetch_optional(conn)
    .await
}

async fn write_rank(
    conn: &mut PgConnection,
    board: Uuid,
    issue: Uuid,
    rank: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO board_rank (tenant_id, board_id, issue_id, rank)
        VALUES (app_current_tenant(), $1, $2, $3)
        ON CONFLICT (tenant_id, board_id, issue_id) DO UPDATE SET rank = EXCLUDED.rank
        "#,
        board,
        issue,
        rank
    )
    .execute(conn)
    .await?;
    Ok(())
}

fn rank_error(error: RankError) -> sqlx::Error {
    sqlx::Error::Protocol(format!("stored ranks are inconsistent: {error:?}"))
}

/// Ranks `unranked`, in this order, after every ticket ranked on `board`:
/// the tickets a move passes over get the place they had in the filter's
/// order (lock the board first).
pub async fn rank_last(conn: &mut PgConnection, board: Uuid, unranked: &[Uuid]) -> Result<(), sqlx::Error> {
    if unranked.is_empty() {
        return Ok(());
    }
    let last = last_rank(conn, board).await?;
    let ranks = rank::spread(last.as_deref(), None, unranked.len()).map_err(rank_error)?;
    for (issue, rank) in unranked.iter().zip(ranks) {
        write_rank(conn, board, *issue, &rank).await?;
    }
    Ok(())
}

/// Where a ticket goes in a board's order: right after `after`, or right
/// before `before` when `after` is not given; last when neither is. Both
/// are ranked tickets of the board (rank them first with [`rank_last`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub after: Option<Uuid>,
    pub before: Option<Uuid>,
}

/// Ranks `issue` on `board` at `place` (lock the board first). The tickets
/// of other lists ranked between the two neighbours keep their own order.
/// Ranks grown too long are rebalanced. `Ok(false)` when a neighbour has no
/// rank on the board.
pub async fn rank_at(
    conn: &mut PgConnection,
    board: Uuid,
    issue: Uuid,
    place: Place,
) -> Result<bool, sqlx::Error> {
    let neighbours: Vec<Uuid> = place.after.into_iter().chain(place.before).collect();
    let known = ranks_of(conn, board, &neighbours).await?;
    let rank_of = |id: Option<Uuid>| id.map(|id| known.get(&id).cloned());
    let (low, high) = match (rank_of(place.after), rank_of(place.before)) {
        (Some(None), _) | (None, Some(None)) => return Ok(false),
        (Some(Some(after)), _) => {
            let mut next = next_rank(conn, board, &after).await?;
            // Its own rank is no bound: it leaves that place.
            if next.is_some() && next == ranks_of(conn, board, &[issue]).await?.remove(&issue) {
                next = next_rank(conn, board, next.as_deref().unwrap_or_default()).await?;
            }
            (Some(after), next)
        }
        (None, Some(Some(before))) => {
            let mut previous = previous_rank(conn, board, &before).await?;
            if previous.is_some() && previous == ranks_of(conn, board, &[issue]).await?.remove(&issue) {
                previous = previous_rank(conn, board, previous.as_deref().unwrap_or_default()).await?;
            }
            (previous, Some(before))
        }
        (None, None) => {
            let last = last_rank(conn, board).await?;
            if last.is_some() && last == ranks_of(conn, board, &[issue]).await?.remove(&issue) {
                return Ok(true);
            }
            (last, None)
        }
    };
    let rank = rank::between(low.as_deref(), high.as_deref()).map_err(rank_error)?;
    write_rank(conn, board, issue, &rank).await?;
    if rank.len() > rank::REBALANCE_LEN {
        rebalance(conn, board).await?;
    }
    Ok(true)
}

/// Gives every ranked ticket of `board` a short rank, in the same order.
pub async fn rebalance(conn: &mut PgConnection, board: Uuid) -> Result<(), sqlx::Error> {
    let issues: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT issue_id FROM board_rank WHERE board_id = $1 ORDER BY rank",
        board
    )
    .fetch_all(&mut *conn)
    .await?;
    let ranks = rank::spread(None, None, issues.len()).map_err(rank_error)?;
    // Out of the way first: the new ranks may take the old ones' values.
    sqlx::query!("DELETE FROM board_rank WHERE board_id = $1", board)
        .execute(&mut *conn)
        .await?;
    for (issue, rank) in issues.iter().zip(ranks) {
        write_rank(conn, board, *issue, &rank).await?;
    }
    Ok(())
}
