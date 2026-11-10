//! `/api/v1/reference-items`: the reference resource.
//!
//! **Copy this file to build a new endpoint** (guide:
//! `docs/en/developer-guide/new-endpoint.md`). It shows every API convention
//! of the specification (§15) on a deliberately trivial resource:
//!
//! | Convention | Where |
//! | --- | --- |
//! | Tenant isolation | every handler takes a [`TenantTx`] |
//! | Validation with field errors | [`create`], [`update`] via `domain::reference_item::validate` |
//! | Problem Details errors | every `Err(Problem)` |
//! | `Idempotency-Key` | [`create`] |
//! | `ETag` / `If-Match` / `If-None-Match` | [`read`], [`update`], [`delete`] |
//! | Cursor pagination | [`list`] |
//! | Field selection (`?fields=`) | [`read`], [`list`], [`create`], [`update`] |
//! | Translated labels (`Accept-Language`) | [`ReferenceItemView`] |
//!
//! Layers: rules in `domain::reference_item`, SQL in
//! `infra::reference_items`, HTTP here.

use std::collections::BTreeMap;

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::reference_item::{ReferenceItemInput, validate};
use infra::idempotency::{Claim, StoredResponse};
use infra::reference_items::{self as repo, ReferenceItem, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::fields::FieldSelection;
use crate::idempotency;
use crate::pagination::{PageBody, PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::TenantTx;

/// OpenAPI tag of this resource.
pub const TAG: &str = "reference-items";

const PATH: &str = "/api/v1/reference-items";

/// Every field a client can select, and those returned by default.
/// `labelTranslations` (every language) is only sent on request.
const FIELDS: &[&str] = &[
    "id",
    "code",
    "label",
    "labelTranslations",
    "quantity",
    "version",
    "createdAt",
    "updatedAt",
];
const DEFAULT_FIELDS: &[&str] = &[
    "id",
    "code",
    "label",
    "quantity",
    "version",
    "createdAt",
    "updatedAt",
];

/// Body of `POST` and `PUT`. Unknown fields are refused, so typos are not
/// silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = ReferenceItemInput,
    example = json!({"code": "REF-1", "labelTranslations": {"fr": "Référence", "en": "Reference"}, "quantity": 3})
)]
pub struct ReferenceItemBody {
    /// 2 to 20 characters among `A-Z`, `0-9` and `-`, starting with a letter;
    /// unique in the tenant.
    #[schema(
        example = "REF-1",
        min_length = 2,
        max_length = 20,
        pattern = "^[A-Z][A-Z0-9-]{1,19}$"
    )]
    pub code: String,
    /// Label per language (`fr`, `en`), at least one; 200 characters at most.
    pub label_translations: BTreeMap<String, String>,
    /// From 0 to 1,000,000; 0 when absent.
    #[serde(default)]
    #[schema(minimum = 0, maximum = 1_000_000, default = 0)]
    pub quantity: i32,
}

impl From<ReferenceItemBody> for ReferenceItemInput {
    fn from(body: ReferenceItemBody) -> Self {
        Self {
            code: body.code,
            label_translations: body.label_translations,
            quantity: body.quantity,
        }
    }
}

/// What the API returns. `label` is resolved in the request's language.
/// With `?fields=`, only the selected fields are present.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(
    as = ReferenceItem,
    example = json!({
        "id": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8b",
        "code": "REF-1",
        "label": "Reference",
        "quantity": 3,
        "version": 1,
        "createdAt": "2026-11-09T10:00:00Z",
        "updatedAt": "2026-11-09T10:00:00Z"
    })
)]
pub struct ReferenceItemView {
    id: Uuid,
    code: String,
    /// Label in the response language (`Content-Language`), falling back to
    /// the tenant's default language, then to any translation; `null` when
    /// the item has no label at all.
    #[schema(required = true)]
    label: Option<String>,
    /// Every translation; only returned when selected with `?fields=`.
    #[schema(required = false)]
    label_translations: BTreeMap<String, String>,
    quantity: i32,
    /// Incremented on every change; the `ETag` is this value in quotes.
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// Documentation only: `list` builds a `PageBody` of JSON values, restricted
// to the selected fields. (`///` comments here would end up in the contract.)
/// A page of reference items, in creation order.
#[derive(Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ReferenceItemPage {
    items: Vec<ReferenceItemView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

impl ReferenceItemView {
    fn new(item: ReferenceItem, locale: &str, fallback: &str) -> Self {
        let label = domain::i18n::resolve(&item.label_translations, locale, fallback).map(str::to_owned);
        Self {
            id: item.id,
            code: item.code,
            label,
            label_translations: item.label_translations,
            quantity: item.quantity,
            version: item.version,
            created_at: item.created_at,
            updated_at: item.updated_at,
        }
    }
}

/// `?fields=` of single-resource endpoints.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FieldsParam {
    /// Comma-separated fields to return, e.g. `id,code`. Default: every field
    /// but `labelTranslations`.
    pub fields: Option<String>,
}

