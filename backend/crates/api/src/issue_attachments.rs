//! Attachments of a ticket (specification §12, L05-US05).
//!
//! The browser uploads and downloads directly to and from the storage,
//! through presigned URLs on the application's own origin
//! (`infra::storage`): the API only checks and records.
//!
//! 1. `POST …/attachments` announces a file (name, size): `edit` on the
//!    project, the maximum size and the tenant's quota are checked, and the
//!    answer holds the URL to `PUT` the file to.
//! 2. `POST …/attachments/{id}/uploaded` once it is: its size is checked in
//!    the storage, the antivirus job queued (`infra::attachments::scan`),
//!    the history and the outbox written.
//! 3. `GET …/attachments/{id}/content` redirects to the file once clean,
//!    always as a download; `…/thumbnail` to an image's thumbnail.
//!
//! An upload that never comes is forgotten after an hour. Removing an
//! attachment takes its uploader (still holding `edit`) or a moderator.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::attachments::{Acting, Status, UploadRefusal, can_delete, check_upload, clean_name};
use domain::permissions::Permission;
use domain::validation::FieldError;
use infra::attachments::{self as repo, Attachment, AttachmentJob, NewAttachment};
use infra::issues::Issue;
use infra::nodes::Node;
use infra::storage::Storage;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::extract::{ApiJson, ApiPath};
use crate::issues::{PersonView, TAG, person, visible_issue};
use crate::nodes::{ensure_changeable, is_tenant_admin};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// Where an attachment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = AttachmentStatus)]
pub enum StatusView {
    /// Announced, not uploaded yet.
    Pending,
    /// Uploaded, being scanned by the antivirus.
    Scanning,
    /// Downloadable.
    Clean,
    /// A virus was found: removed, never downloadable.
    Infected,
    /// Not the file announced (another size): removed.
    Rejected,
}

impl From<Status> for StatusView {
    fn from(status: Status) -> Self {
        match status {
            Status::Pending => Self::Pending,
            Status::Scanning => Self::Scanning,
            Status::Clean => Self::Clean,
            Status::Infected => Self::Infected,
            Status::Rejected => Self::Rejected,
        }
    }
}

/// A file attached to a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueAttachment)]
pub struct AttachmentView {
    id: Uuid,
    #[schema(example = "Recette OIDC.pdf")]
    name: String,
    /// In bytes.
    size: i64,
    /// As the browser gave it.
    media_type: String,
    status: StatusView,
    /// The signature the antivirus found.
    #[schema(required = true)]
    signature: Option<String>,
    /// None: an erased account; "Deleted user #N" once erased.
    #[schema(required = true)]
    uploaded_by: Option<PersonView>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    /// An image's thumbnail can be read (`…/thumbnail`).
    has_thumbnail: bool,
    /// The member reading may remove it.
    can_delete: bool,
}

/// A file to attach.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NewAttachmentBody, example = json!({"name": "Recette OIDC.pdf", "size": 3145728, "mediaType": "application/pdf"}))]
pub struct NewAttachmentBody {
    /// The file's name; only its last path component is kept.
    name: String,
    /// In bytes: the storage refuses an upload of another size.
    size: u64,
    #[serde(default)]
    media_type: Option<String>,
}

/// An attachment announced, and where to upload it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AttachmentUpload)]
pub struct UploadView {
    attachment: AttachmentView,
    /// `PUT` the file here, with its exact size, within 15 minutes: a path
    /// on the application's origin (`/mlbpp-attachments/…`).
    upload_url: String,
}

pub(crate) fn storage_of(state: &AppState) -> Result<&Storage, Problem> {
    state
        .storage
        .as_deref()
        .ok_or_else(|| Problem::new(ErrorCode::StorageUnavailable))
}

/// What the member may do with the attachments of `project`.
async fn acting(state: &AppState, tx: &mut TenantTx, project: &Node) -> Result<Acting, Problem> {
    let account = tx.account_id().as_u128();
    if is_tenant_admin(tx) {
        return Ok(Acting {
            account,
            edits: true,
            moderates: true,
        });
    }
    let permissions = permissions::effective(state, tx, Some(project.id)).await?;
    Ok(Acting {
        account,
        edits: permissions.contains(Permission::Edit),
        moderates: permissions.contains(Permission::AdministerNode),
    })
}

