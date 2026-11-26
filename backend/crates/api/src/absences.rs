//! Absences (specification §8, L07-US05): leave, training or other time off
//! of a resource, by whole or half days. The person linked to the resource
//! declares their own; tenant admins and resource managers declare anyone's
//! and are recorded as the declarer. Every internal member sees the dates
//! and kind; the comment is for the person and the managers only.

use std::collections::BTreeMap;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::absences::{Absence, AbsenceKind, manages, validate};
use domain::resources::trimmed;
use domain::validation::FieldError;
use infra::absences::{self as repo, AbsenceInput, StoredAbsence, WriteConflict};
use infra::capacity::local_date;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::ToSchema;

use crate::calendars::RangeParams;
use crate::etag::{etag, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::tenant::TenantTx;

/// Kind of an absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = AbsenceKind)]
pub enum KindView {
    Leave,
    Training,
    Other,
}

impl From<KindView> for AbsenceKind {
    fn from(kind: KindView) -> Self {
        match kind {
            KindView::Leave => Self::Leave,
            KindView::Training => Self::Training,
            KindView::Other => Self::Other,
        }
    }
}

impl From<AbsenceKind> for KindView {
    fn from(kind: AbsenceKind) -> Self {
        match kind {
            AbsenceKind::Leave => Self::Leave,
            AbsenceKind::Training => Self::Training,
            AbsenceKind::Other => Self::Other,
        }
    }
}

/// Who declared an absence.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AbsenceDeclarer)]
pub struct DeclarerView {
    account_id: Uuid,
    #[schema(example = "Lucas Bernard")]
    display_name: String,
}

/// An absence.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Absence)]
pub struct AbsenceView {
    id: Uuid,
    kind: KindView,
    /// First day off, a local date.
    #[schema(value_type = String, format = Date, example = "2026-12-04")]
    start: Date,
    /// The first day's morning is worked.
    starts_at_noon: bool,
    /// Last day off, included.
    #[schema(value_type = String, format = Date, example = "2026-12-04")]
    end: Date,
    /// The last day's afternoon is worked.
    ends_at_noon: bool,
    /// For the person and the managers only; absent for others.
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    /// `null`: the account no longer exists.
    #[schema(required = true)]
    declared_by: Option<DeclarerView>,
    /// Minutes planned in hours per day or total effort, all tickets
    /// together, that it leaves no room for (L08-US04): to reschedule.
    /// Kept up to date by the worker within seconds of a change.
    #[schema(example = 420)]
    conflict_minutes: u32,
    #[schema(example = 1)]
    version: i64,
}

/// The conflict minutes of the days of `absence`, from those of each day.
fn conflict_minutes(absence: &StoredAbsence, by_day: &BTreeMap<Date, u32>) -> u32 {
    by_day
        .range(absence.start..=absence.end)
        .map(|(_, minutes)| *minutes)
        .sum()
}

/// The conflict minutes of `absence` as they stand: the worker updates
/// them within seconds of a change.
async fn current_conflicts(
    tx: &mut TenantTx,
    resource_id: Uuid,
    absence: &StoredAbsence,
) -> Result<u32, Problem> {
    let by_day = infra::conflicts::minutes_by_day(tx, resource_id, absence.start, absence.end).await?;
    Ok(conflict_minutes(absence, &by_day))
}

impl AbsenceView {
    fn of(absence: StoredAbsence, sees_comment: bool, conflict_minutes: u32) -> Self {
        Self {
            id: absence.id,
            kind: absence.kind.into(),
            start: absence.start,
            starts_at_noon: absence.starts_at_noon,
            end: absence.end,
            ends_at_noon: absence.ends_at_noon,
            comment: absence.comment.filter(|_| sees_comment),
            declared_by: absence.declared_by.map(|account_id| DeclarerView {
                account_id,
                display_name: absence.declared_by_name.unwrap_or_default(),
            }),
            conflict_minutes,
            version: absence.version,
        }
    }
}

/// The absences of a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AbsenceList {
    /// By start date.
    items: Vec<AbsenceView>,
    /// The signed-in member declares and changes them: the person, or a
    /// manager.
    can_manage: bool,
}

/// An absence to declare, or its new state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AbsenceFields, example = json!({
    "kind": "training", "start": "2026-12-07", "startsAtNoon": true, "end": "2026-12-07", "endsAtNoon": false,
    "comment": "Rust training"
}))]
pub struct AbsenceBody {
    kind: KindView,
    #[schema(value_type = String, format = Date)]
    start: Date,
    #[serde(default)]
    starts_at_noon: bool,
    /// On or after `start`, 366 days at most.
    #[schema(value_type = String, format = Date)]
    end: Date,
    #[serde(default)]
    ends_at_noon: bool,
    /// Up to 500 characters; read by the person and the managers only.
    #[serde(default)]
    comment: Option<String>,
}

impl AbsenceBody {
    fn checked(&self) -> Result<AbsenceInput<'_>, Problem> {
        let (Some(start), Some(end)) = (local_date(self.start), local_date(self.end)) else {
            return Err(Problem::validation(vec![FieldError::new(
                "start",
                "out_of_range",
            )]));
        };
        let absence = Absence {
            kind: self.kind.into(),
            start,
            starts_at_noon: self.starts_at_noon,
            end,
            ends_at_noon: self.ends_at_noon,
        };
        validate(&absence, self.comment.as_deref()).map_err(Problem::validation)?;
        Ok(AbsenceInput {
            kind: absence.kind,
            start: self.start,
            starts_at_noon: self.starts_at_noon,
            end: self.end,
            ends_at_noon: self.ends_at_noon,
            comment: trimmed(self.comment.as_deref()),
        })
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::Overlap => Problem::new(ErrorCode::AbsenceOverlap)
            .with_detail("another absence of the resource takes a same half day")
            .with_errors(vec![FieldError::new("start", "overlap")]),
    }
}

