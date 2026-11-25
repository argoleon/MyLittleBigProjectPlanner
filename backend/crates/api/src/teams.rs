//! Teams (specification §8, L07-US02): resources grouped for views and
//! filters. A resource may belong to several teams; assignments always
//! target a resource, never a team (L08). Tenant admins and resource
//! managers manage teams; every internal member reads them. The members of
//! a team are listed by `GET /resources?team=`.

use std::collections::{BTreeMap, HashMap};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::resources::manages_resources;
use domain::teams::{TeamInput, validate};
use domain::validation::FieldError;
use infra::teams::{self as repo, MemberChange, Team, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::roles::Language;
use crate::tenant::TenantTx;

const PATH: &str = "/api/v1/teams";

/// 403 unless the signed-in member manages resources and teams.
fn require_manager(tx: &TenantTx) -> Result<(), Problem> {
    if manages_resources(tx.member().tenant_role) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden).with_detail("tenant admins and resource managers only"))
    }
}

/// A team.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Team)]
pub struct TeamView {
    id: Uuid,
    /// In the language of the request (`Accept-Language`), else the
    /// tenant's default, else any.
    #[schema(example = "Platform")]
    name: Option<String>,
    #[schema(example = json!({"fr": "Plateforme", "en": "Platform"}))]
    name_translations: BTreeMap<String, String>,
    #[schema(example = "Identity and access services")]
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Resources in it, active or not.
    #[schema(example = 3)]
    member_count: i64,
    #[schema(example = 1)]
    version: i64,
}

/// A team a resource belongs to.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TeamRef)]
pub struct TeamRefView {
    id: Uuid,
    /// In the language of the request.
    #[schema(example = "Platform")]
    name: Option<String>,
}

/// The teams of the tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamList {
    /// By name, in the language of the request.
    items: Vec<TeamView>,
    /// The signed-in member creates and changes teams.
    can_manage: bool,
}

/// A team to create, or its new state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = TeamFields, example = json!({
    "nameTranslations": {"fr": "Plateforme", "en": "Platform"},
    "description": "Identity and access services"
}))]
pub struct TeamBody {
    /// One name per language (`fr`, `en`), at least one; 100 characters at
    /// most, unique in the tenant whatever the case.
    name_translations: BTreeMap<String, String>,
    /// Up to 1,000 characters.
    #[serde(default)]
    description: Option<String>,
}

impl TeamBody {
    fn checked(&self) -> Result<TeamInput, Problem> {
        let input = TeamInput {
            name_translations: self.name_translations.clone(),
            description: self.description.clone(),
        };
        validate(&input).map_err(Problem::validation)?;
        Ok(input)
    }
}

fn view(language: &Language, team: Team) -> TeamView {
    TeamView {
        id: team.id,
        name: language.label(&team.name_translations),
        name_translations: team.name_translations,
        description: team.description,
        member_count: team.member_count,
        version: team.version,
    }
}

/// The teams of each of `resource_ids`, by name in `language`.
pub(crate) async fn teams_of(
    tx: &mut TenantTx,
    language: &Language,
    resource_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<TeamRefView>>, Problem> {
    let mut teams: HashMap<Uuid, Vec<TeamRefView>> = HashMap::new();
    for (resource_id, team) in repo::of_resources(tx, resource_ids).await? {
        teams.entry(resource_id).or_default().push(TeamRefView {
            id: team.id,
            name: language.label(&team.name_translations),
        });
    }
    for refs in teams.values_mut() {
        refs.sort_by_key(|t| (t.name.as_deref().map(str::to_lowercase), t.id));
    }
    Ok(teams)
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::NameTaken => Problem::new(ErrorCode::TeamNameTaken)
            .with_errors(vec![FieldError::new("nameTranslations", "taken")]),
    }
}

fn respond(language: &Language, mut response: Response, version: Option<i64>) -> Response {
    language.headers(&mut response);
    if let Some(version) = version {
        response
            .headers_mut()
            .insert(axum::http::header::ETAG, etag(version));
    }
    response
}

