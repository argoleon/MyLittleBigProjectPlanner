//! Attachments of tickets (`attachment`, L05-US05): what describes the
//! files kept in the storage (`crate::storage`), and the jobs that scan
//! them and remove their content.

use domain::attachments::Status;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

/// Job scanning an uploaded file (antivirus, SHA-256, thumbnail).
pub const SCAN_JOB: &str = "attachment.scan";
/// Job removing an attachment whose upload never came.
pub const EXPIRE_JOB: &str = "attachment.expire";
/// Job removing objects no attachment refers to any more.
pub const DELETE_OBJECTS_JOB: &str = "storage.delete";

/// How long the browser has to upload a file before it is forgotten.
pub const UPLOAD_WINDOW: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Payload of [`SCAN_JOB`] and [`EXPIRE_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentJob {
    pub attachment_id: Uuid,
}

/// Payload of [`DELETE_OBJECTS_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteObjects {
    pub keys: Vec<String>,
}

/// An attachment as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub name: String,
    pub size: i64,
    pub media_type: String,
    pub status: Status,
    pub sha256: Option<String>,
    pub object_key: String,
    pub thumbnail_key: Option<String>,
    pub served_type: Option<String>,
    pub signature: Option<String>,
    pub uploaded_by: Option<Uuid>,
    /// "Deleted user #N" once erased, none when no longer a member.
    pub uploader_name: Option<String>,
    pub scanned_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub version: i64,
}

struct Row {
    id: Uuid,
    issue_id: Uuid,
    name: String,
    size: i64,
    media_type: String,
    status: String,
    sha256: Option<String>,
    object_key: String,
    thumbnail_key: Option<String>,
    served_type: Option<String>,
    signature: Option<String>,
    uploaded_by: Option<Uuid>,
    uploader_name: Option<String>,
    scanned_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    version: i64,
}

impl From<Row> for Attachment {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            issue_id: r.issue_id,
            name: r.name,
            size: r.size,
            media_type: r.media_type,
            // The table only holds known statuses.
            status: Status::parse(&r.status).unwrap_or(Status::Rejected),
            sha256: r.sha256,
            object_key: r.object_key,
            thumbnail_key: r.thumbnail_key,
            served_type: r.served_type,
            signature: r.signature,
            uploaded_by: r.uploaded_by,
            uploader_name: r.uploader_name,
            scanned_at: r.scanned_at,
            created_at: r.created_at,
            version: r.version,
        }
    }
}

/// An attachment to record, before its upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment<'a> {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub name: &'a str,
    pub size: i64,
    pub media_type: &'a str,
    pub object_key: &'a str,
    pub uploaded_by: Uuid,
}

/// Records `new` in the current tenant, pending its upload; it as stored.
pub async fn insert(conn: &mut PgConnection, new: &NewAttachment<'_>) -> Result<Attachment, sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO attachment (id, tenant_id, issue_id, name, size, media_type, object_key, uploaded_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7)
        "#,
        new.id,
        new.issue_id,
        new.name,
        new.size,
        new.media_type,
        new.object_key,
        new.uploaded_by
    )
    .execute(&mut *conn)
    .await?;
    find(conn, new.id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// The attachment `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Attachment>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, issue_id, name, size, media_type, status, sha256, object_key, thumbnail_key, served_type,
               signature, uploaded_by, app_member_name(uploaded_by) AS "uploader_name?", scanned_at, created_at,
               version
        FROM attachment WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Attachment::from))
}

/// The attachments of the ticket `issue` that were uploaded (pending ones
/// left out), oldest first.
pub async fn of_issue(conn: &mut PgConnection, issue: Uuid) -> Result<Vec<Attachment>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, issue_id, name, size, media_type, status, sha256, object_key, thumbnail_key, served_type,
               signature, uploaded_by, app_member_name(uploaded_by) AS "uploader_name?", scanned_at, created_at,
               version
        FROM attachment
        WHERE issue_id = $1 AND status <> 'pending'
        ORDER BY created_at, id
        "#,
        issue
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Attachment::from).collect())
}

