//! Exports of list views (specification §6, L06-US06): the tickets a query
//! finds, with the columns a screen displays, as a CSV file.
//!
//! - Up to 1,000 tickets, the file is written at once: the export answers
//!   `ready`. Beyond, the worker writes it (`export.list`) and the member is
//!   notified in the bell, and by email if they chose so: it answers
//!   `queued`. More than 100,000 tickets: `export.too_large`.
//! - The file is kept 24 hours (`export.purge`), for its requester only,
//!   who downloads it through a short-lived link (`GET /exports/{id}/file`).
//! - The query runs as the requester sees the tickets, in their time zone;
//!   headers and labels in the language asked for.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::export::{FILE_LIFETIME_HOURS, Format, MAX_ROWS, SYNC_MAX_ROWS};
use infra::list_exports::{self as repo, ListExport, NewExport, ProduceError, PurgeJob, Ran};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;

use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issue_attachments::{browser_host, redirect, storage_of};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::search::query_problem;
use crate::tenant::{AppState, TenantTx};

pub const TAG: &str = "exports";

const PATH: &str = "/api/v1/exports";

/// What an export is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ExportFormat)]
pub enum FormatView {
    #[default]
    Csv,
}

/// Where an export stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ExportStatus)]
pub enum StatusView {
    /// Being written by the worker.
    Queued,
    /// Its file can be downloaded.
    Ready,
    /// Refused: see `failure`.
    Failed,
}

/// An export of a list view.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ListExport)]
pub struct ExportView {
    id: Uuid,
    #[schema(example = "search-2026-11-18.csv")]
    file_name: String,
    format: FormatView,
    status: StatusView,
    /// Tickets written, once ready.
    rows: Option<i32>,
    size_bytes: Option<i64>,
    /// Why it failed: `query.invalid` (the query no longer runs) or
    /// `export.too_large`.
    failure: Option<String>,
    /// `GET` it to download the file, once ready.
    #[schema(example = "/api/v1/exports/01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a/file")]
    download_path: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    completed_at: Option<OffsetDateTime>,
    /// When the file is deleted.
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

fn view(export: ListExport) -> ExportView {
    let status = match export.status.as_str() {
        "ready" => StatusView::Ready,
        "failed" => StatusView::Failed,
        _ => StatusView::Queued,
    };
    ExportView {
        download_path: (status == StatusView::Ready).then(|| format!("{PATH}/{}/file", export.id)),
        id: export.id,
        file_name: export.file_name,
        format: FormatView::Csv,
        status,
        rows: export.row_count,
        size_bytes: export.size_bytes,
        failure: export.failure,
        created_at: export.created_at,
        completed_at: export.completed_at,
        expires_at: export.expires_at,
    }
}

/// The member's exports.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportList {
    /// Those not expired, newest first.
    items: Vec<ExportView>,
}

/// A list view to export.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NewExport, example = json!({
    "query": "project = PLAT ORDER BY priority DESC",
    "columns": ["key", "summary", "status", "assignee"],
    "format": "csv",
    "name": "search"
}))]
pub struct ExportBody {
    /// A query of the language: the tickets and their order.
    query: String,
    /// The columns displayed, in their order: `key`, `summary`, `project`,
    /// `type`, `status`, `priority`, `assignee`, `reporter`, `due`, `start`,
    /// `created`, `updated`.
    columns: Vec<String>,
    #[serde(default)]
    format: FormatView,
    /// What the file is named after (`search`, `my-tickets`), with the date.
    #[serde(default = "default_name")]
    name: String,
}

fn default_name() -> String {
    "export".to_owned()
}

/// `POST /api/v1/exports`.
#[utoipa::path(
    post,
    path = "/exports",
    tag = TAG,
    operation_id = "createExport",
    summary = "Export the tickets of a query",
    description = "Any member: the tickets they may see, as the query finds them, with the columns \
                   given and their headers in the request's language. Up to 1,000 tickets, the file \
                   is written at once (`201`, `ready`); beyond, in the background (`202`, \
                   `queued`), the member being notified when it is ready. The file is kept 24 \
                   hours, for them only.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the headers and labels")),
    request_body = ExportBody,
    responses(
        (status = 201, description = "Written: download it", body = ExportView),
        (status = 202, description = "Queued: written in the background", body = ExportView),
        (status = 400, description = "`query.invalid`; `validation_failed`: `columns` (`required`, \
                                      `too_many`), `columns.{i}` (`unknown_field`, `duplicate`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`export.too_large`: more than 100,000 tickets", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 503, description = "`storage.unavailable`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<ExportBody>,
) -> Result<Response, Problem> {
    let columns = domain::export::columns(&body.columns).map_err(|e| Problem::validation(vec![e]))?;
    let query = domain::query::parse(&body.query).map_err(|e| query_problem(&e))?;
    let storage = storage_of(&state)?;
    let account = tx.account_id();
    let total = repo::count(&mut tx, account, &query)
        .await
        .map_err(produce_problem)?;
    if total > MAX_ROWS {
        return Err(too_large(total));
    }
    let language = Language::of(&tx, &accept);
    let format = Format::Csv;
    let now = OffsetDateTime::now_utc();
    let file_name = domain::export::file_name(&body.name, &now.date().to_string(), format);
    let new = NewExport {
        id: infra::id::new_id(),
        account_id: account,
        file_name: &file_name,
        format: format.as_str(),
        query: body.query.trim(),
        columns: &body.columns,
        language: language.locale(),
        expires_at: now + Duration::hours(FILE_LIFETIME_HOURS),
    };
    let export = repo::insert(&mut tx, &new).await?;
    let purge = PurgeJob {
        export_id: export.id,
        object_key: repo::object_key(tx.tenant_id(), export.id, format.extension()),
    };
    let lifetime = std::time::Duration::from_secs(u64::try_from(FILE_LIFETIME_HOURS).unwrap_or(24) * 3600);
    let payload = serde_json::to_value(&purge).map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue_in(&mut tx, repo::PURGE_JOB, &payload, lifetime).await?;
    let status = if total <= SYNC_MAX_ROWS {
        let produced = repo::produce(&mut tx, account, &query, &columns, language.locale())
            .await
            .map_err(produce_problem)?;
        let tenant = tx.tenant_id();
        match repo::store(&mut tx, storage, tenant, &export, &produced).await {
            Ok(Ran::Ready { .. }) => {}
            Ok(_) => return Err(Problem::new(ErrorCode::Internal)),
            Err(infra::list_exports::RunError::Storage(e)) => return Err(e.into()),
            Err(infra::list_exports::RunError::Database(e)) => return Err(e.into()),
        }
        StatusCode::CREATED
    } else {
        let payload = serde_json::to_value(repo::ExportJob { export_id: export.id })
            .map_err(|e| Problem::internal(&e))?;
        infra::jobs::enqueue(&mut tx, repo::EXPORT_JOB, &payload).await?;
        StatusCode::ACCEPTED
    };
    let export = repo::find(&mut tx, export.id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::Internal))?;
    tracing::info!(target: "audit", actor = %account, export = %export.id, total, "list export asked");
    tx.commit().await?;
    let mut response = (status, ApiJson(view(export))).into_response();
    language.headers(&mut response);
    Ok(response)
}

