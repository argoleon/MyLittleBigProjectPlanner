//! Integration tests on a real PostgreSQL, S3 storage (SeaweedFS) and
//! ClamAV: attachments (L05-US05). Their metadata kept in its tenant, the
//! quota counting each object once; a clean image scanned and given a
//! thumbnail, the same content deduplicated, the test virus found and its
//! content removed, an upload of another size rejected, a scan run twice;
//! objects removed once no attachment refers to them, uploads that never
//! came forgotten.

#![allow(clippy::unwrap_used)]

use domain::attachments::Status;
use domain::hierarchy::NodeType;
use infra::accounts::TenantRole;
use infra::attachments::{self, Attachment, NewAttachment, Scanned};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, Issue, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx::{self, PgConnection};
use infra::storage::{Storage, upload_key};
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};

/// A ticket of a new project of `tenant`, and the admin who made it.
async fn ticket(db: &TestDb, tenant: &SeedTenant) -> (Issue, Uuid) {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key: Some("PLAT"),
            fields: NodeFields {
                name: "PLAT",
                description: "",
                objectives: "",
                start_date: None,
                end_date: None,
                health: domain::hierarchy::Health::OnTrack,
                owners: &[],
            },
            parent: None,
            created_by: admin,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let fields = json!({});
    let issue = issues::create(
        &mut tx,
        &NewIssue {
            id: new_id(),
            project_id: node.id,
            project_key: "PLAT",
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: None,
            summary: "Ticket",
            description: None,
            priority: "medium",
            reporter_id: admin,
            assignee_id: None,
            start_date: None,
            due_date: None,
            fields: &fields,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (issue, admin)
}

/// Announces `content` as `name` on `issue`; with `upload`, uploads it and
/// marks it uploaded, as the API does.
async fn attach(
    conn: &mut PgConnection,
    storage: &Storage,
    tenant: Uuid,
    (issue, by): (&Issue, Uuid),
    name: &str,
    content: &[u8],
    upload: bool,
) -> Attachment {
    let id = new_id();
    let key = upload_key(tenant, id);
    let attachment = attachments::insert(
        conn,
        &NewAttachment {
            id,
            issue_id: issue.id,
            name,
            size: content.len() as i64,
            media_type: "application/octet-stream",
            object_key: &key,
            uploaded_by: by,
        },
    )
    .await
    .unwrap();
    if upload {
        storage
            .write(&key, content.to_vec().into(), "application/octet-stream")
            .await
            .unwrap();
        attachments::mark_uploaded(conn, id).await.unwrap();
    }
    attachment
}

/// A small PNG image.
fn png() -> Vec<u8> {
    let image = image::RgbImage::from_fn(640, 480, |x, y| {
        image::Rgb([
            u8::try_from(x % 256).unwrap(),
            u8::try_from(y % 256).unwrap(),
            128,
        ])
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// The keys queued for removal from the storage, in the current tenant.
async fn queued_deletions(conn: &mut PgConnection) -> Vec<String> {
    let payloads: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload FROM job WHERE kind = 'storage.delete' ORDER BY created_at, id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    payloads
        .into_iter()
        .flat_map(|p| serde_json::from_value::<Vec<String>>(p["keys"].clone()).unwrap())
        .collect()
}

#[tokio::test]
async fn metadata_stays_in_its_tenant_and_the_quota_counts_each_object_once() {
    let db = TestDb::new().await;
    let storage = testkit::storage().await;
    let (issue, admin) = ticket(&db, &db.tenant_a).await;
    let tenant = db.tenant_a.id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let pending = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "plan.pdf",
        b"12345",
        false,
    )
    .await;
    assert_eq!(
        (pending.status, pending.uploader_name.as_deref()),
        (Status::Pending, Some("Test Member"))
    );
    assert!(
        attachments::of_issue(&mut tx, issue.id).await.unwrap().is_empty(),
        "pending ones are left out"
    );
    assert_eq!(
        attachments::used_bytes(&mut tx).await.unwrap(),
        5,
        "a pending upload counts"
    );
    let uploaded = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "notes.txt",
        b"abcde",
        true,
    )
    .await;
    assert_eq!(attachments::used_bytes(&mut tx).await.unwrap(), 10);
    let listed = attachments::of_issue(&mut tx, issue.id).await.unwrap();
    assert_eq!(listed.iter().map(|a| a.id).collect::<Vec<_>>(), [uploaded.id]);
    assert_eq!(listed[0].status, Status::Scanning);
    // Two attachments of one object (the same content) count once.
    sqlx::query("UPDATE attachment SET object_key = $1 WHERE id = $2")
        .bind(&uploaded.object_key)
        .bind(pending.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(attachments::used_bytes(&mut tx).await.unwrap(), 5);
    assert_eq!(
        attachments::quota(&mut tx).await.unwrap(),
        None,
        "unlimited by default"
    );
    tx.commit().await.unwrap();

    // Tenant B sees none of it, and cannot write into tenant A.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(attachments::find(&mut tx, uploaded.id).await.unwrap(), None);
    assert!(attachments::of_issue(&mut tx, issue.id).await.unwrap().is_empty());
    assert_eq!(attachments::used_bytes(&mut tx).await.unwrap(), 0);
    let refused = sqlx::query(
        "INSERT INTO attachment (id, tenant_id, issue_id, name, size, object_key) VALUES ($1, $2, $3, 'x', 1, 'k')",
    )
    .bind(new_id())
    .bind(tenant)
    .bind(issue.id)
    .execute(&mut *tx)
    .await;
    assert!(
        refused.is_err(),
        "row-level security refuses another tenant's rows"
    );
}

#[tokio::test]
async fn a_clean_image_gets_a_thumbnail_and_the_same_content_is_stored_once() {
    let db = TestDb::new().await;
    let storage = testkit::storage().await;
    let clamav = testkit::clamav_addr();
    let (issue, admin) = ticket(&db, &db.tenant_a).await;
    let tenant = db.tenant_a.id;
    let image = png();

    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let first = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "screen.png",
        &image,
        true,
    )
    .await;
    let scanned = attachments::scan(&mut tx, &storage, &clamav, first.id)
        .await
        .unwrap();
    assert_eq!(
        scanned,
        Scanned::Clean {
            deduplicated: false,
            thumbnail: true
        }
    );
    let first = attachments::find(&mut tx, first.id).await.unwrap().unwrap();
    assert_eq!(first.status, Status::Clean);
    assert_eq!(first.served_type.as_deref(), Some("image/png"));
    let expected: String = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&image)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    assert_eq!(first.sha256.as_deref(), Some(expected.as_str()));
    let thumbnail = first.thumbnail_key.clone().unwrap();
    let mut body = storage.read(&thumbnail).await.unwrap().unwrap();
    let mut png_bytes = Vec::new();
    while let Some(frame) = http_body_util::BodyExt::frame(&mut body).await {
        png_bytes.extend_from_slice(&frame.unwrap().into_data().unwrap());
    }
    let small = image::load_from_memory(&png_bytes).unwrap();
    assert_eq!(
        (small.width(), small.height()),
        (320, 240),
        "fits 320 × 320, proportions kept"
    );
    assert_eq!(
        attachments::scan(&mut tx, &storage, &clamav, first.id)
            .await
            .unwrap(),
        Scanned::Skipped,
        "scanned once"
    );

    // The same content again: it points to the first object, its own upload goes.
    let second = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "copy.png",
        &image,
        true,
    )
    .await;
    assert_eq!(
        attachments::scan(&mut tx, &storage, &clamav, second.id)
            .await
            .unwrap(),
        Scanned::Clean {
            deduplicated: true,
            thumbnail: true
        }
    );
    let second_now = attachments::find(&mut tx, second.id).await.unwrap().unwrap();
    assert_eq!(
        (
            second_now.object_key.as_str(),
            second_now.thumbnail_key.as_deref()
        ),
        (first.object_key.as_str(), Some(thumbnail.as_str()))
    );
    assert_eq!(
        queued_deletions(&mut tx).await,
        std::slice::from_ref(&second.object_key)
    );
    assert_eq!(
        attachments::delete_objects(&mut tx, &storage, std::slice::from_ref(&second.object_key))
            .await
            .unwrap(),
        1
    );
    let mut objects = storage.keys(&format!("{tenant}/")).await.unwrap();
    objects.sort();
    let mut expected_objects = vec![first.object_key.clone(), thumbnail.clone()];
    expected_objects.sort();
    assert_eq!(objects, expected_objects, "one object for the two attachments");

    // Removing the first keeps the shared object; removing the last frees it.
    assert!(attachments::delete(&mut tx, first.id).await.unwrap().is_empty());
    let keys = attachments::delete(&mut tx, second.id).await.unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(
        attachments::delete_objects(&mut tx, &storage, &keys)
            .await
            .unwrap(),
        2
    );
    assert!(storage.keys(&format!("{tenant}/")).await.unwrap().is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn the_test_virus_is_found_and_files_of_another_size_are_rejected() {
    let db = TestDb::new().await;
    let storage = testkit::storage().await;
    let clamav = testkit::clamav_addr();
    let (issue, admin) = ticket(&db, &db.tenant_a).await;
    let tenant = db.tenant_a.id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();

    let virus = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "invoice.exe",
        &testkit::eicar(),
        true,
    )
    .await;
    assert_eq!(
        attachments::scan(&mut tx, &storage, &clamav, virus.id)
            .await
            .unwrap(),
        Scanned::Infected("Eicar-Test-Signature.UNOFFICIAL".to_owned())
    );
    let infected = attachments::find(&mut tx, virus.id).await.unwrap().unwrap();
    assert_eq!(infected.status, Status::Infected);
    assert_eq!(
        infected.signature.as_deref(),
        Some("Eicar-Test-Signature.UNOFFICIAL")
    );
    assert_eq!(infected.sha256, None);
    assert!(!infected.status.downloadable());

    // Uploaded larger, then smaller than announced; then never uploaded.
    let larger = attach(&mut tx, &storage, tenant, (&issue, admin), "a.txt", b"abc", true).await;
    storage
        .write(&larger.object_key, b"abcdef".to_vec().into(), "text/plain")
        .await
        .unwrap();
    assert_eq!(
        attachments::scan(&mut tx, &storage, &clamav, larger.id)
            .await
            .unwrap(),
        Scanned::Rejected
    );
    let smaller = attach(&mut tx, &storage, tenant, (&issue, admin), "b.txt", b"abc", true).await;
    storage
        .write(&smaller.object_key, b"ab".to_vec().into(), "text/plain")
        .await
        .unwrap();
    assert_eq!(
        attachments::scan(&mut tx, &storage, &clamav, smaller.id)
            .await
            .unwrap(),
        Scanned::Rejected
    );
    let missing = attach(&mut tx, &storage, tenant, (&issue, admin), "c.txt", b"abc", false).await;
    attachments::mark_uploaded(&mut tx, missing.id).await.unwrap();
    assert_eq!(
        attachments::scan(&mut tx, &storage, &clamav, missing.id)
            .await
            .unwrap(),
        Scanned::Rejected
    );
    assert_eq!(
        attachments::find(&mut tx, larger.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Rejected
    );

    // Their content is queued for removal, and removed.
    let queued = queued_deletions(&mut tx).await;
    assert_eq!(queued.len(), 4);
    attachments::delete_objects(&mut tx, &storage, &queued)
        .await
        .unwrap();
    assert!(storage.keys(&format!("{tenant}/")).await.unwrap().is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_upload_that_never_came_is_forgotten() {
    let db = TestDb::new().await;
    let storage = testkit::storage().await;
    let (issue, admin) = ticket(&db, &db.tenant_a).await;
    let tenant = db.tenant_a.id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let never = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "late.pdf",
        b"x",
        false,
    )
    .await;
    let came = attach(
        &mut tx,
        &storage,
        tenant,
        (&issue, admin),
        "on-time.pdf",
        b"x",
        true,
    )
    .await;
    assert!(attachments::expire(&mut tx, never.id).await.unwrap());
    assert!(
        !attachments::expire(&mut tx, came.id).await.unwrap(),
        "uploaded: kept"
    );
    assert!(
        !attachments::expire(&mut tx, never.id).await.unwrap(),
        "already forgotten"
    );
    assert_eq!(attachments::find(&mut tx, never.id).await.unwrap(), None);
    assert_eq!(queued_deletions(&mut tx).await, [never.object_key]);
}
