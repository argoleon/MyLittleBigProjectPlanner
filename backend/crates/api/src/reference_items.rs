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

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::fields::FieldSelection;
use crate::idempotency;
use crate::pagination::{PageBody, PageParams, encode_cursor};
use crate::problem::{ErrorCode, Problem};
use crate::tenant::TenantTx;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceItemBody {
    pub code: String,
    pub label_translations: BTreeMap<String, String>,
    #[serde(default)]
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
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceItemView {
    id: Uuid,
    code: String,
    label: Option<String>,
    label_translations: BTreeMap<String, String>,
    quantity: i32,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
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
#[derive(Debug, Default, Deserialize)]
pub struct FieldsParam {
    pub fields: Option<String>,
}

/// `?limit=&cursor=&fields=` of the list endpoint. (Not `#[serde(flatten)]`
/// over `PageParams`: flattening breaks number parsing in query strings.)
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    pub fields: Option<String>,
    pub limit: Option<usize>,
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
