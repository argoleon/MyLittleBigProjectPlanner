//! Resources (specification §8, L07-US01): persons, linked to a member or
//! not, and generic resources (unnamed roles). Tenant admins and resource
//! managers create, change and deactivate them; every internal member reads
//! them. Resources are never deleted: assignments refer to them (L08).
//!
//! A member leaving the tenant leaves their resource behind, unlinked and
//! inactive, under the name it showed (migration `resources`).

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::resources::{Kind, ResourceFields, manages_resources, trimmed, validate};
use domain::validation::FieldError;
use infra::resources::{self as repo, NameKey, Resource, ResourceFilter, ResourceInput, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::pagination::{DEFAULT_LIMIT, MAX_LIMIT};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::teams::{TeamRefView, teams_of};
use crate::tenant::TenantTx;

/// OpenAPI tag of resources.
pub const TAG: &str = "resources";

const PATH: &str = "/api/v1/resources";

/// 403 unless the signed-in member manages resources.
fn require_manager(tx: &TenantTx) -> Result<(), Problem> {
    if manages_resources(tx.member().tenant_role) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden).with_detail("tenant admins and resource managers only"))
    }
}

/// Kind of a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ResourceKind)]
pub enum KindView {
    /// Someone, linked to a member's account or not.
    Person,
    /// An unnamed role ("Rust developer").
    Generic,
}

impl From<KindView> for Kind {
    fn from(kind: KindView) -> Self {
        match kind {
            KindView::Person => Self::Person,
            KindView::Generic => Self::Generic,
        }
    }
}

impl From<Kind> for KindView {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Person => Self::Person,
            Kind::Generic => Self::Generic,
        }
    }
}

/// A resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Resource)]
pub struct ResourceView {
    id: Uuid,
    kind: KindView,
    /// The member's name when linked to an account, else its own.
    #[schema(example = "Camille Martin")]
    display_name: String,
    #[schema(example = "Back-end developer")]
    #[serde(skip_serializing_if = "Option::is_none")]
    job_title: Option<String>,
    /// The member this person is; `null` for a person without an account
    /// and a generic resource.
    #[schema(required = true)]
    account_id: Option<Uuid>,
    /// A member who left the tenant: no longer linked, and inactive when
    /// they left.
    former_member: bool,
    /// Offered for new assignments.
    active: bool,
    /// The teams it belongs to, by name in the language of the request.
    teams: Vec<TeamRefView>,
    #[schema(example = 1)]
    version: i64,
}

impl ResourceView {
    fn new(resource: Resource, teams: Vec<TeamRefView>) -> Self {
        Self {
            id: resource.id,
            kind: resource.kind.into(),
            display_name: resource.display_name,
            job_title: resource.job_title,
            account_id: resource.account_id,
            former_member: resource.former_member,
            active: resource.active,
            teams,
            version: resource.version,
        }
    }
}

/// The views of `resources`, with their teams.
async fn views(
    tx: &mut TenantTx,
    language: &Language,
    resources: Vec<Resource>,
) -> Result<Vec<ResourceView>, Problem> {
    let ids: Vec<Uuid> = resources.iter().map(|r| r.id).collect();
    let mut teams = teams_of(tx, language, &ids).await?;
    Ok(resources
        .into_iter()
        .map(|r| {
            let of_resource = teams.remove(&r.id).unwrap_or_default();
            ResourceView::new(r, of_resource)
        })
        .collect())
}

async fn view_of(
    tx: &mut TenantTx,
    language: &Language,
    resource: Resource,
) -> Result<ResourceView, Problem> {
    let teams = teams_of(tx, language, &[resource.id])
        .await?
        .remove(&resource.id)
        .unwrap_or_default();
    Ok(ResourceView::new(resource, teams))
}

/// A page of resources.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePage {
    items: Vec<ResourceView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
    /// The signed-in member creates and changes resources.
    can_manage: bool,
}

/// `?kind=&active=&search=&team=&limit=&cursor=` of the resource list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ResourceListParams {
    /// Only this kind.
    pub kind: Option<KindView>,
    /// Only active (`true`) or inactive (`false`) resources; all when left
    /// out.
    pub active: Option<bool>,
    /// Part of the name or job title, any case.
    pub search: Option<String>,
    /// Only the members of this team.
    pub team: Option<Uuid>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// A resource to create, or its new state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ResourceFields, example = json!({
    "kind": "person",
    "displayName": "Camille Martin",
    "jobTitle": "Back-end developer",
    "accountId": null,
    "active": true
}))]
pub struct ResourceBody {
    kind: KindView,
    /// 1 to 200 characters. Required unless `accountId` is set: a linked
    /// person shows its member's name, and this one is ignored.
    #[serde(default)]
    display_name: Option<String>,
    /// Up to 100 characters.
    #[serde(default)]
    job_title: Option<String>,
    /// An internal member of the tenant not linked to another resource; a
    /// person only.
    #[serde(default)]
    account_id: Option<Uuid>,
    /// `true` when left out.
    #[serde(default = "active_by_default")]
    active: bool,
}

