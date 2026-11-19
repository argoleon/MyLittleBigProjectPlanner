//! SQL of list view exports (L06-US06). Row-level security scopes every
//! query to the current tenant; each member reads their own exports only.

use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

/// Job writing the file of a large export.
pub const EXPORT_JOB: &str = "export.list";
/// Job deleting an export and its file once expired.
pub const PURGE_JOB: &str = "export.purge";

/// The object key of an export's file.
pub fn object_key(tenant: Uuid, export: Uuid, extension: &str) -> String {
    format!("{tenant}/exports/{export}.{extension}")
}

/// An export of a list view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListExport {
    pub id: Uuid,
    pub account_id: Uuid,
    pub file_name: String,
    pub format: String,
    pub query: String,
    pub columns: Vec<String>,
    pub language: String,
    /// `queued`, `ready` or `failed`.
    pub status: String,
    pub row_count: Option<i32>,
    pub object_key: Option<String>,
    pub size_bytes: Option<i64>,
    pub failure: Option<String>,
    pub completed_at: Option<OffsetDateTime>,
    pub expires_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
}

/// An export to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExport<'a> {
    pub id: Uuid,
    pub account_id: Uuid,
    pub file_name: &'a str,
    pub format: &'a str,
    pub query: &'a str,
    pub columns: &'a [String],
    pub language: &'a str,
    pub expires_at: OffsetDateTime,
}

/// Opens an export, queued.
pub async fn insert(conn: &mut PgConnection, new: &NewExport<'_>) -> Result<ListExport, sqlx::Error> {
    sqlx::query_as!(
        ListExport,
        r#"
        INSERT INTO list_export (id, tenant_id, account_id, file_name, format, query, columns, language, expires_at)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, account_id, file_name, format, query, columns, language, status, row_count, object_key,
                  size_bytes, failure, completed_at, expires_at, created_at
        "#,
        new.id,
        new.account_id,
        new.file_name,
        new.format,
        new.query,
        new.columns,
        new.language,
        new.expires_at
    )
    .fetch_one(conn)
    .await
}

