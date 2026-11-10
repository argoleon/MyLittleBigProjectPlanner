//! Outgoing HTTPS for the server (OIDC discovery, keys and tokens).
//!
//! rustls with ring and the system's certificates. Redirects are never
//! followed, answers are bounded in time and size, and plain HTTP is refused
//! unless the configuration allows it (development and tests only): the URLs
//! come from tenant admins.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE};
use hyper::{Method, Request, StatusCode, Uri};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde::de::DeserializeOwned;

/// Longest wait for an answer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Largest answer read.
const MAX_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("cannot set up TLS: {0}")]
    Tls(String),
    #[error("invalid URL {0}")]
    Url(String),
    #[error("plain HTTP is not allowed: {0}")]
    PlainHttp(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("no answer within {} seconds", TIMEOUT.as_secs())]
    Timeout,
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("unexpected answer: {0}")]
    Body(String),
}

/// A client of external HTTPS services.
#[derive(Clone)]
pub struct HttpClient {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    allow_http: bool,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("allow_http", &self.allow_http)
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    /// `allow_http`: also plain `http://` URLs (development and tests).
    pub fn new(allow_http: bool) -> Result<Self, HttpError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(provider)
            .map_err(|e| HttpError::Tls(e.to_string()))?
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            allow_http,
        })
    }

    fn uri(&self, url: &str) -> Result<Uri, HttpError> {
        let uri: Uri = url.parse().map_err(|_| HttpError::Url(url.to_owned()))?;
        match uri.scheme_str() {
            Some("https") => Ok(uri),
            Some("http") if self.allow_http => Ok(uri),
            Some("http") => Err(HttpError::PlainHttp(url.to_owned())),
            _ => Err(HttpError::Url(url.to_owned())),
        }
    }

    /// `GET url`, its JSON answer.
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, HttpError> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(self.uri(url)?)
            .header(ACCEPT, "application/json")
            .body(Full::new(Bytes::new()))
            .map_err(|e| HttpError::Request(e.to_string()))?;
        self.send(request).await
    }

    /// `POST url` with a form body, its JSON answer.
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, HttpError> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let request = Request::builder()
            .method(Method::POST)
            .uri(self.uri(url)?)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| HttpError::Request(e.to_string()))?;
        self.send(request).await
    }

    async fn send<T: DeserializeOwned>(&self, request: Request<Full<Bytes>>) -> Result<T, HttpError> {
        let response = tokio::time::timeout(TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| HttpError::Timeout)?
            .map_err(|e| HttpError::Request(e.to_string()))?;
        let status = response.status();
        let body = tokio::time::timeout(
            TIMEOUT,
            Limited::new(response.into_body(), MAX_BODY_BYTES).collect(),
        )
        .await
        .map_err(|_| HttpError::Timeout)?
        .map_err(|e| HttpError::Body(e.to_string()))?
        .to_bytes();
        if status != StatusCode::OK {
            let text = String::from_utf8_lossy(&body).chars().take(300).collect();
            return Err(HttpError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        serde_json::from_slice(&body).map_err(|e| HttpError::Body(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plain_http_needs_to_be_allowed() {
        let strict = HttpClient::new(false).unwrap();
        let err = strict
            .get_json::<serde_json::Value>("http://127.0.0.1:1/x")
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::PlainHttp(_)), "{err}");
        let err = strict
            .get_json::<serde_json::Value>("ftp://example.org/x")
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Url(_)), "{err}");
        let lax = HttpClient::new(true).unwrap();
        let err = lax
            .get_json::<serde_json::Value>("http://127.0.0.1:1/x")
            .await
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Request(_)),
            "allowed, then refused by the network: {err}"
        );
    }

    #[tokio::test]
    async fn non_ok_answers_and_redirects_are_errors() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Answers by path, whatever connects (tests run in parallel).
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 1024];
                    let read = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)
                        .await
                        .unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    let answer = if request.starts_with("GET /redirect ") {
                        "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\nContent-Length: 0\r\n\r\n"
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}"
                    };
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, answer.as_bytes()).await;
                });
            }
        });
        let client = HttpClient::new(true).unwrap();
        let err = client
            .get_json::<serde_json::Value>(&format!("http://{addr}/redirect"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Status { status: 302, .. }),
            "not followed: {err}"
        );
        let ok: serde_json::Value = client.get_json(&format!("http://{addr}/ok")).await.unwrap();
        assert_eq!(ok, serde_json::json!({"ok": true}));
    }
}
