//! OpenAPI contract and interactive documentation (specification §15).
//!
//! The contract is generated from the code: each handler carries a
//! `#[utoipa::path]` annotation and is registered through
//! `utoipa_axum::routes!` (see `routes.rs`). It is served at
//! [`CONTRACT_PATH`], browsable at [`DOCS_PATH`], and exported to
//! `backend/openapi.json` by `just openapi`; the Angular client is generated
//! from that file.
//!
//! The documentation page is Swagger UI, embedded in the binary: it loads no
//! third-party resource, and its Content-Security-Policy forbids it.

use axum::Router;
use axum::http::{HeaderValue, header};
use tower_http::set_header::SetResponseHeaderLayer;
use utoipa::openapi::OpenApi as Contract;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::problem::{ErrorCode, ProblemBody};
use crate::tenant::AppState;
use crate::{auth, platform, reference_items};

/// URL of the contract.
pub const CONTRACT_PATH: &str = "/api/v1/openapi.json";

/// URL of the interactive documentation.
pub const DOCS_PATH: &str = "/api/v1/docs";

/// Only same-origin resources; inline styles are needed by Swagger UI.
const DOCS_CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
                        script-src 'self'; connect-src 'self'; font-src 'self'; object-src 'none'; \
                        base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Header and components shared by the whole contract. Paths are added by
/// the router.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "My Little Big Project Planner API",
        description = "REST API of My Little Big Project Planner. The web interface uses this same API.\n\n\
                       **Conventions**\n\n\
                       - Errors are RFC 9457 Problem Details (`application/problem+json`) with a stable \
                       `code`, listed in the `ErrorCode` schema.\n\
                       - Updates and deletions require `If-Match` with the resource's `ETag` \
                       (428 without it, 412 when the resource changed).\n\
                       - Creations accept an `Idempotency-Key` header: replaying the request does not \
                       create a duplicate.\n\
                       - Lists are paginated with an opaque cursor (`limit`, `cursor`, `nextCursor`).\n\
                       - `?fields=` selects the returned fields.\n\
                       - Translated labels follow `Accept-Language`, then the tenant's default language.\n\n\
                       **Authentication**\n\n\
                       Endpoints need a session unless stated otherwise: sign in with \
                       `POST /api/v1/auth/sign-in`, which sets the `mlbpp_session` cookie. Every \
                       state-changing request (POST, PUT, PATCH, DELETE) must also send the session's \
                       CSRF token in the `X-CSRF-Token` header (403 `csrf.invalid` otherwise); it is in \
                       the `mlbpp_csrf` cookie and in `GET /api/v1/auth/session`.",
        license(name = "MIT", identifier = "MIT"),
    ),
    components(schemas(ErrorCode, ProblemBody)),
    modifiers(&SecuritySchemes),
    security(("session" = [])),
    tags(
        (name = "system", description = "Technical endpoints"),
        (name = auth::TAG, description = "Sign-in, sessions and password reset"),
        (name = platform::TAG, description = "Platform administration, for the platform super admin"),
        (name = reference_items::TAG, description = "Reference resource showing every API convention; \
                                                     new endpoints copy it"),
    )
)]
struct ApiDoc;

/// The `session` cookie scheme required by default.
struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, contract: &mut Contract) {
        let components = contract.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                auth::SESSION_COOKIE,
                "Session cookie set by `POST /api/v1/auth/sign-in`. State-changing requests also \
                 need the `X-CSRF-Token` header.",
            ))),
        );
    }
}

/// The contract before paths are added.
pub(crate) fn base() -> Contract {
    ApiDoc::openapi()
}

/// The complete contract, as served and exported.
pub fn contract() -> Contract {
    crate::routes::documented_routes().split_for_parts().1
}

/// The contract as pretty-printed JSON, ending with a newline (the format of
/// the exported file).
pub fn contract_json() -> Result<String, serde_json::Error> {
    let mut json = contract().to_pretty_json()?;
    json.push('\n');
    Ok(json)
}