fn too_large(total: usize) -> Problem {
    Problem::new(ErrorCode::ExportTooLarge).with_detail(format!(
        "the query finds {total} tickets; an export holds {MAX_ROWS} at most"
    ))
}

fn produce_problem(error: ProduceError) -> Problem {
    match error {
        ProduceError::Query(e) => query_problem(&e),
        ProduceError::TooMany(total) => too_large(total),
        ProduceError::Database(e) => e.into(),
    }
}

/// The export `id` of the member, unless expired.
async fn mine(tx: &mut TenantTx, id: Uuid) -> Result<ListExport, Problem> {
    let account = tx.account_id();
    repo::find_of(tx, account, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such export"))
}

/// `GET /api/v1/exports`.
#[utoipa::path(
    get,
    path = "/exports",
    tag = TAG,
    operation_id = "listMyExports",
    summary = "The member's exports",
    description = "Those of the last 24 hours, newest first (50 at most).",
    responses(
        (status = 200, description = "The exports", body = ExportList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(mut tx: TenantTx) -> Result<ApiJson<ExportList>, Problem> {
    let account = tx.account_id();
    let items = repo::list_of(&mut tx, account, 50)
        .await?
        .into_iter()
        .map(view)
        .collect();
    tx.commit().await?;
    Ok(ApiJson(ExportList { items }))
}

/// `GET /api/v1/exports/{id}`.
#[utoipa::path(
    get,
    path = "/exports/{id}",
    tag = TAG,
    operation_id = "getExport",
    summary = "Read an export",
    description = "Its requester only, until it expires.",
    params(("id" = Uuid, Path, description = "Id of the export")),
    responses(
        (status = 200, description = "The export", body = ExportView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such export of the member, or expired", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(mut tx: TenantTx, ApiPath(id): ApiPath<Uuid>) -> Result<ApiJson<ExportView>, Problem> {
    let export = mine(&mut tx, id).await?;
    tx.commit().await?;
    Ok(ApiJson(view(export)))
}

/// `GET /api/v1/exports/{id}/file`.
#[utoipa::path(
    get,
    path = "/exports/{id}/file",
    tag = TAG,
    operation_id = "downloadExport",
    summary = "Download an export's file",
    description = "Its requester only, once ready and until it expires: a redirect to a link of the \
                   storage valid 15 minutes.",
    params(("id" = Uuid, Path, description = "Id of the export")),
    responses(
        (status = 303, description = "To the file", headers(("Location" = String, description = "Short-lived link to the file"))),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such export of the member, or expired", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`export.not_ready`: queued or failed", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 503, description = "`storage.unavailable`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn download(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let storage = storage_of(&state)?;
    let export = mine(&mut tx, id).await?;
    let Some(key) = export.object_key.as_deref().filter(|_| export.status == "ready") else {
        return Err(Problem::new(ErrorCode::ExportNotReady));
    };
    let format = Format::parse(&export.format).unwrap_or_default();
    let url = storage.download_url(
        &browser_host(&state, &tx, &headers),
        key,
        format.content_type(),
        &infra::storage::content_disposition("attachment", &export.file_name),
    )?;
    tx.commit().await?;
    redirect(&url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(status: &str) -> ListExport {
        let at = OffsetDateTime::UNIX_EPOCH;
        ListExport {
            id: Uuid::nil(),
            account_id: Uuid::nil(),
            file_name: "search-2026-11-18.csv".to_owned(),
            format: "csv".to_owned(),
            query: String::new(),
            columns: vec!["key".to_owned()],
            language: "en".to_owned(),
            status: status.to_owned(),
            row_count: None,
            object_key: None,
            size_bytes: None,
            failure: None,
            completed_at: None,
            expires_at: at,
            created_at: at,
        }
    }

    #[test]
    fn only_a_ready_export_has_a_download_path() {
        assert_eq!(
            view(export("ready")).download_path.as_deref(),
            Some("/api/v1/exports/00000000-0000-0000-0000-000000000000/file")
        );
        assert_eq!(view(export("queued")).download_path, None);
        assert_eq!(view(export("failed")).status, StatusView::Failed);
        assert_eq!(too_large(120_000).code, ErrorCode::ExportTooLarge);
    }
}
