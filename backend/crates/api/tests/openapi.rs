//! Integration tests: the OpenAPI contract and the documentation page, as
//! served by a running API.

#![allow(clippy::unwrap_used)]

mod common;

use common::{json, multi, start};
use reqwest::Method;
use reqwest::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION};
use serde_json::Value;
use testkit::TestDb;

#[tokio::test]
async fn serves_the_generated_contract() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;

    let res = api
        .client
        .get(format!("{}/api/v1/openapi.json", api.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let (_, served) = json(res).await;

    let generated: Value = serde_json::from_str(&api::openapi::contract_json().unwrap()).unwrap();
    assert_eq!(served, generated);
    assert_eq!(served["openapi"], "3.1.0");
    for (path, methods) in [
        ("/api/v1/reference-items", &["get", "post"][..]),
        ("/api/v1/reference-items/{id}", &["get", "put", "delete"][..]),
    ] {
        for method in methods {
            assert!(served["paths"][path][method].is_object(), "{method} {path}");
        }
    }
    assert!(served["components"]["schemas"]["ErrorCode"]["enum"].is_array());
}

/// The example of the contract is a request the API accepts, and the
/// response has every field the contract requires: what a user trying the
/// endpoint from the documentation page gets.
#[tokio::test]
async fn the_contract_example_creates_a_reference_item() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let contract: Value = serde_json::from_str(&api::openapi::contract_json().unwrap()).unwrap();
    let schemas = &contract["components"]["schemas"];

    let res = api
        .request(Method::POST, &db.tenant_a, "/api/v1/reference-items")
        .json(&schemas["ReferenceItemInput"]["example"])
        .send()
        .await
        .unwrap();
    let (status, body) = json(res).await;
    assert_eq!(status, 201, "{body}");
    for field in schemas["ReferenceItem"]["required"].as_array().unwrap() {
        let field = field.as_str().unwrap();
        assert!(body.get(field).is_some(), "required field {field} missing");
    }
}

#[tokio::test]
async fn documentation_page_is_self_hosted() {
    let db = TestDb::new().await;
    let api = start(&db, multi()).await;
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = no_redirect
        .get(format!("{}/api/v1/docs", api.base))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_redirection());
    assert_eq!(res.headers()[LOCATION], "/api/v1/docs/");

    let res = api
        .client
        .get(format!("{}/api/v1/docs/", api.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let csp = res.headers()[CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(!csp.contains("http"), "{csp}");
    let html = res.text().await.unwrap();
    assert!(!html.contains("http://") && !html.contains("https://"), "{html}");

    // Every stylesheet and script the page references is served by the API.
    let assets: Vec<&str> = html
        .split(['"', '\''])
        .filter(|s| {
            std::path::Path::new(s)
                .extension()
                .is_some_and(|ext| ext == "js" || ext == "css")
        })
        .collect();
    assert!(assets.contains(&"./swagger-ui-bundle.js"), "{assets:?}");
    for asset in assets {
        let url = format!("{}/api/v1/docs/{}", api.base, asset.trim_start_matches("./"));
        let res = api.client.get(&url).send().await.unwrap();
        assert_eq!(res.status(), 200, "{url}");
    }

    // The page loads our contract and disables the third-party validator.
    let init = api
        .client
        .get(format!("{}/api/v1/docs/swagger-initializer.js", api.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(init.contains("/api/v1/openapi.json"), "{init}");
    assert!(
        init.contains(r#""validatorUrl": "none""#) || init.contains(r#""validatorUrl":"none""#),
        "{init}"
    );
    assert!(!init.contains("https://"), "{init}");
}