/// Whether the signed-in member manages the absences of `resource_id`;
/// 404 when there is no such resource.
async fn rights(tx: &mut TenantTx, resource_id: Uuid) -> Result<bool, Problem> {
    let person = repo::person_of(tx, resource_id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such resource"))?;
    Ok(manages(person == Some(tx.account_id()), tx.member().tenant_role))
}

async fn require_rights(tx: &mut TenantTx, resource_id: Uuid) -> Result<(), Problem> {
    if rights(tx, resource_id).await? {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden)
            .with_detail("the person, tenant admins and resource managers only"))
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

/// `GET /api/v1/resources/{id}/absences`.
#[utoipa::path(
    get,
    path = "/resources/{id}/absences",
    tag = TAG,
    operation_id = "listAbsences",
    summary = "List the absences of a resource",
    description = "Any internal member: the absences meeting `from`–`to` (366 days at most). The \
                   comment only shows to the person and the managers. Each tells the hours planned \
                   on its days that it leaves no room for (`conflictMinutes`), all tickets together.",
    params(("id" = Uuid, Path, description = "Id of the resource"), RangeParams),
    responses(
        (status = 200, description = "The absences", body = AbsenceList),
        (status = 400, description = "`invalid_query`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(range): ApiQuery<RangeParams>,
) -> Result<ApiJson<AbsenceList>, Problem> {
    range.checked()?;
    let can_manage = rights(&mut tx, id).await?;
    let absences = repo::list(&mut tx, id, range.from, range.to).await?;
    let by_day = match (
        absences.iter().map(|a| a.start).min(),
        absences.iter().map(|a| a.end).max(),
    ) {
        (Some(first), Some(last)) => infra::conflicts::minutes_by_day(&mut tx, id, first, last).await?,
        _ => BTreeMap::new(),
    };
    let items = absences
        .into_iter()
        .map(|a| {
            let minutes = conflict_minutes(&a, &by_day);
            AbsenceView::of(a, can_manage, minutes)
        })
        .collect();
    tx.commit().await?;
    Ok(ApiJson(AbsenceList { items, can_manage }))
}

/// `POST /api/v1/resources/{id}/absences`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/resources/{id}/absences",
    tag = TAG,
    operation_id = "createAbsence",
    summary = "Declare an absence",
    description = "The person linked to the resource, tenant admins and resource managers; the \
                   signed-in member is recorded as the declarer. Absences of a resource never take a \
                   same half day.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation"),
    ),
    request_body = AbsenceBody,
    responses(
        (status = 201, description = "Declared", body = AbsenceView, headers(
            ("ETag" = String, description = "Version of the absence"),
            ("Location" = String, description = "URL of the absence"),
        )),
        (status = 400, description = "`validation_failed` (`end`: `before_start`, `out_of_range`; `endsAtNoon`: \
                                      `not_allowed`; `comment`: `too_long`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither the person nor a manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`absence.overlap`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AbsenceBody>,
) -> Result<Response, Problem> {
    require_rights(&mut tx, id).await?;
    let route = format!("/api/v1/resources/{id}/absences");
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {route}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = body.checked()?;
    let actor = tx.account_id();
    let absence = repo::insert(&mut tx, infra::id::new_id(), id, &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, resource = %id, absence = %absence.id, "absence declared");
    let (absence_id, version) = (absence.id, absence.version);
    let minutes = current_conflicts(&mut tx, id, &absence).await?;
    let view = AbsenceView::of(absence, true, minutes);
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &view,
        format!("{route}/{absence_id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `PUT /api/v1/resources/{id}/absences/{absenceId}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/resources/{id}/absences/{absenceId}",
    tag = TAG,
    operation_id = "updateAbsence",
    summary = "Change an absence",
    description = "The person and the managers; the signed-in member becomes its declarer.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("absenceId" = Uuid, Path, description = "Id of the absence"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = AbsenceBody,
    responses(
        (status = 200, description = "The absence", body = AbsenceView, headers(("ETag" = String, description = "New version"))),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource or absence", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`absence.overlap`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The absence changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, absence_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<AbsenceBody>,
) -> Result<Response, Problem> {
    require_rights(&mut tx, id).await?;
    let if_match = require_if_match(&headers)?;
    let input = body.checked()?;
    let actor = tx.account_id();
    let absence = repo::update(&mut tx, id, absence_id, if_match.versions(), &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, resource = %id, absence = %absence_id, "absence changed");
    let version = absence.version;
    let minutes = current_conflicts(&mut tx, id, &absence).await?;
    tx.commit().await?;
    Ok(with_etag(
        ApiJson(AbsenceView::of(absence, true, minutes)).into_response(),
        version,
    ))
}

/// `DELETE /api/v1/resources/{id}/absences/{absenceId}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/resources/{id}/absences/{absenceId}",
    tag = TAG,
    operation_id = "deleteAbsence",
    summary = "Delete an absence",
    description = "The person and the managers.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("absenceId" = Uuid, Path, description = "Id of the absence"),
        ("If-Match" = String, Header, description = "ETag of the absence, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource or absence", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The absence changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, absence_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_rights(&mut tx, id).await?;
    let if_match = require_if_match(&headers)?;
    let actor = tx.account_id();
    repo::delete(&mut tx, id, absence_id, if_match.versions(), Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, resource = %id, absence = %absence_id, "absence deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
