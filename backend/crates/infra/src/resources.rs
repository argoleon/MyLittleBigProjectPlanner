//! SQL of resources (L07-US01). Row-level security scopes every query to the
//! current tenant. A linked person shows its member's live name
//! (`app_member_name`); the stored `display_name` is a copy, the name it
//! keeps once the member leaves (migration `resources`).

use domain::account::AccountType;
use domain::resources::Kind;
use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::invitations::like_pattern;

/// Name of the unique index on `(tenant_id, account_id)`.
const ACCOUNT_UNIQUE: &str = "resource_account_idx";

/// A resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub id: Uuid,
    pub kind: Kind,
    /// The member's name when linked, else its own.
    pub display_name: String,
    pub job_title: Option<String>,
    /// The member it is.
    pub account_id: Option<Uuid>,
    /// It was a member who left the tenant.
    pub former_member: bool,
    pub active: bool,
    pub version: i64,
    pub created_at: OffsetDateTime,
}

/// Position in the list, ordered by name then id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameKey {
    /// The name, lowercased by the database.
    pub name: String,
    pub id: Uuid,
}

/// A page of resources, and the position of its last one when more follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePage {
    pub items: Vec<Resource>,
    pub next: Option<NameKey>,
}

/// Filter of the list.
#[derive(Debug, Clone, Default)]
pub struct ResourceFilter<'a> {
    pub kind: Option<Kind>,
    pub active: Option<bool>,
    /// Part of the name or job title, any case.
    pub search: Option<&'a str>,
}

/// A resource as written: texts trimmed, empty ones absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceInput<'a> {
    pub kind: Kind,
    /// Ignored when linked: the member's name is kept instead.
    pub display_name: Option<&'a str>,
    pub job_title: Option<&'a str>,
    pub account_id: Option<Uuid>,
    pub active: bool,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// The account is linked to another resource of the tenant.
    AccountTaken,
    /// The account is not an internal member of the tenant.
    NotInternalMember,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    kind: String,
    display_name: String,
    job_title: Option<String>,
    account_id: Option<Uuid>,
    former_member: bool,
    active: bool,
    version: i64,
    created_at: OffsetDateTime,
}

impl From<Row> for Resource {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            // The database checks the kind.
            kind: Kind::parse(&row.kind).unwrap_or(Kind::Person),
            display_name: row.display_name,
            job_title: row.job_title,
            account_id: row.account_id,
            former_member: row.former_member,
            active: row.active,
            version: row.version,
            created_at: row.created_at,
        }
    }
}

struct ListRow {
    sort_key: String,
    id: Uuid,
    kind: String,
    display_name: String,
    job_title: Option<String>,
    account_id: Option<Uuid>,
    former_member: bool,
    active: bool,
    version: i64,
    created_at: OffsetDateTime,
}

/// One page of resources, by name.
pub async fn list(
    conn: &mut PgConnection,
    filter: &ResourceFilter<'_>,
    after: Option<&NameKey>,
    limit: usize,
) -> Result<ResourcePage, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let pattern = filter.search.map(like_pattern);
    let mut rows = sqlx::query_as!(
        ListRow,
        r#"
        SELECT lower(name) AS "sort_key!", id, kind, name AS "display_name!", job_title, account_id,
               former_member AS "former_member!", active, version, created_at
        FROM (
            SELECT r.*, coalesce(app_member_name(r.account_id), r.display_name) AS name,
                   r.former_account_id IS NOT NULL AS former_member
            FROM resource r
        ) r
        WHERE ($1::text IS NULL OR kind = $1)
          AND ($2::boolean IS NULL OR active = $2)
          AND ($3::text IS NULL OR lower(name) LIKE $3 OR lower(job_title) LIKE $3)
          AND ($4::text IS NULL OR (lower(name), id) > ($4, $5::uuid))
        ORDER BY lower(name), id
        LIMIT $6
        "#,
        filter.kind.map(Kind::as_str),
        filter.active,
        pattern,
        after.map(|k| k.name.as_str()),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let next = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().map(|row| NameKey {
            name: row.sort_key.clone(),
            id: row.id,
        })
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|row| {
            Resource::from(Row {
                id: row.id,
                kind: row.kind,
                display_name: row.display_name,
                job_title: row.job_title,
                account_id: row.account_id,
                former_member: row.former_member,
                active: row.active,
                version: row.version,
                created_at: row.created_at,
            })
        })
        .collect();
    Ok(ResourcePage { items, next })
}

