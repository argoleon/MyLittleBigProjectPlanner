//! Capacity of resources (specification §8, L07-US03): periods of minutes
//! per weekday, historized by effective date, and the tenant's workload
//! settings (default capacity, overload tolerance and mode).
//!
//! Tenant admins and resource managers manage periods; every internal
//! member reads them. Only tenant admins change the workload settings.
//! Every change is recorded in the outbox (`resource.capacity_changed`,
//! `tenant.capacity_changed`) for the load engine (L08).

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::capacity::{WeekMinutes, validate};
use domain::resources::{manages_resources, trimmed};
use domain::tenant::{WorkloadSettingsInput, validate_workload_settings};
use domain::validation::FieldError;
use infra::capacity::{self as repo, CapacityPattern, PatternInput, WorkloadSettings, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::ToSchema;

use crate::etag::{etag, require_if_match};
use crate::extract::{ApiJson, ApiPath};
use crate::idempotency::{self, Start};
use crate::members::require_member_admin;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::tenant::TenantTx;

fn require_manager(tx: &TenantTx) -> Result<(), Problem> {
    if manages_resources(tx.member().tenant_role) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden).with_detail("tenant admins and resource managers only"))
    }
}

/// A capacity period of a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CapacityPattern)]
pub struct PatternView {
    id: Uuid,
    /// First day it applies (a local date); it applies until the next
    /// period.
    #[schema(value_type = String, format = Date, example = "2026-03-01")]
    effective_from: Date,
    /// Minutes per weekday, Monday first.
    #[schema(example = json!([420, 420, 0, 420, 420, 0, 0]))]
    minutes: Vec<u16>,
    /// Minutes of the whole week.
    #[schema(example = 1680)]
    weekly_minutes: u32,
    #[schema(example = "Part time 80%")]
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[schema(example = 1)]
    version: i64,
}

impl From<CapacityPattern> for PatternView {
    fn from(pattern: CapacityPattern) -> Self {
        Self {
            id: pattern.id,
            effective_from: pattern.effective_from,
            minutes: pattern.week.0.to_vec(),
            weekly_minutes: pattern.week.total(),
            label: pattern.label,
            version: pattern.version,
        }
    }
}

/// The capacity periods of a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapacityPatternList {
    /// By effective date.
    items: Vec<PatternView>,
    /// The tenant's default minutes per weekday, Monday first: what applies
    /// before the first period, or without any.
    #[schema(example = json!([420, 420, 420, 420, 420, 0, 0]))]
    default_minutes: Vec<u16>,
    /// The signed-in member manages the periods.
    can_manage: bool,
}

/// A period to add, or its new state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = CapacityPatternFields, example = json!({
    "effectiveFrom": "2026-03-01",
    "minutes": [420, 420, 0, 420, 420, 0, 0],
    "label": "Part time 80%"
}))]
pub struct PatternBody {
    /// First day it applies; one period per resource and date.
    #[schema(value_type = String, format = Date)]
    effective_from: Date,
    /// Seven values, Monday first, from 0 to 1,440 minutes.
    minutes: Vec<i64>,
    /// Up to 100 characters.
    #[serde(default)]
    label: Option<String>,
}

impl PatternBody {
    fn checked(&self) -> Result<PatternInput<'_>, Problem> {
        validate(&self.minutes, self.label.as_deref()).map_err(Problem::validation)?;
        if repo::local_date(self.effective_from).is_none() {
            return Err(Problem::validation(vec![FieldError::new(
                "effectiveFrom",
                "out_of_range",
            )]));
        }
        let minutes: Vec<i32> = self
            .minutes
            .iter()
            .map(|m| i32::try_from(*m).unwrap_or(0))
            .collect();
        let week = WeekMinutes::from_stored(&minutes)
            .ok_or_else(|| Problem::validation(vec![FieldError::new("minutes", "out_of_range")]))?;
        Ok(PatternInput {
            effective_from: self.effective_from,
            week,
            label: trimmed(self.label.as_deref()),
        })
    }
}

/// The workload settings of the tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadSettings)]
pub struct SettingsView {
    /// Minutes per weekday, Monday first, of resources without a capacity
    /// period.
    #[schema(example = json!([420, 420, 420, 420, 420, 0, 0]))]
    default_capacity_minutes: Vec<u16>,
    /// Load accepted above capacity, in percent, before a day is overloaded.
    #[schema(example = 0)]
    overload_tolerance_percent: i16,
    /// `blocking`: an overloading assignment is refused; `warning`: it is
    /// accepted and flagged (L08).
    #[schema(example = "blocking")]
    overload_mode: String,
    /// Provisional assignments count in the overload check (L08-US03).
    #[schema(example = true)]
    overload_counts_provisional: bool,
    /// Version of the tenant, for `If-Match`.
    #[schema(example = 3)]
    version: i64,
}