/// `?limit=&cursor=&fields=` of the list endpoint. (Not `#[serde(flatten)]`
/// over `PageParams`: flattening breaks number parsing in query strings.)
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// Comma-separated fields to return for each item, e.g. `id,code`.
    pub fields: Option<String>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// Language of the response for this request and tenant.
struct Language {
    locale: &'static str,
    fallback: String,
}

impl Language {
    fn of(tx: &TenantTx, accept: &AcceptLanguage) -> Self {
        let fallback = tx.default_locale().to_owned();
        let locale = domain::i18n::negotiate(accept.0.as_deref(), &fallback);
        Self { locale, fallback }
    }

    fn view(&self, item: ReferenceItem) -> ReferenceItemView {
        ReferenceItemView::new(item, self.locale, &self.fallback)
    }
}

/// JSON of one resource restricted to the selected fields.
fn to_json(view: &ReferenceItemView, fields: &FieldSelection) -> Result<serde_json::Value, Problem> {
    let value = serde_json::to_value(view).map_err(|e| Problem::internal(&e))?;
    Ok(fields.apply(value))
}

/// Response headers of a single resource.
fn resource_headers(response: &mut Response, version: i64, language: &Language) {
    let headers = response.headers_mut();
    headers.insert(header::ETAG, etag(version));
    headers.insert(
        header::CONTENT_LANGUAGE,
        HeaderValue::from_static(language.locale),
    );
    headers.insert(header::VARY, HeaderValue::from_static("accept-language"));
}

fn location(id: Uuid) -> String {
    format!("{PATH}/{id}")
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::CodeTaken => Problem::new(ErrorCode::ReferenceItemCodeTaken)
            .with_errors(vec![domain::validation::FieldError::new("code", "taken")]),
    }
}