/// The resource `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Resource>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, kind, coalesce(app_member_name(account_id), display_name) AS "display_name!", job_title,
               account_id, former_account_id IS NOT NULL AS "former_member!", active, version, created_at
        FROM resource
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Resource::from))
}

/// The resource of the member `account_id`, if any.
pub async fn find_by_account(
    conn: &mut PgConnection,
    account_id: Uuid,
) -> Result<Option<Resource>, sqlx::Error> {
    let id = sqlx::query_scalar!("SELECT id FROM resource WHERE account_id = $1", account_id)
        .fetch_optional(&mut *conn)
        .await?;
    match id {
        Some(id) => find(conn, id).await,
        None => Ok(None),
    }
}

/// Creates the resource `id`.
pub async fn insert(conn: &mut PgConnection, id: Uuid, input: &ResourceInput<'_>) -> WriteResult<Resource> {
    if let Some(account) = input.account_id
        && !is_internal_member(conn, account).await?
    {
        return Ok(Err(WriteConflict::NotInternalMember));
    }
    let inserted = sqlx::query!(
        r#"
        INSERT INTO resource (id, tenant_id, kind, display_name, job_title, account_id, active)
        VALUES ($1, app_current_tenant(), $2,
                coalesce((SELECT nullif(left(btrim(display_name), 200), '') FROM account WHERE id = $5), $3),
                $4, $5, $6)
        "#,
        id,
        input.kind.as_str(),
        input.display_name,
        input.job_title,
        input.account_id,
        input.active
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique_violation(inserted)? {
        return Ok(Err(conflict));
    }
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the resource `id`, if its version is one of `expected_versions`
/// (`None`: any). Linking it to a member forgets the member it was; making
/// it generic too.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &ResourceInput<'_>,
) -> WriteResult<Resource> {
    if let Some(account) = input.account_id
        && !is_internal_member(conn, account).await?
    {
        return Ok(Err(WriteConflict::NotInternalMember));
    }
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE resource
        SET kind = $3,
            display_name = coalesce((SELECT nullif(left(btrim(a.display_name), 200), '') FROM account a WHERE a.id = $6),
                                    $4, display_name),
            job_title = $5,
            account_id = $6,
            former_account_id = CASE WHEN $6::uuid IS NULL AND $3 = 'person' THEN former_account_id END,
            active = $7,
            version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        input.kind.as_str(),
        input.display_name,
        input.job_title,
        input.account_id,
        input.active
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique_violation(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// An internal member who can be linked to a person resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkableAccount {
    pub account_id: Uuid,
    pub display_name: String,
    pub email: String,
}

/// The internal members of the tenant linked to no resource yet, whose name
/// or email holds `search`, by name.
pub async fn linkable_accounts(
    conn: &mut PgConnection,
    search: Option<&str>,
    limit: usize,
) -> Result<Vec<LinkableAccount>, sqlx::Error> {
    let pattern = search.map(like_pattern);
    sqlx::query_as!(
        LinkableAccount,
        r#"
        SELECT a.id AS account_id, a.display_name, a.email
        FROM membership m
        JOIN account a ON a.id = m.account_id
        WHERE m.account_type = 'internal'
          AND NOT EXISTS (SELECT 1 FROM resource r WHERE r.account_id = m.account_id)
          AND ($1::text IS NULL OR a.email LIKE $1 OR lower(a.display_name) LIKE $1)
        ORDER BY lower(a.display_name), a.id
        LIMIT $2
        "#,
        pattern,
        i64::try_from(limit).unwrap_or(i64::MAX)
    )
    .fetch_all(conn)
    .await
}

async fn is_internal_member(conn: &mut PgConnection, account: Uuid) -> Result<bool, sqlx::Error> {
    let account_type = sqlx::query_scalar!(
        "SELECT account_type FROM membership WHERE account_id = $1",
        account
    )
    .fetch_optional(conn)
    .await?;
    Ok(account_type
        .as_deref()
        .and_then(AccountType::parse)
        .is_some_and(domain::resources::can_be_linked))
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM resource WHERE id = $1) AS "exists!""#,
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

fn map_unique_violation<T>(result: Result<T, sqlx::Error>) -> WriteResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(ACCOUNT_UNIQUE) => {
            Ok(Err(WriteConflict::AccountTaken))
        }
        Err(err) => Err(err),
    }
}