/// Locks the attachment `id` until the transaction ends; it.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<Attachment>, sqlx::Error> {
    let locked = sqlx::query_scalar!("SELECT id FROM attachment WHERE id = $1 FOR UPDATE", id)
        .fetch_optional(&mut *conn)
        .await?;
    match locked {
        Some(id) => find(conn, id).await,
        None => Ok(None),
    }
}

/// The bytes the current tenant's files occupy: each object once, of the
/// attachments that hold content (pending uploads included).
pub async fn used_bytes(conn: &mut PgConnection) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT coalesce(sum(size), 0)::bigint AS "used!"
        FROM (SELECT DISTINCT ON (object_key) size FROM attachment
              WHERE status IN ('pending', 'scanning', 'clean')
              ORDER BY object_key) AS objects
        "#
    )
    .fetch_one(conn)
    .await
}

/// The storage quota of the current tenant, in bytes; none: unlimited.
pub async fn quota(conn: &mut PgConnection) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT storage_quota_bytes FROM tenant WHERE id = app_current_tenant()"#)
        .fetch_optional(conn)
        .await
        .map(Option::flatten)
}

/// Marks the attachment `id` uploaded: to be scanned.
pub async fn mark_uploaded(conn: &mut PgConnection, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE attachment SET status = 'scanning', version = version + 1 WHERE id = $1 AND status = 'pending'",
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// What a clean content already stored is: its object, thumbnail and type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredContent {
    pub object_key: String,
    pub thumbnail_key: Option<String>,
    pub served_type: String,
}

/// The first clean object of the content `sha256` in the current tenant,
/// besides the attachment `except`. Serialized per content: two scans of
/// the same content never both keep their object.
pub async fn stored_content(
    conn: &mut PgConnection,
    sha256: &str,
    except: Uuid,
) -> Result<Option<StoredContent>, sqlx::Error> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended(app_current_tenant()::text || $1, 0))",
        sha256
    )
    .execute(&mut *conn)
    .await?;
    let row = sqlx::query!(
        r#"
        SELECT object_key, thumbnail_key, served_type AS "served_type!"
        FROM attachment
        WHERE sha256 = $1 AND status = 'clean' AND id <> $2
        ORDER BY created_at, id
        LIMIT 1
        "#,
        sha256,
        except
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| StoredContent {
        object_key: r.object_key,
        thumbnail_key: r.thumbnail_key,
        served_type: r.served_type,
    }))
}

