//! Helpers shared by the API integration tests.

#![allow(clippy::unwrap_used, dead_code)]

use std::collections::HashMap;
use std::time::Duration;

use api::{AppState, Auth, HttpSettings};
use domain::tenancy::Tenancy;
use infra::accounts::TenantRole;
use infra::config::{Env, PublicUrl, SessionSettings};
use infra::sessions::{SessionLifetime, SessionStore};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb, TestMember};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A running API on a random port; stops when dropped.
///
/// [`start`] signs in a tenant administrator of each seeded tenant:
/// [`TestApi::request`] sends that member's session cookie and CSRF token,
/// [`TestApi::anonymous`] sends neither.
pub struct TestApi {
    pub base: String,
    pub client: reqwest::Client,
    /// Valkey and key prefix of this API's sessions and rate limits.
    pub valkey: infra::valkey::Valkey,
    pub key_prefix: String,
    /// The signed-in member of each seeded tenant, by tenant id.
    pub members: HashMap<Uuid, TestSession>,
    stop: Option<oneshot::Sender<()>>,
}

/// A member signed in for the tests.
#[derive(Debug, Clone)]
pub struct TestSession {
    pub member: TestMember,
    pub token: String,
    pub csrf_token: String,
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

/// Session settings of the tests: `Secure` cookies, as in production.
pub fn session_settings() -> SessionSettings {
    SessionSettings {
        cookie_secure: true,
        idle: Duration::from_secs(600),
        max_age: Duration::from_secs(3600),
    }
}

pub fn public_url(tenancy: &Tenancy) -> PublicUrl {
    let url = match tenancy {
        Tenancy::Multi { .. } => "https://{tenant}.example.test",
        Tenancy::Single { .. } => "https://planner.example.test",
    };
    PublicUrl::from_env(&Env::from_pairs([("PUBLIC_URL", url)]), tenancy).unwrap()
}

/// Authentication on a Valkey nobody listens to, for tests that never
/// authenticate. Needs a Tokio runtime.
pub fn offline_auth(tenancy: &Tenancy) -> Auth {
    let valkey = infra::valkey::connect_lazy(&infra::config::Secret::new("redis://127.0.0.1:1")).unwrap();
    Auth::new(
        valkey,
        "offline:",
        public_url(tenancy),
        session_settings(),
        data_cipher(),
        true,
    )
    .unwrap()
}

/// The key encrypting stored secrets in the tests (32 bytes in base64).
pub const DATA_KEY: &str = "S2V5IG9mIHRoZSB0ZXN0cywgMzIgYnl0ZXMgbG9uZyE="; // gitleaks:allow

pub fn data_cipher() -> infra::crypto::SecretCipher {
    infra::crypto::SecretCipher::new(&infra::config::Secret::new(DATA_KEY)).unwrap()
}

pub async fn start(db: &TestDb, tenancy: Tenancy) -> TestApi {
    let (valkey, key_prefix) = testkit::valkey().await;
    let auth = Auth::new(
        valkey.clone(),
        &key_prefix,
        public_url(&tenancy),
        session_settings(),
        data_cipher(),
        // The test identity provider (Keycloak) is served over plain HTTP.
        true,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let settings = HttpSettings {
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 16 * 1024,
    };
    let app = api::app(AppState::new(db.app_pool.clone(), tenancy, auth), &settings);
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(api::serve(
        listener,
        app,
        async move {
            let _ = stopped.await;
        },
        Duration::from_secs(1),
    ));

    // Sessions opened directly in the store the API reads.
    let store = SessionStore::new(
        valkey.clone(),
        key_prefix.clone(),
        SessionLifetime {
            idle: session_settings().idle,
            max_age: session_settings().max_age,
        },
    );
    let mut members = HashMap::new();
    for tenant in [&db.tenant_a, &db.tenant_b] {
        let member = db.add_member(tenant, TenantRole::TenantAdmin).await;
        let opened = store.create(member.account_id, tenant.id).await.unwrap();
        members.insert(
            tenant.id,
            TestSession {
                member,
                token: opened.token,
                csrf_token: opened.session.csrf_token,
            },
        );
    }
    TestApi {
        base,
        client: reqwest::Client::new(),
        valkey,
        key_prefix,
        members,
        stop: Some(stop),
    }
}

impl TestApi {
    /// A request to `path` by the signed-in member of `tenant` (its subdomain
    /// as Host, its session cookie, its CSRF token).
    pub fn request(
        &self,
        method: reqwest::Method,
        tenant: &SeedTenant,
        path: &str,
    ) -> reqwest::RequestBuilder {
        let session = &self.members[&tenant.id];
        self.anonymous(method, tenant, path)
            .header("cookie", format!("mlbpp_session={}", session.token))
            .header("x-csrf-token", &session.csrf_token)
    }

    /// A request to `path` on `tenant`'s host without session.
    pub fn anonymous(
        &self,
        method: reqwest::Method,
        tenant: &SeedTenant,
        path: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("host", format!("{}.example.test", tenant.slug))
    }

    /// The signed-in member of `tenant`.
    pub fn session(&self, tenant: &SeedTenant) -> &TestSession {
        &self.members[&tenant.id]
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