fn view(attachment: Attachment, acting: &Acting) -> AttachmentView {
    AttachmentView {
        can_delete: can_delete(attachment.uploaded_by.map(|id| id.as_u128()), acting),
        uploaded_by: person(attachment.uploaded_by, attachment.uploader_name.as_ref()),
        has_thumbnail: attachment.status.downloadable() && attachment.thumbnail_key.is_some(),
        status: attachment.status.into(),
        id: attachment.id,
        name: attachment.name,
        size: attachment.size,
        media_type: attachment.media_type,
        signature: attachment.signature,
        created_at: attachment.created_at,
    }
}

/// The attachment `id` of `issue`; 404 otherwise.
async fn attachment_of(
    tx: &mut TenantTx,
    issue: &Issue,
    id: Uuid,
    lock: bool,
) -> Result<Attachment, Problem> {
    let found = if lock {
        repo::lock(tx, id).await?
    } else {
        repo::find(tx, id).await?
    };
    found
        .filter(|a| a.issue_id == issue.id)
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such attachment"))
}

/// The host through which the browser reached the API (`acme.example.org`,
/// `localhost:18443`): presigned URLs are signed for it, so that the browser
/// reaches the storage on its own origin. Only ever given back to the same
/// browser; the tenant was already resolved from it.
pub(crate) fn browser_host(state: &AppState, tx: &TenantTx, headers: &HeaderMap) -> String {
    headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            // `https://acme.example.org:8443` → `acme.example.org:8443`.
            let public = state.auth.public_url().for_tenant(tx.slug());
            let rest = public.split_once("://").map_or(public.as_str(), |(_, rest)| rest);
            rest.split('/').next().map(str::to_owned)
        })
        .unwrap_or_default()
}