const fn active_by_default() -> bool {
    true
}

impl ResourceBody {
    /// Checked by the domain rules.
    fn checked(&self) -> Result<ResourceInput<'_>, Problem> {
        let fields = ResourceFields {
            kind: self.kind.into(),
            display_name: self.display_name.as_deref(),
            job_title: self.job_title.as_deref(),
            linked: self.account_id.is_some(),
        };
        validate(&fields).map_err(Problem::validation)?;
        Ok(ResourceInput {
            kind: fields.kind,
            display_name: trimmed(fields.display_name),
            job_title: trimmed(fields.job_title),
            account_id: self.account_id,
            active: self.active,
        })
    }
}

/// A member a person can be linked to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkableAccount {
    account_id: Uuid,
    #[schema(example = "Camille Martin")]
    display_name: String,
    #[schema(example = "camille.martin@acme.example")]
    email: String,
}

/// `?search=&limit=` of the members a person can be linked to.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LinkableParams {
    /// Part of the name or email, any case.
    pub search: Option<String>,
    /// Most members, from 1 to 50 (default 10).
    #[param(minimum = 1, maximum = 50)]
    pub limit: Option<usize>,
}

const CURSOR_VERSION: &str = "r1";

fn encode_cursor(key: &NameKey) -> String {
    URL_SAFE_NO_PAD.encode(format!("{CURSOR_VERSION}:{}:{}", key.id.simple(), key.name))
}

fn decode_cursor(cursor: &str) -> Option<NameKey> {
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(cursor).ok()?).ok()?;
    let (version, rest) = raw.split_once(':')?;
    let (id, name) = rest.split_once(':')?;
    (version == CURSOR_VERSION).then_some(())?;
    Some(NameKey {
        name: name.to_owned(),
        id: Uuid::try_parse(id).ok()?,
    })
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::AccountTaken => Problem::new(ErrorCode::ResourceAccountTaken)
            .with_detail("this member is already linked to another resource")
            .with_errors(vec![FieldError::new("accountId", "taken")]),
        WriteConflict::NotInternalMember => {
            Problem::validation(vec![FieldError::new("accountId", "not_internal_member")])
        }
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