/// `GET /api/v1/teams`.
#[utoipa::path(
    get,
    path = "/teams",
    tag = TAG,
    operation_id = "listTeams",
    summary = "List the teams of the tenant",
    description = "Any internal member. Every team, by name in the language of the request: a tenant \
                   has a few dozen, no pages.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of `name`")),
    responses(
        (status = 200, description = "The teams", body = TeamList, headers(
            ("Content-Language" = String, description = "Language of `name`"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(mut tx: TenantTx, accept: AcceptLanguage) -> Result<Response, Problem> {
    let language = Language::of(&tx, &accept);
    let mut items: Vec<TeamView> = repo::list(&mut tx)
        .await?
        .into_iter()
        .map(|team| view(&language, team))
        .collect();
    items.sort_by_key(|t| (t.name.as_deref().map(str::to_lowercase), t.id));
    let can_manage = manages_resources(tx.member().tenant_role);
    tx.commit().await?;
    Ok(respond(
        &language,
        ApiJson(TeamList { items, can_manage }).into_response(),
        None,
    ))
}

/// `POST /api/v1/teams`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/teams",
    tag = TAG,
    operation_id = "createTeam",
    summary = "Create a team",
    description = "Tenant admins and resource managers.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = TeamBody,
    responses(
        (status = 201, description = "Created", body = TeamView, headers(
            ("ETag" = String, description = "Version of the team"),
            ("Location" = String, description = "URL of the team"),
        )),
        (status = 400, description = "`validation_failed` (`nameTranslations`: `required`; \
                                      `nameTranslations.<locale>`: `too_long`, `unsupported_locale`; \
                                      `description`: `too_long`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`team.name_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<TeamBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = body.checked()?;
    let team = repo::insert(&mut tx, infra::id::new_id(), &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), team = %team.id, "team created");
    let language = Language::of(&tx, &accept);
    let (id, version) = (team.id, team.version);
    let view = view(&language, team);
    let response =
        idempotency::created(&mut tx, key.as_deref(), &view, format!("{PATH}/{id}"), version).await?;
    tx.commit().await?;
    Ok(respond(&language, response, None))
}

/// `GET /api/v1/teams/{id}`.
#[utoipa::path(
    get,
    path = "/teams/{id}",
    tag = TAG,
    operation_id = "getTeam",
    summary = "Read a team",
    description = "Any internal member. Its members: `GET /resources?team={id}`.",
    params(
        ("id" = Uuid, Path, description = "Id of the team"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "The team", body = TeamView, headers(
            ("ETag" = String, description = "Version of the team"),
            ("Content-Language" = String, description = "Language of `name`"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such team in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let team = repo::find(&mut tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound))?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let version = team.version;
    if if_none_match(&headers, version) {
        return Ok(respond(
            &language,
            StatusCode::NOT_MODIFIED.into_response(),
            Some(version),
        ));
    }
    Ok(respond(
        &language,
        ApiJson(view(&language, team)).into_response(),
        Some(version),
    ))
}

/// `PUT /api/v1/teams/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/teams/{id}",
    tag = TAG,
    operation_id = "updateTeam",
    summary = "Change a team's name and description",
    description = "Tenant admins and resource managers.",
    params(
        ("id" = Uuid, Path, description = "Id of the team"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = TeamBody,
    responses(
        (status = 200, description = "The team", body = TeamView, headers(
            ("ETag" = String, description = "New version of the team"),
        )),
        (status = 400, description = "`validation_failed`, as on creation", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such team in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`team.name_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The team changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<TeamBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = body.checked()?;
    let team = repo::update(&mut tx, id, if_match.versions(), &input)
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), team = %id, "team changed");
    let language = Language::of(&tx, &accept);
    let version = team.version;
    tx.commit().await?;
    Ok(respond(
        &language,
        ApiJson(view(&language, team)).into_response(),
        Some(version),
    ))
}

/// `DELETE /api/v1/teams/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/teams/{id}",
    tag = TAG,
    operation_id = "deleteTeam",
    summary = "Delete a team",
    description = "Tenant admins and resource managers. Its resources stay, with their assignments.",
    params(
        ("id" = Uuid, Path, description = "Id of the team"),
        ("If-Match" = String, Header, description = "ETag of the team, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such team in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The team changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_manager(&tx)?;
    let if_match = require_if_match(&headers)?;
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), team = %id, "team deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/teams/{id}/members/{resourceId}`.
#[utoipa::path(
    put,
    path = "/teams/{id}/members/{resourceId}",
    tag = TAG,
    operation_id = "addTeamMember",
    summary = "Add a resource to a team",
    description = "Tenant admins and resource managers. Adding a resource already in the team changes \
                   nothing; a resource may belong to several teams.",
    params(
        ("id" = Uuid, Path, description = "Id of the team"),
        ("resourceId" = Uuid, Path, description = "Id of a resource of the tenant"),
    ),
    responses(
        (status = 204, description = "In the team"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such team, or no such resource, in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn add_member(
    mut tx: TenantTx,
    ApiPath((id, resource_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_manager(&tx)?;
    match repo::add_member(&mut tx, id, resource_id).await? {
        MemberChange::Done => {
            tracing::info!(target: "audit", actor = %tx.account_id(), team = %id, resource = %resource_id, "team member added");
        }
        MemberChange::Unchanged => {}
        MemberChange::NoTeam => return Err(Problem::new(ErrorCode::NotFound).with_detail("no such team")),
        MemberChange::NoResource => {
            return Err(Problem::new(ErrorCode::NotFound).with_detail("no such resource"));
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/teams/{id}/members/{resourceId}`.
#[utoipa::path(
    delete,
    path = "/teams/{id}/members/{resourceId}",
    tag = TAG,
    operation_id = "removeTeamMember",
    summary = "Remove a resource from a team",
    description = "Tenant admins and resource managers. The resource keeps its assignments.",
    params(
        ("id" = Uuid, Path, description = "Id of the team"),
        ("resourceId" = Uuid, Path, description = "Id of the resource"),
    ),
    responses(
        (status = 204, description = "Out of the team"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such team in this tenant, or the resource is not in it", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn remove_member(
    mut tx: TenantTx,
    ApiPath((id, resource_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_manager(&tx)?;
    match repo::remove_member(&mut tx, id, resource_id).await? {
        MemberChange::Done => {
            tracing::info!(target: "audit", actor = %tx.account_id(), team = %id, resource = %resource_id, "team member removed");
        }
        MemberChange::Unchanged | MemberChange::NoResource => {
            return Err(Problem::new(ErrorCode::NotFound).with_detail("not in this team"));
        }
        MemberChange::NoTeam => return Err(Problem::new(ErrorCode::NotFound).with_detail("no such team")),
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