pub(crate) fn redirect(url: &str) -> Result<Response, Problem> {
    let location = HeaderValue::from_str(url).map_err(|e| Problem::internal(&e))?;
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// `GET /api/v1/issues/{idOrKey}/attachments`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/attachments",
    tag = TAG,
    operation_id = "listIssueAttachments",
    summary = "The files attached to a ticket",
    description = "Whoever sees its project: the files uploaded, oldest first, with their scan \
                   status. Files announced but not uploaded yet are left out.",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    responses(
        (status = 200, description = "The attachments", body = Vec<AttachmentView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<ApiJson<Vec<AttachmentView>>, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    let attachments = repo::of_issue(&mut tx, issue.id).await?;
    tx.commit().await?;
    Ok(ApiJson(
        attachments.into_iter().map(|a| view(a, &acting)).collect(),
    ))
}

/// `POST /api/v1/issues/{idOrKey}/attachments`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/attachments",
    tag = TAG,
    operation_id = "createIssueAttachment",
    summary = "Announce a file to attach to a ticket",
    description = "Members with `edit` on its project (tenant admins always). Checks the size \
                   against the maximum (`ATTACHMENT_MAX_BYTES`, 50 MB by default) and the \
                   organization's storage quota, then answers the URL to `PUT` the file to. \
                   Call `…/uploaded` once it is; an upload that never comes is forgotten after \
                   an hour.",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    request_body = NewAttachmentBody,
    responses(
        (status = 201, description = "The attachment, pending its upload", body = UploadView),
        (status = 400, description = "`validation_failed`: `name` `required`, `size` `empty`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 413, description = "`attachment.too_large`, or `attachment.quota_exceeded`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 503, description = "`storage.unavailable`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<NewAttachmentBody>,
) -> Result<Response, Problem> {
    let storage = storage_of(&state)?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    if !acting.edits {
        return Err(Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: edit"));
    }
    ensure_changeable(&project)?;
    let Some(name) = clean_name(&body.name) else {
        return Err(Problem::validation(vec![FieldError::new("name", "required")]));
    };
    let used = u64::try_from(repo::used_bytes(&mut tx).await?).unwrap_or(0);
    let quota = repo::quota(&mut tx).await?.and_then(|q| u64::try_from(q).ok());
    match check_upload(body.size, storage.max_attachment_bytes(), used, quota) {
        Ok(()) => {}
        Err(UploadRefusal::Empty) => {
            return Err(Problem::validation(vec![FieldError::new(
                "size",
                UploadRefusal::Empty.code(),
            )]));
        }
        Err(UploadRefusal::TooLarge) => {
            return Err(Problem::new(ErrorCode::AttachmentTooLarge)
                .with_detail(format!("at most {} bytes", storage.max_attachment_bytes())));
        }
        Err(UploadRefusal::QuotaExceeded) => return Err(Problem::new(ErrorCode::AttachmentQuotaExceeded)),
    }
    let media_type = body
        .media_type
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.len() <= 255 && t.is_ascii())
        .unwrap_or("application/octet-stream");
    let id = infra::id::new_id();
    let key = infra::storage::upload_key(tx.tenant_id(), id);
    let size = i64::try_from(body.size).map_err(|e| Problem::internal(&e))?;
    let uploader = tx.account_id();
    let attachment = repo::insert(
        &mut tx,
        &NewAttachment {
            id,
            issue_id: issue.id,
            name: &name,
            size,
            media_type,
            object_key: &key,
            uploaded_by: uploader,
        },
    )
    .await?;
    let payload =
        serde_json::to_value(AttachmentJob { attachment_id: id }).map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue_in(&mut tx, repo::EXPIRE_JOB, &payload, repo::UPLOAD_WINDOW).await?;
    let upload_url = storage.upload_url(&browser_host(&state, &tx, &headers), &key, body.size)?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        ApiJson(UploadView {
            attachment: view(attachment, &acting),
            upload_url,
        }),
    )
        .into_response())
}

/// `POST /api/v1/issues/{idOrKey}/attachments/{attachmentId}/uploaded`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/attachments/{attachmentId}/uploaded",
    tag = TAG,
    operation_id = "completeIssueAttachment",
    summary = "Tell that a file was uploaded",
    description = "Its uploader. Checks that the storage holds the file whole, queues its \
                   antivirus scan (status `scanning`), records it in the ticket's history and \
                   writes `attachment.added` to the event outbox. Calling it again changes nothing.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("attachmentId" = Uuid, Path, description = "Id of the attachment"),
    ),
    responses(
        (status = 200, description = "The attachment", body = AttachmentView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: not its uploader, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member, or attachment of it", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`attachment.not_uploaded`: the storage does not hold the file whole", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 503, description = "`storage.unavailable`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn uploaded(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id_or_key, attachment_id)): ApiPath<(String, Uuid)>,
) -> Result<ApiJson<AttachmentView>, Problem> {
    let storage = storage_of(&state)?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    let attachment = attachment_of(&mut tx, &issue, attachment_id, true).await?;
    if attachment.uploaded_by != Some(tx.account_id()) {
        return Err(
            Problem::new(ErrorCode::PermissionDenied).with_detail("only its uploader completes an upload")
        );
    }
    if attachment.status != Status::Pending {
        tx.commit().await?;
        return Ok(ApiJson(view(attachment, &acting)));
    }
    let stored = storage.size_of(&attachment.object_key).await?;
    if stored != u64::try_from(attachment.size).ok() {
        return Err(Problem::new(ErrorCode::AttachmentNotUploaded));
    }
    repo::mark_uploaded(&mut tx, attachment.id).await?;
    let payload = serde_json::to_value(AttachmentJob {
        attachment_id: attachment.id,
    })
    .map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue(&mut tx, repo::SCAN_JOB, &payload).await?;
    crate::issue_history::record(
        &mut tx,
        issue.id,
        &[crate::issue_history::attachment(
            true,
            attachment.id,
            &attachment.name,
        )],
    )
    .await?;
    let actor = tx.account_id();
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "attachment.added",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: serde_json::json!({
                "attachmentId": attachment.id,
                "key": issue.key,
                "projectId": issue.project_id,
                "name": attachment.name,
                "size": attachment.size,
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, attachment = %attachment.id, "file attached");
    let updated = attachment_of(&mut tx, &issue, attachment.id, false).await?;
    tx.commit().await?;
    Ok(ApiJson(view(updated, &acting)))
}

/// `GET /api/v1/issues/{idOrKey}/attachments/{attachmentId}/content`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/attachments/{attachmentId}/content",
    tag = TAG,
    operation_id = "downloadIssueAttachment",
    summary = "Download an attached file",
    description = "Whoever sees its project, once the antivirus found it clean: redirects (303) \
                   to the file in the storage, always served as a download (`Content-Disposition: \
                   attachment`), for 15 minutes.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("attachmentId" = Uuid, Path, description = "Id of the attachment"),
    ),
    responses(
        (status = 303, description = "To the file", headers(("Location" = String, description = "A presigned URL of the file"))),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member, or attachment of it", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`attachment.not_scanned`: not scanned yet, or refused by the antivirus", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 503, description = "`storage.unavailable`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn content(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id_or_key, attachment_id)): ApiPath<(String, Uuid)>,
) -> Result<Response, Problem> {
    let storage = storage_of(&state)?;
    let (issue, _) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let attachment = attachment_of(&mut tx, &issue, attachment_id, false).await?;
    if !attachment.status.downloadable() {
        return Err(Problem::new(ErrorCode::AttachmentNotScanned));
    }
    let url = storage.download_url(
        &browser_host(&state, &tx, &headers),
        &attachment.object_key,
        attachment
            .served_type
            .as_deref()
            .unwrap_or("application/octet-stream"),
        &infra::storage::content_disposition("attachment", &attachment.name),
    )?;
    tx.commit().await?;
    redirect(&url)
}

/// `GET /api/v1/issues/{idOrKey}/attachments/{attachmentId}/thumbnail`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/attachments/{attachmentId}/thumbnail",
    tag = TAG,
    operation_id = "readIssueAttachmentThumbnail",
    summary = "The thumbnail of an attached image",
    description = "Whoever sees its project, once clean: redirects (303) to a PNG of at most \
                   320 × 320 pixels made by the worker.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("attachmentId" = Uuid, Path, description = "Id of the attachment"),
    ),
    responses(
        (status = 303, description = "To the thumbnail", headers(("Location" = String, description = "A presigned URL of the thumbnail"))),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket, attachment, or thumbnail", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 503, description = "`storage.unavailable`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn thumbnail(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id_or_key, attachment_id)): ApiPath<(String, Uuid)>,
) -> Result<Response, Problem> {
    let storage = storage_of(&state)?;
    let (issue, _) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let attachment = attachment_of(&mut tx, &issue, attachment_id, false).await?;
    let Some(key) = attachment
        .thumbnail_key
        .filter(|_| attachment.status.downloadable())
    else {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("no thumbnail"));
    };
    let url = storage.download_url(
        &browser_host(&state, &tx, &headers),
        &key,
        "image/png",
        &infra::storage::content_disposition("inline", "thumbnail.png"),
    )?;
    tx.commit().await?;
    redirect(&url)
}

