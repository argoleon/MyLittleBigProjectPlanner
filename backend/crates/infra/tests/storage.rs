//! Integration tests against a real S3-compatible storage (SeaweedFS) and a
//! real ClamAV: presigned uploads and downloads on the application's
//! origin, the size the storage enforces, downloads served as attachments,
//! a tenant's files removed by prefix; the antivirus telling EICAR from a
//! clean file.

#![allow(clippy::unwrap_used)]

use infra::antivirus::{self, Verdict};
use infra::id::new_id;
use infra::storage::{content_disposition, upload_key};

/// The public address presigned URLs are signed for: the reverse proxy
/// keeps the browser's Host when it forwards `/<bucket>/` to the storage.
const PUBLIC: &str = "https://planner.example.test";

/// Sends a request for the presigned `url` (signed for [`PUBLIC`]) to the
/// storage itself, with the Host the browser would send.
fn through_proxy(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &url::Url,
) -> reqwest::RequestBuilder {
    let endpoint = std::env::var("TEST_S3_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:58333".to_owned());
    let target = format!("{endpoint}{}?{}", url.path(), url.query().unwrap_or_default());
    client
        .request(method, target)
        .header("host", url.host_str().unwrap())
}

#[tokio::test]
async fn files_go_up_and_down_through_presigned_urls_of_the_public_origin() {
    let storage = testkit::storage().await;
    storage.ensure_bucket().await.unwrap();
    let client = reqwest::Client::new();
    let tenant = new_id();
    let key = upload_key(tenant, new_id());
    let content = b"Recette OIDC: OK".to_vec();

    let url = storage.upload_url(PUBLIC, &key, content.len() as u64).unwrap();
    assert!(
        url.as_str()
            .starts_with(&format!("{PUBLIC}/{}/{tenant}/uploads/", testkit::TEST_BUCKET))
    );
    // Another size than the one signed: refused.
    let refused = through_proxy(&client, reqwest::Method::PUT, &url)
        .body(b"more than announced".to_vec())
        .send()
        .await
        .unwrap();
    assert!(refused.status().is_client_error(), "{}", refused.status());
    assert_eq!(storage.size_of(&key).await.unwrap(), None);
    let uploaded = through_proxy(&client, reqwest::Method::PUT, &url)
        .body(content.clone())
        .send()
        .await
        .unwrap();
    assert!(
        uploaded.status().is_success(),
        "{}",
        uploaded.text().await.unwrap()
    );
    assert_eq!(storage.size_of(&key).await.unwrap(), Some(content.len() as u64));

    // Read back as a download, whatever was uploaded.
    let disposition = content_disposition("attachment", "Recette été.txt");
    let url = storage
        .download_url(PUBLIC, &key, "application/octet-stream", &disposition)
        .unwrap();
    let read = through_proxy(&client, reqwest::Method::GET, &url)
        .send()
        .await
        .unwrap();
    assert!(read.status().is_success());
    assert_eq!(read.headers()["content-disposition"], disposition.as_str());
    assert_eq!(read.headers()["content-type"], "application/octet-stream");
    assert_eq!(read.bytes().await.unwrap().as_ref(), content.as_slice());
    // A tampered URL is refused.
    let mut tampered = url.clone();
    tampered.set_path(&url.path().replace("/uploads/", "/thumbnails/"));
    let forbidden = through_proxy(&client, reqwest::Method::GET, &tampered)
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);

    // The services write and read directly; a tenant's files go by prefix.
    let other = format!("{tenant}/thumbnails/x.png");
    storage
        .write(&other, b"png".to_vec().into(), "image/png")
        .await
        .unwrap();
    let mut keys = storage.keys(&format!("{tenant}/")).await.unwrap();
    keys.sort();
    assert_eq!(keys, [other.clone(), key.clone()]);
    let another_tenant = upload_key(new_id(), new_id());
    storage
        .write(&another_tenant, b"kept".to_vec().into(), "text/plain")
        .await
        .unwrap();
    assert_eq!(storage.delete_prefix(&format!("{tenant}/")).await.unwrap(), 2);
    assert!(storage.keys(&format!("{tenant}/")).await.unwrap().is_empty());
    assert_eq!(
        storage.size_of(&another_tenant).await.unwrap(),
        Some(4),
        "other tenants untouched"
    );
    storage.delete(&another_tenant).await.unwrap();
    storage.delete(&another_tenant).await.unwrap();
}

#[tokio::test]
async fn the_antivirus_finds_the_test_virus_and_passes_a_clean_file() {
    let address = testkit::clamav_addr();
    assert_eq!(
        antivirus::scan(&address, &testkit::eicar()).await.unwrap(),
        Verdict::Infected("Eicar-Test-Signature.UNOFFICIAL".to_owned())
    );
    assert_eq!(antivirus::scan(&address, b"Hello").await.unwrap(), Verdict::Clean);
    // Sent in parts, as the scan job streams a file.
    let mut scan = antivirus::Scan::start(&address).await.unwrap();
    let eicar = testkit::eicar();
    scan.send(&eicar[..20]).await.unwrap();
    scan.send(&eicar[20..]).await.unwrap();
    assert!(matches!(scan.finish().await.unwrap(), Verdict::Infected(_)));
}
