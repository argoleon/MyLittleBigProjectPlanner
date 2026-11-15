//! The S3-compatible storage of attachments (specification §12, L05-US05):
//! SeaweedFS in development and in the compose stack, any S3 service
//! otherwise.
//!
//! Browsers upload and download directly, through presigned URLs: files
//! never go through the API. Those URLs are signed for the tenant's public
//! address (`PUBLIC_URL`) in path style, `https://<tenant host>/<bucket>/<key>`,
//! and the reverse proxy routes `/<bucket>/` to the storage, so the browser
//! stays on the application's origin (no CORS, the CSP unchanged). The
//! services themselves reach the storage at `S3_ENDPOINT`, plain HTTP inside
//! the backend network.
//!
//! Every object of a tenant sits under its id: `<tenant>/uploads/<id>`,
//! `<tenant>/thumbnails/<id>.png`, so a tenant's files are removed with its
//! prefix.

use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_LENGTH, CONTENT_TYPE};
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rusty_s3::actions::ListObjectsV2;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use uuid::Uuid;

use crate::config::StorageConfig;

/// How long a presigned URL works.
pub const URL_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// How long the storage may take to start answering a request.
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("invalid storage address: {0}")]
    Address(String),
    #[error("storage request failed: {0}")]
    Request(String),
    #[error("the storage did not answer within {} seconds", TIMEOUT.as_secs())]
    Timeout,
    #[error("the storage answered HTTP {status}: {body}")]
    Status { status: u16, body: String },
}

/// The key of an uploaded file of `tenant`.
pub fn upload_key(tenant: Uuid, attachment: Uuid) -> String {
    format!("{tenant}/uploads/{attachment}")
}

/// The key of the thumbnail of an attachment of `tenant`.
pub fn thumbnail_key(tenant: Uuid, attachment: Uuid) -> String {
    format!("{tenant}/thumbnails/{attachment}.png")
}

/// A `Content-Disposition` naming `filename`, for any browser: an ASCII
/// fallback, and the exact name (RFC 6266, RFC 5987).
pub fn content_disposition(kind: &str, filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if (c.is_ascii_graphic() && c != '"' && c != '\\') || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in filename.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// The storage of attachments.
#[derive(Clone)]
pub struct Storage {
    /// The bucket as the services reach it.
    bucket: Bucket,
    credentials: Credentials,
    client: Client<HttpConnector, Full<Bytes>>,
    max_attachment_bytes: u64,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("bucket", &self.bucket.name())
            .finish_non_exhaustive()
    }
}

impl Storage {
    pub fn new(config: &StorageConfig) -> Result<Self, StorageError> {
        let bucket = Bucket::new(
            config.endpoint.clone(),
            UrlStyle::Path,
            config.bucket.clone(),
            config.region.clone(),
        )
        .map_err(|e| StorageError::Address(e.to_string()))?;
        Ok(Self {
            bucket,
            credentials: Credentials::new(config.access_key.expose(), config.secret_key.expose()),
            client: Client::builder(TokioExecutor::new()).build_http(),
            max_attachment_bytes: config.max_attachment_bytes,
        })
    }

    /// The largest attachment accepted.
    pub const fn max_attachment_bytes(&self) -> u64 {
        self.max_attachment_bytes
    }

    pub fn bucket_name(&self) -> &str {
        self.bucket.name()
    }

    /// The bucket as browsers of `public_origin` reach it.
    fn public_bucket(&self, public_origin: &str) -> Result<Bucket, StorageError> {
        let origin = url::Url::parse(public_origin).map_err(|e| StorageError::Address(e.to_string()))?;
        Bucket::new(
            origin,
            UrlStyle::Path,
            self.bucket.name().to_owned(),
            self.bucket.region().to_owned(),
        )
        .map_err(|e| StorageError::Address(e.to_string()))
    }

    /// A URL to upload `size` bytes to `key` with `PUT`, for the browser of
    /// `public_origin`: the storage refuses any other size.
    pub fn upload_url(&self, public_origin: &str, key: &str, size: u64) -> Result<url::Url, StorageError> {
        let bucket = self.public_bucket(public_origin)?;
        let mut action = bucket.put_object(Some(&self.credentials), key);
        action.headers_mut().insert("content-length", size.to_string());
        Ok(action.sign(URL_LIFETIME))
    }

    /// A URL to read `key` for the browser of `public_origin`, served with
    /// `content_type` and `disposition` whatever was uploaded.
    pub fn download_url(
        &self,
        public_origin: &str,
        key: &str,
        content_type: &str,
        disposition: &str,
    ) -> Result<url::Url, StorageError> {
        let bucket = self.public_bucket(public_origin)?;
        let mut action = bucket.get_object(Some(&self.credentials), key);
        action
            .query_mut()
            .insert("response-content-type", content_type.to_owned());
        action
            .query_mut()
            .insert("response-content-disposition", disposition.to_owned());
        Ok(action.sign(URL_LIFETIME))
    }

    async fn send(&self, request: Request<Full<Bytes>>) -> Result<hyper::Response<Incoming>, StorageError> {
        tokio::time::timeout(TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| StorageError::Timeout)?
            .map_err(|e| StorageError::Request(e.to_string()))
    }