/// Marks the attachment `id` clean, its content being `content`.
pub async fn mark_clean(
    conn: &mut PgConnection,
    id: Uuid,
    sha256: &str,
    content: &StoredContent,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE attachment
        SET status = 'clean', sha256 = $2, object_key = $3, thumbnail_key = $4, served_type = $5,
            scanned_at = now(), version = version + 1
        WHERE id = $1
        "#,
        id,
        sha256,
        content.object_key,
        content.thumbnail_key,
        content.served_type
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Marks the attachment `id` infected (`signature`) or rejected (none).
pub async fn mark_refused(
    conn: &mut PgConnection,
    id: Uuid,
    signature: Option<&str>,
) -> Result<(), sqlx::Error> {
    let status = if signature.is_some() {
        Status::Infected
    } else {
        Status::Rejected
    };
    sqlx::query!(
        r#"
        UPDATE attachment SET status = $2, signature = $3, scanned_at = now(), version = version + 1
        WHERE id = $1
        "#,
        id,
        status.as_str(),
        signature
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the attachment `id`; the keys of its objects that no other
/// attachment refers to, to delete from the storage.
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<Vec<String>, sqlx::Error> {
    let removed = sqlx::query!(
        "DELETE FROM attachment WHERE id = $1 RETURNING object_key, thumbnail_key",
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(removed) = removed else {
        return Ok(Vec::new());
    };
    let keys: Vec<String> = std::iter::once(removed.object_key)
        .chain(removed.thumbnail_key)
        .collect();
    unreferenced(conn, &keys).await
}

/// Those of `keys` no attachment holding content (pending, being scanned or
/// clean) refers to: an infected or rejected one's content is gone.
pub async fn unreferenced(conn: &mut PgConnection, keys: &[String]) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT k AS "key!" FROM unnest($1::text[]) AS k
        WHERE NOT EXISTS (SELECT 1 FROM attachment a
                          WHERE (a.object_key = k OR a.thumbnail_key = k)
                            AND a.status IN ('pending', 'scanning', 'clean'))
        "#,
        keys
    )
    .fetch_all(conn)
    .await
}

/// Queues the deletion of `keys` from the storage, in the current
/// transaction: they go once it commits.
pub async fn delete_objects_later(conn: &mut PgConnection, keys: &[String]) -> Result<(), sqlx::Error> {
    if keys.is_empty() {
        return Ok(());
    }
    let payload = serde_json::to_value(DeleteObjects { keys: keys.to_vec() })
        .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    crate::jobs::enqueue(conn, DELETE_OBJECTS_JOB, &payload).await?;
    Ok(())
}

// --- Scanning ----------------------------------------------------------------

/// What came of scanning an attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scanned {
    /// Clean; `deduplicated` when its content was already stored.
    Clean { deduplicated: bool, thumbnail: bool },
    /// The signature the antivirus found.
    Infected(String),
    /// Missing, or not the size announced.
    Rejected,
    /// Not waiting for a scan (scanned already, removed): nothing done.
    Skipped,
}

/// Why a scan could not be done; worth another attempt.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Storage(#[from] crate::storage::StorageError),
    #[error(transparent)]
    Antivirus(#[from] crate::antivirus::AntivirusError),
    #[error("cannot read the uploaded file: {0}")]
    Read(String),
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A PNG thumbnail of the image `bytes` of `kind`, fitting
/// `THUMBNAIL_SIZE`; none if it cannot be read within the limits.
pub fn thumbnail(bytes: &[u8], kind: domain::attachments::ImageKind) -> Option<Vec<u8>> {
    use domain::attachments::{ImageKind, THUMBNAIL_SIZE, THUMBNAIL_SOURCE_MAX_PIXELS};
    use image::{ImageFormat, ImageReader, Limits};
    let format = match kind {
        ImageKind::Png => ImageFormat::Png,
        ImageKind::Jpeg => ImageFormat::Jpeg,
        ImageKind::Gif => ImageFormat::Gif,
        ImageKind::WebP => ImageFormat::WebP,
    };
    let mut limits = Limits::default();
    limits.max_image_width = Some(THUMBNAIL_SOURCE_MAX_PIXELS);
    limits.max_image_height = Some(THUMBNAIL_SOURCE_MAX_PIXELS);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let small = image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE);
    let mut png = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// Records the attachment `id` infected (`signature`) or rejected, and
/// queues the removal of its upload `own`.
async fn refuse(
    conn: &mut PgConnection,
    id: Uuid,
    own: &str,
    signature: Option<String>,
) -> Result<Scanned, ScanError> {
    mark_refused(conn, id, signature.as_deref()).await?;
    delete_objects_later(conn, &[own.to_owned()]).await?;
    Ok(signature.map_or(Scanned::Rejected, Scanned::Infected))
}

/// Scans the attachment `id` of the current tenant: streams its upload
/// from `storage` to the antivirus at `clamav`, computing its SHA-256 and
/// size; then keeps it clean (deduplicated, with a thumbnail if it is an
/// image), or records it infected or rejected and queues the removal of its
/// content. Runs in the job's transaction; idempotent.
pub async fn scan(
    conn: &mut PgConnection,
    storage: &crate::storage::Storage,
    clamav: &str,
    id: Uuid,
) -> Result<Scanned, ScanError> {
    use domain::attachments::{ImageKind, THUMBNAIL_SOURCE_MAX_BYTES, served_type};
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};

    let Some(attachment) = lock(conn, id).await? else {
        return Ok(Scanned::Skipped);
    };
    if attachment.status != Status::Scanning {
        return Ok(Scanned::Skipped);
    }
    let own = attachment.object_key.clone();
    let declared = u64::try_from(attachment.size).unwrap_or(0);
    let Some(mut body) = storage.read(&attachment.object_key).await? else {
        return refuse(conn, id, &own, None).await;
    };

    let mut antivirus = crate::antivirus::Scan::start(clamav).await?;
    let mut hasher = Sha256::new();
    let mut read: u64 = 0;
    // The first bytes tell an image; small images are kept for their thumbnail.
    let mut kept: Vec<u8> = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| ScanError::Read(e.to_string()))?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        read += chunk.len() as u64;
        if read > declared {
            // Larger than announced: not read further.
            return refuse(conn, id, &own, None).await;
        }
        hasher.update(&chunk);
        antivirus.send(&chunk).await?;
        if read <= THUMBNAIL_SOURCE_MAX_BYTES {
            kept.extend_from_slice(&chunk);
        } else if kept.len() > 16 {
            kept.truncate(16);
        }
    }
    let verdict = antivirus.finish().await?;
    if read != declared {
        return refuse(conn, id, &own, None).await;
    }
    if let crate::antivirus::Verdict::Infected(signature) = verdict {
        return refuse(conn, id, &own, Some(signature)).await;
    }

    let sha256 = hex(&hasher.finalize());
    let image = ImageKind::sniff(&kept);
    if let Some(stored) = stored_content(conn, &sha256, id).await? {
        mark_clean(conn, id, &sha256, &stored).await?;
        if stored.object_key != own {
            delete_objects_later(conn, std::slice::from_ref(&own)).await?;
        }
        return Ok(Scanned::Clean {
            deduplicated: true,
            thumbnail: stored.thumbnail_key.is_some(),
        });
    }
    let mut thumbnail_key = None;
    if let Some(kind) = image
        && read <= THUMBNAIL_SOURCE_MAX_BYTES
    {
        let made = tokio::task::spawn_blocking(move || thumbnail(&kept, kind))
            .await
            .ok()
            .flatten();
        if let Some(png) = made {
            let tenant: Uuid = sqlx::query_scalar!(r#"SELECT app_current_tenant() AS "tenant!""#)
                .fetch_one(&mut *conn)
                .await?;
            let key = crate::storage::thumbnail_key(tenant, id);
            storage.write(&key, png.into(), "image/png").await?;
            thumbnail_key = Some(key);
        }
    }
    let made = thumbnail_key.is_some();
    mark_clean(
        conn,
        id,
        &sha256,
        &StoredContent {
            object_key: own,
            thumbnail_key,
            served_type: served_type(image).to_owned(),
        },
    )
    .await?;
    Ok(Scanned::Clean {
        deduplicated: false,
        thumbnail: made,
    })
}

/// Forgets the attachment `id` if its upload never came; whether it did.
pub async fn expire(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    let Some(attachment) = lock(conn, id).await? else {
        return Ok(false);
    };
    if attachment.status != Status::Pending {
        return Ok(false);
    }
    let keys = delete(conn, id).await?;
    delete_objects_later(conn, &keys).await?;
    Ok(true)
}

/// Removes from `storage` those of `keys` no attachment of the current
/// tenant refers to (again) by now; how many.
pub async fn delete_objects(
    conn: &mut PgConnection,
    storage: &crate::storage::Storage,
    keys: &[String],
) -> Result<usize, ScanError> {
    let unused = unreferenced(conn, keys).await?;
    for key in &unused {
        storage.delete(key).await?;
    }
    Ok(unused.len())
}