/// The export `id`, whoever asked for it (the worker's view).
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<ListExport>, sqlx::Error> {
    sqlx::query_as!(
        ListExport,
        r#"
        SELECT id, account_id, file_name, format, query, columns, language, status, row_count, object_key,
               size_bytes, failure, completed_at, expires_at, created_at
        FROM list_export WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

/// The export `id` of `account`, unless expired.
pub async fn find_of(
    conn: &mut PgConnection,
    account: Uuid,
    id: Uuid,
) -> Result<Option<ListExport>, sqlx::Error> {
    sqlx::query_as!(
        ListExport,
        r#"
        SELECT id, account_id, file_name, format, query, columns, language, status, row_count, object_key,
               size_bytes, failure, completed_at, expires_at, created_at
        FROM list_export WHERE id = $1 AND account_id = $2 AND expires_at > now()
        "#,
        id,
        account
    )
    .fetch_optional(conn)
    .await
}

/// The unexpired exports of `account`, newest first.
pub async fn list_of(
    conn: &mut PgConnection,
    account: Uuid,
    limit: i64,
) -> Result<Vec<ListExport>, sqlx::Error> {
    sqlx::query_as!(
        ListExport,
        r#"
        SELECT id, account_id, file_name, format, query, columns, language, status, row_count, object_key,
               size_bytes, failure, completed_at, expires_at, created_at
        FROM list_export WHERE account_id = $1 AND expires_at > now()
        ORDER BY created_at DESC, id DESC
        LIMIT $2
        "#,
        account,
        limit
    )
    .fetch_all(conn)
    .await
}

/// Marks the export `id` ready: its file at `object_key`. `false` if it is
/// no longer queued (done already, or gone).
pub async fn complete(
    conn: &mut PgConnection,
    id: Uuid,
    object_key: &str,
    rows: i32,
    size: i64,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE list_export
        SET status = 'ready', object_key = $2, row_count = $3, size_bytes = $4, completed_at = now(),
            version = version + 1
        WHERE id = $1 AND status = 'queued'
        "#,
        id,
        object_key,
        rows,
        size
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Marks the export `id` failed with the stable code `failure`.
pub async fn fail(conn: &mut PgConnection, id: Uuid, failure: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE list_export
        SET status = 'failed', failure = $2, completed_at = now(), version = version + 1
        WHERE id = $1 AND status = 'queued'
        "#,
        id,
        failure
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

// --- Writing the file ------------------------------------------------------

/// Tickets read at a time while writing a file.
const PAGE: usize = 1_000;

/// Why an export could not be written.
#[derive(Debug)]
pub enum ProduceError {
    /// The query no longer runs (a field deleted since…).
    Query(domain::query::QueryError),
    /// More tickets than [`domain::export::MAX_ROWS`].
    TooMany(usize),
    Database(sqlx::Error),
}

impl From<sqlx::Error> for ProduceError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

impl From<crate::search::SearchError> for ProduceError {
    fn from(err: crate::search::SearchError) -> Self {
        match err {
            crate::search::SearchError::Query(e) => Self::Query(e),
            crate::search::SearchError::Database(e) => Self::Database(e),
            crate::search::SearchError::Cursor => Self::Database(sqlx::Error::Protocol("cursor".to_owned())),
        }
    }
}

/// A file written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Produced {
    pub content: String,
    pub rows: usize,
}

/// The projects `account` sees: `None` for a tenant admin (all).
pub async fn sight_of(conn: &mut PgConnection, account: Uuid) -> Result<Option<Vec<Uuid>>, sqlx::Error> {
    let member = crate::accounts::find(conn, account).await?;
    if member.is_some_and(|m| m.tenant_role == crate::accounts::TenantRole::TenantAdmin) {
        return Ok(None);
    }
    Ok(Some(
        crate::grants::sight(conn, account)
            .await?
            .visible
            .into_iter()
            .collect(),
    ))
}

/// How many tickets `query` finds for `account`.
pub async fn count(
    conn: &mut PgConnection,
    account: Uuid,
    query: &domain::query::Query,
) -> Result<usize, ProduceError> {
    let visible = sight_of(conn, account).await?;
    let time_zone = crate::search::time_zone_of(conn, account).await?;
    let context = crate::search::Context {
        account_id: account,
        time_zone: &time_zone,
        visible_projects: visible.as_deref(),
        audience: crate::full_text::Audience::Members,
    };
    let page = crate::search::search(conn, query, &context, None, 1, true).await?;
    Ok(page.total.map_or(0, |t| usize::try_from(t).unwrap_or(usize::MAX)))
}

/// The labels of the configuration of a project, in the export's language.
struct Labels {
    types: std::collections::HashMap<Uuid, String>,
    statuses: std::collections::HashMap<Uuid, String>,
}

/// Writes the CSV of the tickets `query` finds for `account` (as they see
/// them, in their time zone), with `columns`, headers and labels in
/// `language`. Refuses beyond [`domain::export::MAX_ROWS`] tickets.
pub async fn produce(
    conn: &mut PgConnection,
    account: Uuid,
    query: &domain::query::Query,
    columns: &[domain::export::Column],
    language: &str,
) -> Result<Produced, ProduceError> {
    use domain::export::{Cell, Column, MAX_ROWS};

    let visible = sight_of(conn, account).await?;
    let time_zone = crate::search::time_zone_of(conn, account).await?;
    let context = crate::search::Context {
        account_id: account,
        time_zone: &time_zone,
        visible_projects: visible.as_deref(),
        audience: crate::full_text::Audience::Members,
    };
    let headers: Vec<String> = columns.iter().map(|c| c.header(language).to_owned()).collect();
    let mut content = domain::export::csv_header(&headers);
    let mut projects: std::collections::HashMap<Uuid, (String, Option<Uuid>)> =
        std::collections::HashMap::new();
    let mut labels: std::collections::HashMap<Uuid, Labels> = std::collections::HashMap::new();
    let mut cursor: Option<String> = None;
    let mut rows = 0_usize;
    loop {
        let page =
            crate::search::search(conn, query, &context, cursor.as_deref(), PAGE, cursor.is_none()).await?;
        if let Some(total) = page.total.and_then(|t| usize::try_from(t).ok())
            && total > MAX_ROWS
        {
            return Err(ProduceError::TooMany(total));
        }
        let unknown: Vec<Uuid> = page
            .items
            .iter()
            .map(|i| i.project_id)
            .filter(|p| !projects.contains_key(p))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        for project in crate::search::projects_of(conn, &unknown).await? {
            projects.insert(project.id, (project.name, project.config_id));
        }
        for issue in &page.items {
            let Some((project_name, config)) = projects.get(&issue.project_id).cloned() else {
                continue;
            };
            if let Some(config) = config
                && !labels.contains_key(&config)
            {
                labels.insert(config, labels_of(conn, config, language).await?);
            }
            let labels = config.and_then(|c| labels.get(&c));
            let text = |value: Option<&String>| value.map_or(Cell::Empty, |v| Cell::Text(v.clone()));
            let cells: Vec<Cell> = columns
                .iter()
                .map(|column| match column {
                    Column::Key => Cell::Text(issue.key.clone()),
                    Column::Summary => Cell::Text(issue.summary.clone()),
                    Column::Project => Cell::Text(project_name.clone()),
                    Column::Type => text(labels.and_then(|l| l.types.get(&issue.type_id))),
                    Column::Status => text(labels.and_then(|l| l.statuses.get(&issue.status_id))),
                    Column::Priority => domain::export::priority_label(&issue.priority, language)
                        .map_or(Cell::Empty, |p| Cell::Text(p.to_owned())),
                    Column::Assignee => text(issue.assignee_name.as_ref()),
                    Column::Reporter => text(issue.reporter_name.as_ref()),
                    Column::Due => issue.due_date.map_or(Cell::Empty, |d| Cell::Date(d.to_string())),
                    Column::Start => issue
                        .start_date
                        .map_or(Cell::Empty, |d| Cell::Date(d.to_string())),
                    Column::Created => Cell::DateTime(utc(issue.created_at)),
                    Column::Updated => Cell::DateTime(utc(issue.updated_at)),
                })
                .collect();
            domain::export::csv_line(&cells, &mut content);
            rows += 1;
        }
        match page.next_cursor {
            Some(next) if rows < MAX_ROWS => cursor = Some(next),
            _ => break,
        }
    }
    Ok(Produced { content, rows })
}

/// `at` in RFC 3339, in UTC, to the second.
fn utc(at: OffsetDateTime) -> String {
    let at = at
        .to_offset(time::UtcOffset::UTC)
        .replace_nanosecond(0)
        .unwrap_or(at);
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

async fn labels_of(conn: &mut PgConnection, config: Uuid, language: &str) -> Result<Labels, sqlx::Error> {
    let Some(version) = crate::configs::find(conn, config).await?.map(|c| c.published.id) else {
        return Ok(Labels {
            types: std::collections::HashMap::new(),
            statuses: std::collections::HashMap::new(),
        });
    };
    let label = |translations: &std::collections::BTreeMap<String, String>, key: &str| {
        domain::i18n::resolve(translations, language, "en")
            .unwrap_or(key)
            .to_owned()
    };
    let types = crate::issue_types::list(conn, version)
        .await?
        .into_iter()
        .map(|t| (t.item_id, label(&t.label_translations, &t.key)))
        .collect();
    let statuses = crate::workflows::load(conn, version)
        .await?
        .statuses
        .into_iter()
        .map(|s| (s.item_id, label(&s.label_translations, &s.key)))
        .collect();
    Ok(Labels { types, statuses })
}

// --- Jobs ------------------------------------------------------------------

/// Payload of [`EXPORT_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub export_id: Uuid,
}

/// Payload of [`PURGE_JOB`]: the file's key is known before it exists, so
/// that a file written by a run that failed afterwards goes too.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeJob {
    pub export_id: Uuid,
    pub object_key: String,
}

/// What running an export did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    /// Written: this many rows, this many bytes.
    Ready { rows: usize, bytes: usize },
    /// Refused for good, with the stable code kept on the export.
    Failed(&'static str),
    /// Gone, or done already.
    Nothing,
}

/// Why running an export must be tried again.
#[derive(Debug)]
pub enum RunError {
    Database(sqlx::Error),
    Storage(crate::storage::StorageError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "database: {e}"),
            Self::Storage(e) => write!(f, "storage: {e}"),
        }
    }
}

