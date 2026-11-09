//! Helpers shared by the API integration tests.

#![allow(clippy::unwrap_used, dead_code)]

use std::time::Duration;

use api::{AppState, HttpSettings};
use domain::tenancy::Tenancy;
use testkit::{SeedTenant, TestDb};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A running API on a random port; stops when dropped.
pub struct TestApi {
    pub base: String,
    pub client: reqwest::Client,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for TestApi {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Multi-tenant mode on `*.example.test`.
pub fn multi() -> Tenancy {
    Tenancy::Multi {
        base_domain: "example.test".to_owned(),
    }
}

pub async fn start(db: &TestDb, tenancy: Tenancy) -> TestApi {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let settings = HttpSettings {
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 16 * 1024,
    };
    let app = api::app(AppState::new(db.app_pool.clone(), tenancy), &settings);
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(api::serve(
        listener,
        app,
        async move {
            let _ = stopped.await;
        },
        Duration::from_secs(1),
    ));
    TestApi {
        base,
        client: reqwest::Client::new(),
        stop: Some(stop),
    }
}

impl TestApi {
    /// A request to `path` on behalf of `tenant` (its subdomain as Host).
    pub fn request(
        &self,
        method: reqwest::Method,
        tenant: &SeedTenant,
        path: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("host", format!("{}.example.test", tenant.slug))
    }
}

/// Status and JSON body of a response.
pub async fn json(res: reqwest::Response) -> (u16, serde_json::Value) {
    let status = res.status().as_u16();
    let text = res.text().await.unwrap();
    let body = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"))
    };
    (status, body)
}