/// `GET /api/v1/resources`, by name.
#[utoipa::path(
    get,
    path = "/resources",
    tag = TAG,
    operation_id = "listResources",
    summary = "List the resources of the tenant",
    description = "Any internal member. By name; follow `nextCursor` until it is `null`.",
    params(
        ResourceListParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of the team names"),
    ),
    responses(
        (status = 200, description = "A page of resources", body = ResourcePage, headers(
            ("Content-Language" = String, description = "Language of the team names"),
        )),
        (status = 400, description = "`invalid_query`: bad `limit`, `cursor` or `kind`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<ResourceListParams>,
) -> Result<Response, Problem> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail(format!("limit: 1 to {MAX_LIMIT}")));
    }
    let after = match params.cursor.as_deref() {
        None | Some("") => None,
        Some(cursor) => Some(decode_cursor(cursor).ok_or_else(|| {
            Problem::new(ErrorCode::InvalidQuery).with_detail("cursor is invalid or expired")
        })?),
    };
    let filter = ResourceFilter {
        kind: params.kind.map(Kind::from),
        active: params.active,
        search: trimmed(params.search.as_deref()),
        team: params.team,
    };
    let page = repo::list(&mut tx, &filter, after.as_ref(), limit).await?;
    let can_manage = manages_resources(tx.member().tenant_role);
    let language = Language::of(&tx, &accept);
    let items = views(&mut tx, &language, page.items).await?;
    tx.commit().await?;
    let mut response = ApiJson(ResourcePage {
        items,
        next_cursor: page.next.as_ref().map(encode_cursor),
        can_manage,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/resources`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/resources",
    tag = TAG,
    operation_id = "createResource",
    summary = "Create a resource",
    description = "Tenant admins and resource managers. A person linked to a member shows the \
                   member's name; one member is linked to one resource at most.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the team names"),
    ),
    request_body = ResourceBody,
    responses(
        (status = 201, description = "Created", body = ResourceView, headers(
            ("ETag" = String, description = "Version of the resource"),
            ("Location" = String, description = "URL of the resource"),
        )),
        (status = 400, description = "`validation_failed` (`displayName`: `required`, `too_long`; `jobTitle`: \
                                      `too_long`; `accountId`: `person_only`, `not_internal_member`)",
                       body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`resource.account_taken`: the member is linked to another resource; or \
                                      `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<ResourceBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = body.checked()?;
    let resource = repo::insert(&mut tx, infra::id::new_id(), &input)
        .await?
        .map_err(conflict_problem)?;
    let actor = tx.account_id();
    tracing::info!(target: "audit", actor = %actor, resource = %resource.id, kind = resource.kind.as_str(),
                   account = ?resource.account_id, "resource created");
    let (id, version) = (resource.id, resource.version);
    let language = Language::of(&tx, &accept);
    let view = view_of(&mut tx, &language, resource).await?;
    let response =
        idempotency::created(&mut tx, key.as_deref(), &view, format!("{PATH}/{id}"), version).await?;
    tx.commit().await?;
    Ok(response)
}

/// `GET /api/v1/resources/{id}`.
#[utoipa::path(
    get,
    path = "/resources/{id}",
    tag = TAG,
    operation_id = "getResource",
    summary = "Read a resource",
    description = "Any internal member.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the team names"),
    ),
    responses(
        (status = 200, description = "The resource", body = ResourceView, headers(
            ("ETag" = String, description = "Version of the resource"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let resource = repo::find(&mut tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound))?;
    let version = resource.version;
    let language = Language::of(&tx, &accept);
    let view = view_of(&mut tx, &language, resource).await?;
    tx.commit().await?;
    let mut response = if if_none_match(&headers, version) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ApiJson(view).into_response()
    };
    language.headers(&mut response);
    Ok(with_etag(response, version))
}

/// `PUT /api/v1/resources/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/resources/{id}",
    tag = TAG,
    operation_id = "updateResource",
    summary = "Change or deactivate a resource",
    description = "Tenant admins and resource managers: every field replaced. `active: false` \
                   deactivates it (resources are never deleted). Linking a member forgets the \
                   member it was; leaving `accountId` out unlinks it.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the team names"),
    ),
    request_body = ResourceBody,
    responses(
        (status = 200, description = "The resource", body = ResourceView, headers(
            ("ETag" = String, description = "New version of the resource"),
        )),
        (status = 400, description = "`validation_failed`, as on creation", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`resource.account_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The resource changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ResourceBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = body.checked()?;
    let resource = repo::update(&mut tx, id, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    let actor = tx.account_id();
    tracing::info!(target: "audit", actor = %actor, resource = %id, active = resource.active,
                   account = ?resource.account_id, "resource changed");
    let version = resource.version;
    let language = Language::of(&tx, &accept);
    let view = view_of(&mut tx, &language, resource).await?;
    tx.commit().await?;
    let mut response = ApiJson(view).into_response();
    language.headers(&mut response);
    Ok(with_etag(response, version))
}

/// `GET /api/v1/resources/linkable-accounts`.
#[utoipa::path(
    get,
    path = "/resources/linkable-accounts",
    tag = TAG,
    operation_id = "listLinkableAccounts",
    summary = "Search the members a person can be linked to",
    description = "Tenant admins and resource managers. The internal members linked to no resource \
                   yet, whose name or email holds `search`, by name.",
    params(LinkableParams),
    responses(
        (status = 200, description = "The members", body = Vec<LinkableAccount>),
        (status = 400, description = "Invalid `limit`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn linkable_accounts(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<LinkableParams>,
) -> Result<ApiJson<Vec<LinkableAccount>>, Problem> {
    require_manager(&tx)?;
    let limit = params.limit.unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("limit: 1 to 50"));
    }
    let accounts = repo::linkable_accounts(&mut tx, trimmed(params.search.as_deref()), limit).await?;
    tx.commit().await?;
    Ok(ApiJson(
        accounts
            .into_iter()
            .map(|a| LinkableAccount {
                account_id: a.account_id,
                display_name: a.display_name,
                email: a.email,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_names_with_colons() {
        let key = NameKey {
            name: "dev: rust".to_owned(),
            id: Uuid::from_u128(7),
        };
        assert_eq!(decode_cursor(&encode_cursor(&key)), Some(key));
    }

    #[test]
    fn foreign_and_tampered_cursors_are_rejected() {
        for bad in [
            "not base64!".to_owned(),
            URL_SAFE_NO_PAD.encode("v1:00000000000000000000000000000001:x"),
            URL_SAFE_NO_PAD.encode("r1:not-a-uuid:x"),
            URL_SAFE_NO_PAD.encode("r1"),
        ] {
            assert_eq!(decode_cursor(&bad), None, "{bad}");
        }
    }
}