    fn request(method: Method, url: &url::Url, body: Bytes) -> Result<Request<Full<Bytes>>, StorageError> {
        Request::builder()
            .method(method)
            .uri(url.as_str())
            .header(CONTENT_LENGTH, body.len())
            .body(Full::new(body))
            .map_err(|e| StorageError::Request(e.to_string()))
    }

    async fn failed(response: hyper::Response<Incoming>) -> StorageError {
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .collect()
            .await
            .map(|b| String::from_utf8_lossy(&b.to_bytes()).chars().take(300).collect())
            .unwrap_or_default();
        StorageError::Status { status, body }
    }

    /// Creates the bucket unless it exists. The services call it at start.
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        let head = self
            .bucket
            .head_bucket(Some(&self.credentials))
            .sign(URL_LIFETIME);
        let response = self
            .send(Self::request(Method::HEAD, &head, Bytes::new())?)
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        let create = self.bucket.create_bucket(&self.credentials).sign(URL_LIFETIME);
        let response = self
            .send(Self::request(Method::PUT, &create, Bytes::new())?)
            .await?;
        match response.status() {
            status if status.is_success() => Ok(()),
            // Created meanwhile by another service.
            StatusCode::CONFLICT => Ok(()),
            _ => Err(Self::failed(response).await),
        }
    }

    /// The size of `key`, if it exists.
    pub async fn size_of(&self, key: &str) -> Result<Option<u64>, StorageError> {
        let url = self
            .bucket
            .head_object(Some(&self.credentials), key)
            .sign(URL_LIFETIME);
        let response = self
            .send(Self::request(Method::HEAD, &url, Bytes::new())?)
            .await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())),
            _ => Err(Self::failed(response).await),
        }
    }

    /// The content of `key` as a stream, if it exists.
    pub async fn read(&self, key: &str) -> Result<Option<Incoming>, StorageError> {
        let url = self
            .bucket
            .get_object(Some(&self.credentials), key)
            .sign(URL_LIFETIME);
        let response = self.send(Self::request(Method::GET, &url, Bytes::new())?).await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(response.into_body())),
            _ => Err(Self::failed(response).await),
        }
    }

    /// Stores `content` at `key`.
    pub async fn write(&self, key: &str, content: Bytes, content_type: &str) -> Result<(), StorageError> {
        let url = self
            .bucket
            .put_object(Some(&self.credentials), key)
            .sign(URL_LIFETIME);
        let mut request = Self::request(Method::PUT, &url, content)?;
        request.headers_mut().insert(
            CONTENT_TYPE,
            content_type
                .parse()
                .map_err(|_| StorageError::Request("invalid content type".to_owned()))?,
        );
        let response = self.send(request).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Self::failed(response).await)
        }
    }

    /// Removes `key`; nothing if it does not exist.
    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let url = self
            .bucket
            .delete_object(Some(&self.credentials), key)
            .sign(URL_LIFETIME);
        let response = self
            .send(Self::request(Method::DELETE, &url, Bytes::new())?)
            .await?;
        match response.status() {
            status if status.is_success() || status == StatusCode::NOT_FOUND => Ok(()),
            _ => Err(Self::failed(response).await),
        }
    }

    /// The keys under `prefix`.
    pub async fn keys(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut action: ListObjectsV2<'_> = self.bucket.list_objects_v2(Some(&self.credentials));
            action.with_prefix(prefix);
            if let Some(token) = &token {
                action.with_continuation_token(token.clone());
            }
            let url = action.sign(URL_LIFETIME);
            let response = self.send(Self::request(Method::GET, &url, Bytes::new())?).await?;
            if !response.status().is_success() {
                return Err(Self::failed(response).await);
            }
            let body = response
                .into_body()
                .collect()
                .await
                .map_err(|e| StorageError::Request(e.to_string()))?
                .to_bytes();
            let listed = ListObjectsV2::parse_response(&String::from_utf8_lossy(&body))
                .map_err(|e| StorageError::Request(e.to_string()))?;
            keys.extend(listed.contents.into_iter().map(|c| c.key));
            match listed.next_continuation_token {
                Some(next) => token = Some(next),
                None => return Ok(keys),
            }
        }
    }

    /// Removes every object under `prefix` (a tenant's); how many.
    pub async fn delete_prefix(&self, prefix: &str) -> Result<usize, StorageError> {
        let keys = self.keys(prefix).await?;
        for key in &keys {
            self.delete(key).await?;
        }
        Ok(keys.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sit_under_their_tenant() {
        let (tenant, file) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            upload_key(tenant, file),
            "00000000-0000-0000-0000-000000000001/uploads/00000000-0000-0000-0000-000000000002"
        );
        assert!(thumbnail_key(tenant, file).starts_with(&format!("{tenant}/thumbnails/")));
    }

    #[test]
    fn a_disposition_names_any_file_safely() {
        assert_eq!(
            content_disposition("attachment", "Plan v2.pdf"),
            "attachment; filename=\"Plan v2.pdf\"; filename*=UTF-8''Plan%20v2.pdf"
        );
        assert_eq!(
            content_disposition("attachment", "été \"x\".txt"),
            "attachment; filename=\"_t_ _x_.txt\"; filename*=UTF-8''%C3%A9t%C3%A9%20%22x%22.txt"
        );
    }
}