/// `DELETE /api/v1/issues/{idOrKey}/attachments/{attachmentId}`.
#[utoipa::path(
    delete,
    path = "/issues/{idOrKey}/attachments/{attachmentId}",
    tag = TAG,
    operation_id = "deleteIssueAttachment",
    summary = "Remove a file from a ticket",
    description = "Its uploader while they hold `edit` on the project, or those who administer it \
                   (`administer_node`, tenant admins). Its content leaves the storage unless \
                   another attachment holds the same. Records it in the ticket's history and \
                   writes `attachment.removed` to the event outbox.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("attachmentId" = Uuid, Path, description = "Id of the attachment"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member, or attachment of it", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id_or_key, attachment_id)): ApiPath<(String, Uuid)>,
) -> Result<StatusCode, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    let attachment = attachment_of(&mut tx, &issue, attachment_id, true).await?;
    if !can_delete(attachment.uploaded_by.map(|id| id.as_u128()), &acting) {
        return Err(Problem::new(ErrorCode::PermissionDenied)
            .with_detail("only its uploader or a moderator removes an attachment"));
    }
    ensure_changeable(&project)?;
    let keys = repo::delete(&mut tx, attachment.id).await?;
    repo::delete_objects_later(&mut tx, &keys).await?;
    let actor = tx.account_id();
    if attachment.status != Status::Pending {
        crate::issue_history::record(
            &mut tx,
            issue.id,
            &[crate::issue_history::attachment(
                false,
                attachment.id,
                &attachment.name,
            )],
        )
        .await?;
        infra::outbox::record(
            &mut tx,
            &infra::outbox::Event {
                kind: "attachment.removed",
                aggregate_type: "issue",
                aggregate_id: issue.id,
                payload: serde_json::json!({
                    "attachmentId": attachment.id,
                    "key": issue.key,
                    "projectId": issue.project_id,
                    "name": attachment.name,
                }),
                actor_id: Some(actor),
            },
        )
        .await?;
    }
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, attachment = %attachment.id, "attachment removed");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
