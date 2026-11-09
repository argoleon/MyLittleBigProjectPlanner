//! Integration tests: tenant resolution and isolation through HTTP, on a
//! real PostgreSQL database (one per test, see `testkit`).

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use api::{AppState, HttpSettings};
use domain::tenancy::Tenancy;
use testkit::TestDb;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A running API; stops when dropped.
struct TestApi {
    base: String,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for TestApi {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn start(db: &TestDb, tenancy: Tenancy) -> TestApi {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let settings = HttpSettings {
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 1024,
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
        stop: Some(stop),
    }
}

fn multi() -> Tenancy {
    Tenancy::Multi {
        base_domain: "example.test".to_owned(),
    }
}

async fn get_tenant(api: &TestApi, host: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}/api/v1/tenant", api.base))
        .header("host", host)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn each_subdomain_sees_only_its_own_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    for tenant in [&db.tenant_a, &db.tenant_b] {
        let res = get_tenant(&api, &format!("{}.example.test", tenant.slug)).await;
        assert_eq!(res.status(), 200, "tenant {}", tenant.slug);
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(body["id"], tenant.id.to_string());
        assert_eq!(body["slug"], tenant.slug.as_str());
        assert_eq!(body["name"], tenant.name.as_str());
        assert_eq!(body["defaultLocale"], "fr");
        assert_eq!(body["timeZone"], "Europe/Paris");
    }
}

#[tokio::test]
async fn host_with_port_and_uppercase_resolves() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let res = get_tenant(&api, "BETA.example.test:8443").await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["slug"], "beta");
}

#[tokio::test]
async fn unknown_or_missing_subdomain_is_a_problem_404() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    for host in ["gamma.example.test", "example.test", "alpha.other.test"] {
        let res = get_tenant(&api, host).await;
        assert_eq!(res.status(), 404, "{host}");
        assert_eq!(res.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(body["code"], "tenant.not_found", "{host}");
        assert_eq!(body["status"], 404);
    }
}

#[tokio::test]
async fn single_tenant_mode_serves_the_default_tenant_on_any_host() {
    let db = TestDb::new().await;
    let api = start(
        &db,
        Tenancy::Single {
            default_slug: "alpha".to_owned(),
        },
    )
    .await;

    for host in ["planner.internal", "localhost:4200"] {
        let res = get_tenant(&api, host).await;
        assert_eq!(res.status(), 200, "{host}");
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(body["slug"], "alpha");
    }
}

#[tokio::test]
async fn single_tenant_mode_with_unknown_default_is_a_problem_404() {
    let db = TestDb::new().await;
    let api = start(
        &db,
        Tenancy::Single {
            default_slug: "missing".to_owned(),
        },
    )
    .await;
    let res = get_tenant(&api, "localhost").await;
    assert_eq!(res.status(), 404);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["code"], "tenant.not_found");
}

#[tokio::test]
async fn concurrent_requests_for_two_tenants_never_mix() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let mut requests = Vec::new();
    for i in 0..40 {
        let tenant = if i % 2 == 0 { &db.tenant_a } else { &db.tenant_b };
        let url = format!("{}/api/v1/tenant", api.base);
        let host = format!("{}.example.test", tenant.slug);
        let expected = tenant.slug.clone();
        requests.push(tokio::spawn(async move {
            let res = reqwest::Client::new()
                .get(url)
                .header("host", host)
                .send()
                .await
                .unwrap();
            let body: serde_json::Value = res.json().await.unwrap();
            assert_eq!(body["slug"], expected.as_str());
        }));
    }
    for request in requests {
        request.await.unwrap();
    }
}