/// `POST /api/v1/reference-items`: create; idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/reference-items",
    tag = TAG,
    operation_id = "createReferenceItem",
    summary = "Create a reference item",
    description = "Replaying the same request with the same `Idempotency-Key` within 24 hours \
                   returns the first response (`Idempotent-Replayed: true`) without creating a duplicate.",
    params(
        FieldsParam,
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = ReferenceItemBody,
    responses(
        (status = 201, description = "Created", body = ReferenceItemView, headers(
            ("ETag" = String, description = "Version of the item"),
            ("Location" = String, description = "URL of the item"),
            ("Content-Language" = String, description = "Language of `label`"),
        )),
        (status = 400, description = "Invalid fields, JSON or `Idempotency-Key`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`reference_item.code_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 415, description = "Body is not JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`: missing or wrong `X-CSRF-Token`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiQuery(params): ApiQuery<FieldsParam>,
    ApiJson(body): ApiJson<ReferenceItemBody>,
) -> Result<Response, Problem> {
    let fields = FieldSelection::parse(params.fields.as_deref(), FIELDS, DEFAULT_FIELDS)?;
    let language = Language::of(&tx, &accept);

    // 1. Idempotency: a replay returns the first response, untouched.
    let key = idempotency::key(&headers)?;
    if let Some(key) = &key {
        let hash = idempotency::request_hash(&format!("POST {PATH}"), &body)?;
        match infra::idempotency::claim(&mut tx, key, &hash).await? {
            Claim::New => {}
            Claim::Replay(stored) => return Ok(idempotency::replay(stored)),
            Claim::Mismatch => return Err(Problem::new(ErrorCode::IdempotencyKeyReused)),
            Claim::InFlight => return Err(Problem::new(ErrorCode::IdempotencyKeyInFlight)),
        }
    }

    // 2. Business rules: every field error at once.
    let input = ReferenceItemInput::from(body);
    validate(&input).map_err(Problem::validation)?;

    // 3. Write. The id is generated here (UUID v7), never by the database.
    let item = repo::insert(&mut tx, infra::id::new_id(), &input)
        .await?
        .map_err(conflict_problem)?;

    // 4. Response, stored for replays in the same transaction, then commit.
    let version = item.version;
    let id = item.id;
    let json = to_json(&language.view(item), &fields)?;
    if let Some(key) = &key {
        let stored = StoredResponse {
            status: StatusCode::CREATED.as_u16(),
            body: json.clone(),
            location: Some(location(id)),
            etag: Some(format!("\"{version}\"")),
        };
        infra::idempotency::complete(&mut tx, key, &stored).await?;
    }
    tx.commit().await?;

    let mut response = (StatusCode::CREATED, axum::Json(json)).into_response();
    resource_headers(&mut response, version, &language);
    if let Ok(value) = HeaderValue::from_str(&location(id)) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    Ok(response)
}

/// `GET /api/v1/reference-items/{id}`; 304 if `If-None-Match` is current.
#[utoipa::path(
    get,
    path = "/reference-items/{id}",
    tag = TAG,
    operation_id = "getReferenceItem",
    summary = "Read a reference item",
    params(
        ("id" = Uuid, Path, description = "Id of the item"),
        FieldsParam,
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "The item", body = ReferenceItemView, headers(
            ("ETag" = String, description = "Version of the item"),
            ("Content-Language" = String, description = "Language of `label`"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 400, description = "Unknown field in `fields`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such item in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<FieldsParam>,
) -> Result<Response, Problem> {
    let fields = FieldSelection::parse(params.fields.as_deref(), FIELDS, DEFAULT_FIELDS)?;
    let language = Language::of(&tx, &accept);

    // Another tenant's item is invisible (row-level security): 404, like a missing one.
    let item = repo::find(&mut tx, id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tx.commit().await?;

    let version = item.version;
    if if_none_match(&headers, version) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        resource_headers(&mut response, version, &language);
        return Ok(response);
    }
    let mut response = axum::Json(to_json(&language.view(item), &fields)?).into_response();
    resource_headers(&mut response, version, &language);
    Ok(response)
}

/// `GET /api/v1/reference-items?limit=&cursor=&fields=`, in creation order.
#[utoipa::path(
    get,
    path = "/reference-items",
    tag = TAG,
    operation_id = "listReferenceItems",
    summary = "List reference items",
    description = "In creation order. Follow `nextCursor` until it is `null`.",
    params(
        ListParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "A page of items", body = ReferenceItemPage, headers(
            ("Content-Language" = String, description = "Language of `label`"),
        )),
        (status = 400, description = "Invalid `limit`, `cursor` or `fields`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<Response, Problem> {
    let fields = FieldSelection::parse(params.fields.as_deref(), FIELDS, DEFAULT_FIELDS)?;
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let language = Language::of(&tx, &accept);

    let page = repo::list(&mut tx, after, limit).await?;
    tx.commit().await?;

    let items = page
        .items
        .into_iter()
        .map(|item| to_json(&language.view(item), &fields))
        .collect::<Result<Vec<_>, _>>()?;
    let body = PageBody {
        items,
        next_cursor: page.next.map(encode_cursor),
    };
    let mut response = ApiJson(body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_LANGUAGE,
        HeaderValue::from_static(language.locale),
    );
    headers.insert(header::VARY, HeaderValue::from_static("accept-language"));
    Ok(response)
}

/// `PUT /api/v1/reference-items/{id}`: replace; requires `If-Match`.
#[utoipa::path(
    put,
    path = "/reference-items/{id}",
    tag = TAG,
    operation_id = "updateReferenceItem",
    summary = "Replace a reference item",
    params(
        ("id" = Uuid, Path, description = "Id of the item"),
        FieldsParam,
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = ReferenceItemBody,
    responses(
        (status = 200, description = "The updated item", body = ReferenceItemView, headers(
            ("ETag" = String, description = "New version of the item"),
            ("Content-Language" = String, description = "Language of `label`"),
        )),
        (status = 400, description = "Invalid fields or JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such item in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`reference_item.code_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The item changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`: missing or wrong `X-CSRF-Token`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<FieldsParam>,
    ApiJson(body): ApiJson<ReferenceItemBody>,
) -> Result<Response, Problem> {
    let fields = FieldSelection::parse(params.fields.as_deref(), FIELDS, DEFAULT_FIELDS)?;
    let if_match = require_if_match(&headers)?;
    let language = Language::of(&tx, &accept);

    let input = ReferenceItemInput::from(body);
    validate(&input).map_err(Problem::validation)?;

    let item = repo::update(&mut tx, id, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;

    let version = item.version;
    let mut response = axum::Json(to_json(&language.view(item), &fields)?).into_response();
    resource_headers(&mut response, version, &language);
    Ok(response)
}

/// `DELETE /api/v1/reference-items/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/reference-items/{id}",
    tag = TAG,
    operation_id = "deleteReferenceItem",
    summary = "Delete a reference item",
    params(
        ("id" = Uuid, Path, description = "Id of the item"),
        ("If-Match" = String, Header, description = "ETag of the version being deleted, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "No such item in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The item changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`: no valid session", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`: missing or wrong `X-CSRF-Token`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