impl From<sqlx::Error> for RunError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

/// Stores the file of the export `export` (queued, of the current tenant
/// `tenant`) and marks it ready, telling its requester through the outbox
/// (`export.ready`); or marks it failed when its query no longer runs or
/// finds too many tickets.
pub async fn store(
    conn: &mut PgConnection,
    storage: &crate::storage::Storage,
    tenant: Uuid,
    export: &ListExport,
    produced: &Produced,
) -> Result<Ran, RunError> {
    let format = domain::export::Format::parse(&export.format).unwrap_or_default();
    let key = object_key(tenant, export.id, format.extension());
    let bytes = produced.content.len();
    storage
        .write(
            &key,
            hyper::body::Bytes::from(produced.content.clone()),
            format.content_type(),
        )
        .await
        .map_err(RunError::Storage)?;
    let rows = i32::try_from(produced.rows).unwrap_or(i32::MAX);
    if !complete(
        conn,
        export.id,
        &key,
        rows,
        i64::try_from(bytes).unwrap_or(i64::MAX),
    )
    .await?
    {
        return Ok(Ran::Nothing);
    }
    Ok(Ran::Ready {
        rows: produced.rows,
        bytes,
    })
}

/// Runs the queued export `id` of the current tenant `tenant` in the
/// background: writes its file, then tells its requester.
pub async fn run(
    conn: &mut PgConnection,
    storage: &crate::storage::Storage,
    tenant: Uuid,
    id: Uuid,
) -> Result<Ran, RunError> {
    let Some(export) = find(conn, id).await? else {
        return Ok(Ran::Nothing);
    };
    if export.status != "queued" {
        return Ok(Ran::Nothing);
    }
    let (query, columns) = match prepare(&export) {
        Ok(prepared) => prepared,
        Err(code) => return failed(conn, export.id, code).await,
    };
    let produced = match produce(conn, export.account_id, &query, &columns, &export.language).await {
        Ok(produced) => produced,
        Err(ProduceError::Query(_)) => return failed(conn, export.id, "query.invalid").await,
        Err(ProduceError::TooMany(_)) => return failed(conn, export.id, "export.too_large").await,
        Err(ProduceError::Database(e)) => return Err(RunError::Database(e)),
    };
    let ran = store(conn, storage, tenant, &export, &produced).await?;
    if let Ran::Ready { rows, .. } = ran {
        crate::outbox::record(
            conn,
            &crate::outbox::Event {
                kind: crate::notifications::EXPORT_READY,
                aggregate_type: "list_export",
                aggregate_id: export.id,
                payload: serde_json::json!({
                    "accountId": export.account_id,
                    "fileName": export.file_name,
                    "rows": rows,
                }),
                actor_id: None,
            },
        )
        .await?;
    }
    Ok(ran)
}

async fn failed(conn: &mut PgConnection, id: Uuid, code: &'static str) -> Result<Ran, RunError> {
    Ok(if fail(conn, id, code).await? {
        Ran::Failed(code)
    } else {
        Ran::Nothing
    })
}

/// The query and columns of `export`, as stored.
fn prepare(export: &ListExport) -> Result<(domain::query::Query, Vec<domain::export::Column>), &'static str> {
    let query = domain::query::parse(&export.query).map_err(|_| "query.invalid")?;
    let columns = domain::export::columns(&export.columns).map_err(|_| "query.invalid")?;
    Ok((query, columns))
}

/// Deletes an expired export and its file.
pub async fn purge(
    conn: &mut PgConnection,
    storage: &crate::storage::Storage,
    job: &PurgeJob,
) -> Result<bool, RunError> {
    let deleted = delete(conn, job.export_id).await?.is_some();
    storage.delete(&job.object_key).await.map_err(RunError::Storage)?;
    Ok(deleted)
}

/// Deletes the export `id`; its file's key, to delete it too.
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<Option<Option<String>>, sqlx::Error> {
    sqlx::query_scalar!("DELETE FROM list_export WHERE id = $1 RETURNING object_key", id)
        .fetch_optional(conn)
        .await
}