impl From<WorkloadSettings> for SettingsView {
    fn from(settings: WorkloadSettings) -> Self {
        Self {
            default_capacity_minutes: settings.default_capacity.0.to_vec(),
            overload_tolerance_percent: settings.overload_tolerance_percent,
            overload_mode: settings.overload_mode,
            overload_counts_provisional: settings.overload_counts_provisional,
            version: settings.version,
        }
    }
}

/// New workload settings.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = WorkloadSettingsFields, example = json!({
    "defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0],
    "overloadTolerancePercent": 10,
    "overloadMode": "blocking",
    "overloadCountsProvisional": true
}))]
pub struct SettingsBody {
    /// Seven values, Monday first, from 0 to 1,440 minutes.
    default_capacity_minutes: Vec<i64>,
    /// From 0 to 100.
    overload_tolerance_percent: i64,
    /// `blocking` or `warning`.
    overload_mode: String,
    /// Whether provisional assignments count in the overload check; left
    /// out: unchanged.
    #[serde(default)]
    overload_counts_provisional: Option<bool>,
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::NoResource => Problem::new(ErrorCode::NotFound).with_detail("no such resource"),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::DateTaken => Problem::new(ErrorCode::CapacityPatternDateTaken)
            .with_detail("the resource has another period starting that day")
            .with_errors(vec![FieldError::new("effectiveFrom", "taken")]),
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

async fn require_resource(tx: &mut TenantTx, id: Uuid) -> Result<(), Problem> {
    infra::resources::find(tx, id)
        .await?
        .map(|_| ())
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such resource"))
}

/// `GET /api/v1/resources/{id}/capacity-patterns`.
#[utoipa::path(
    get,
    path = "/resources/{id}/capacity-patterns",
    tag = TAG,
    operation_id = "listCapacityPatterns",
    summary = "List the capacity periods of a resource",
    description = "Any internal member. By effective date, with the tenant's default capacity, which \
                   applies before the first period or without any.",
    params(("id" = Uuid, Path, description = "Id of the resource")),
    responses(
        (status = 200, description = "The periods", body = CapacityPatternList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<ApiJson<CapacityPatternList>, Problem> {
    require_resource(&mut tx, id).await?;
    let items = repo::list(&mut tx, id)
        .await?
        .into_iter()
        .map(PatternView::from)
        .collect();
    let default_minutes = repo::workload_settings(&mut tx)
        .await?
        .map(|s| s.default_capacity.0.to_vec())
        .unwrap_or_default();
    let can_manage = manages_resources(tx.member().tenant_role);
    tx.commit().await?;
    Ok(ApiJson(CapacityPatternList {
        items,
        default_minutes,
        can_manage,
    }))
}

/// `POST /api/v1/resources/{id}/capacity-patterns`: idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/resources/{id}/capacity-patterns",
    tag = TAG,
    operation_id = "createCapacityPattern",
    summary = "Add a capacity period to a resource",
    description = "Tenant admins and resource managers. It applies from its date until the next period.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
    ),
    request_body = PatternBody,
    responses(
        (status = 201, description = "Added", body = PatternView, headers(
            ("ETag" = String, description = "Version of the period"),
            ("Location" = String, description = "URL of the period"),
        )),
        (status = 400, description = "`validation_failed` (`minutes`: `length`; `minutes.<day>`: `out_of_range`; \
                                      `label`: `too_long`; `effectiveFrom`: `out_of_range`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`capacity_pattern.date_taken`: another period starts that day; or \
                                      `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<PatternBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let route = format!("/api/v1/resources/{id}/capacity-patterns");
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {route}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = body.checked()?;
    let actor = tx.account_id();
    let pattern = repo::insert(&mut tx, infra::id::new_id(), id, &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, resource = %id, pattern = %pattern.id, "capacity period added");
    let (pattern_id, version) = (pattern.id, pattern.version);
    let view = PatternView::from(pattern);
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &view,
        format!("{route}/{pattern_id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `PUT /api/v1/resources/{id}/capacity-patterns/{patternId}`: requires
/// `If-Match`.
#[utoipa::path(
    put,
    path = "/resources/{id}/capacity-patterns/{patternId}",
    tag = TAG,
    operation_id = "updateCapacityPattern",
    summary = "Change a capacity period",
    description = "Tenant admins and resource managers: its date, minutes and label, all replaced.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("patternId" = Uuid, Path, description = "Id of the period"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = PatternBody,
    responses(
        (status = 200, description = "The period", body = PatternView, headers(
            ("ETag" = String, description = "New version of the period"),
        )),
        (status = 400, description = "`validation_failed`, as on creation", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource or period in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`capacity_pattern.date_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The period changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, pattern_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<PatternBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = body.checked()?;
    let actor = tx.account_id();
    let pattern = repo::update(&mut tx, id, pattern_id, if_match.versions(), &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, resource = %id, pattern = %pattern_id, "capacity period changed");
    let version = pattern.version;
    tx.commit().await?;
    Ok(with_etag(
        ApiJson(PatternView::from(pattern)).into_response(),
        version,
    ))
}

/// `DELETE /api/v1/resources/{id}/capacity-patterns/{patternId}`: requires
/// `If-Match`.
#[utoipa::path(
    delete,
    path = "/resources/{id}/capacity-patterns/{patternId}",
    tag = TAG,
    operation_id = "deleteCapacityPattern",
    summary = "Delete a capacity period",
    description = "Tenant admins and resource managers. The previous period, or the tenant's default, \
                   then applies to its days.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("patternId" = Uuid, Path, description = "Id of the period"),
        ("If-Match" = String, Header, description = "ETag of the period, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource or period in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The period changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, pattern_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_manager(&tx)?;
    let if_match = require_if_match(&headers)?;
    let actor = tx.account_id();
    repo::delete(&mut tx, id, pattern_id, if_match.versions(), Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, resource = %id, pattern = %pattern_id, "capacity period deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/tenant/workload-settings`.
#[utoipa::path(
    get,
    path = "/tenant/workload-settings",
    tag = TAG,
    operation_id = "getWorkloadSettings",
    summary = "Read the workload settings of the tenant",
    description = "Tenant admins only: default capacity, overload tolerance and mode, and whether \
                   provisional assignments count in the overload check.",
    responses(
        (status = 200, description = "The settings", body = SettingsView, headers(
            ("ETag" = String, description = "Version of the tenant"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn settings(mut tx: TenantTx) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let settings = repo::workload_settings(&mut tx)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound))?;
    tx.commit().await?;
    let version = settings.version;
    Ok(with_etag(
        ApiJson(SettingsView::from(settings)).into_response(),
        version,
    ))
}

/// `PUT /api/v1/tenant/workload-settings`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/tenant/workload-settings",
    tag = TAG,
    operation_id = "updateWorkloadSettings",
    summary = "Change the workload settings of the tenant",
    description = "Tenant admins only. A new default capacity applies at once to every resource \
                   without a capacity period, and to the days before their first one.",
    params(("If-Match" = String, Header, description = "ETag of the settings, or `*`")),
    request_body = SettingsBody,
    responses(
        (status = 200, description = "The settings", body = SettingsView, headers(
            ("ETag" = String, description = "New version of the tenant"),
        )),
        (status = 400, description = "`validation_failed` (`defaultCapacityMinutes`: `length`; \
                                      `defaultCapacityMinutes.<day>`, `overloadTolerancePercent`: `out_of_range`; \
                                      `overloadMode`: `format`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The settings changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_settings(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiJson(body): ApiJson<SettingsBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = WorkloadSettingsInput {
        default_capacity_minutes: body.default_capacity_minutes.clone(),
        overload_tolerance_percent: body.overload_tolerance_percent,
        overload_mode: body.overload_mode.clone(),
    };
    validate_workload_settings(&input).map_err(Problem::validation)?;
    let counts_provisional = match body.overload_counts_provisional {
        Some(counts) => counts,
        None => repo::workload_settings(&mut tx)
            .await?
            .is_none_or(|current| current.overload_counts_provisional),
    };
    let minutes: Vec<i32> = input
        .default_capacity_minutes
        .iter()
        .map(|m| i32::try_from(*m).unwrap_or(0))
        .collect();
    let settings = WorkloadSettings {
        default_capacity: WeekMinutes::from_stored(&minutes).ok_or_else(|| {
            Problem::validation(vec![FieldError::new("defaultCapacityMinutes", "out_of_range")])
        })?,
        overload_tolerance_percent: i16::try_from(input.overload_tolerance_percent).unwrap_or(0),
        overload_mode: input.overload_mode,
        overload_counts_provisional: counts_provisional,
        version: 0,
    };
    let actor = tx.account_id();
    let updated = repo::update_workload_settings(&mut tx, actor, if_match.versions(), &settings)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::PreconditionFailed))?;
    tracing::info!(target: "audit", actor = %actor, tolerance = updated.overload_tolerance_percent,
                   mode = %updated.overload_mode, provisional = updated.overload_counts_provisional,
                   "workload settings changed");
    let version = updated.version;
    tx.commit().await?;
    Ok(with_etag(
        ApiJson(SettingsView::from(updated)).into_response(),
        version,
    ))
}