/// Routes serving the contract and the documentation page.
pub(crate) fn docs(contract: Contract) -> Router<AppState> {
    let config = Config::default()
        // The default validator is a third-party service.
        .validator_url("none")
        .try_it_out_enabled(true)
        .display_operation_id(true);
    let swagger: Router<AppState> = SwaggerUi::new(DOCS_PATH)
        .url(CONTRACT_PATH, contract)
        .config(config)
        .into();
    swagger.layer(SetResponseHeaderLayer::overriding(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(DOCS_CSP),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const OPENAPI_31_SCHEMA: &str = include_str!("../tests/fixtures/openapi-3.1-schema.json");
    const COMMITTED_CONTRACT: &str = include_str!("../../../openapi.json");

    fn contract_value() -> Value {
        serde_json::from_str(&contract_json().unwrap()).unwrap()
    }

    #[test]
    fn contract_is_a_valid_openapi_3_1_document() {
        let schema: Value = serde_json::from_str(OPENAPI_31_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let contract = contract_value();
        let errors: Vec<String> = validator
            .iter_errors(&contract)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "invalid OpenAPI document:\n{}",
            errors.join("\n")
        );
        assert_eq!(contract["openapi"], "3.1.0");
        // The validator does reject broken documents.
        assert!(!validator.is_valid(&serde_json::json!({"openapi": "3.1.0", "paths": {}})));
        assert!(!validator.is_valid(&serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "t", "version": "1"},
            "paths": {"/x": {"get": {"responses": {"200": {}}}}}
        })));
    }

    #[test]
    fn every_component_schema_is_a_valid_json_schema() {
        let contract = contract_value();
        let schemas = contract["components"]["schemas"].as_object().unwrap();
        for (name, schema) in schemas {
            assert!(
                jsonschema::meta::is_valid(schema),
                "{name} is not a valid JSON Schema"
            );
        }
    }

    #[test]
    fn error_code_schema_lists_the_whole_catalog() {
        let contract = contract_value();
        let schema = &contract["components"]["schemas"]["ErrorCode"];
        let listed: Vec<&str> = schema["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let catalog: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(listed, catalog);
        let description = schema["description"].as_str().unwrap();
        assert!(description.contains("| `reference_item.code_taken` | 409 |"));
    }

    #[test]
    fn every_error_response_is_a_problem() {
        let contract = contract_value();
        for (path, item) in contract["paths"].as_object().unwrap() {
            for (method, operation) in item.as_object().unwrap() {
                for (status, response) in operation["responses"].as_object().unwrap() {
                    if !status.starts_with('4') && !status.starts_with('5') {
                        continue;
                    }
                    assert_eq!(
                        response["content"]["application/problem+json"]["schema"]["$ref"],
                        "#/components/schemas/Problem",
                        "{method} {path} {status}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_operation_has_an_id_a_tag_and_a_summary() {
        let contract = contract_value();
        let mut ids = std::collections::HashSet::new();
        for (path, item) in contract["paths"].as_object().unwrap() {
            assert!(path.starts_with("/api/v1/"), "{path}");
            for (method, operation) in item.as_object().unwrap() {
                let id = operation["operationId"].as_str().unwrap_or_default();
                assert!(!id.is_empty(), "{method} {path} has no operationId");
                assert!(ids.insert(id.to_owned()), "duplicate operationId {id}");
                assert!(operation["tags"].as_array().is_some_and(|t| t.len() == 1), "{id}");
                assert!(operation["summary"].is_string(), "{id}");
            }
        }
    }

    #[test]
    fn writes_require_if_match_and_creations_accept_idempotency_keys() {
        let contract = contract_value();
        let header = |path: &str, method: &str, name: &str| -> Option<bool> {
            contract["paths"][path][method]["parameters"]
                .as_array()?
                .iter()
                .find(|p| p["in"] == "header" && p["name"] == name)
                .map(|p| p["required"] == true)
        };
        let item = "/api/v1/reference-items/{id}";
        assert_eq!(header(item, "put", "If-Match"), Some(true));
        assert_eq!(header(item, "delete", "If-Match"), Some(true));
        assert_eq!(
            header("/api/v1/reference-items", "post", "Idempotency-Key"),
            Some(false)
        );
    }

    /// `backend/openapi.json` is the input of the Angular client generator:
    /// it must match the code.
    #[test]
    fn committed_contract_is_up_to_date() {
        assert!(
            COMMITTED_CONTRACT == contract_json().unwrap(),
            "backend/openapi.json is out of date: run `just generate-client` and commit the result"
        );
    }
}
