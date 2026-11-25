//! Calendars (specification §7, §8, L07-US04): the tenant calendar, site
//! calendars ("Paris"), the dated exceptions of calendars and resources,
//! and the days of a resource as its calendar and capacity make them.
//!
//! Public holidays are computed for any year from the calendar's holiday
//! set; only exceptions are stored. Working weekdays come from the
//! capacity. Tenant admins and resource managers manage calendars and
//! exceptions; every internal member reads them.

use std::collections::{BTreeMap, HashMap};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::calendars::{self as rules, ExceptionKind, Source, Status, validate_exception};
use domain::holidays::HolidaySet;
use domain::resources::{manages_resources, trimmed};
use domain::validation::FieldError;
use infra::calendars::{
    self as repo, Calendar, CalendarInput, CalendarKind, ExceptionInput, Owner, StoredException,
    WriteConflict,
};
use infra::capacity::local_date;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::roles::Language;
use crate::tenant::TenantTx;

/// Longest range of days read at once.
const MAX_DAYS: i64 = 366;

fn require_manager(tx: &TenantTx) -> Result<(), Problem> {
    if manages_resources(tx.member().tenant_role) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden).with_detail("tenant admins and resource managers only"))
    }
}

/// A set of public holidays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
#[schema(as = HolidaySet)]
pub enum HolidaySetView {
    /// Metropolitan France.
    Fr,
    /// France, Alsace and Moselle (Good Friday, 26 December).
    FrAlsaceMoselle,
    /// No public holiday.
    None,
}

impl From<HolidaySetView> for HolidaySet {
    fn from(set: HolidaySetView) -> Self {
        match set {
            HolidaySetView::Fr => Self::France,
            HolidaySetView::FrAlsaceMoselle => Self::FranceAlsaceMoselle,
            HolidaySetView::None => Self::None,
        }
    }
}

impl From<HolidaySet> for HolidaySetView {
    fn from(set: HolidaySet) -> Self {
        match set {
            HolidaySet::France => Self::Fr,
            HolidaySet::FranceAlsaceMoselle => Self::FrAlsaceMoselle,
            HolidaySet::None => Self::None,
        }
    }
}

/// Kind of a calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CalendarKind)]
pub enum CalendarKindView {
    /// The organization's: every resource without a site.
    Tenant,
    /// A site's, which its resources use.
    Site,
}

/// A calendar.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Calendar)]
pub struct CalendarView {
    id: Uuid,
    kind: CalendarKindView,
    /// In the language of the request.
    #[schema(example = "Paris")]
    name: Option<String>,
    #[schema(example = json!({"fr": "Paris", "en": "Paris"}))]
    name_translations: BTreeMap<String, String>,
    /// Its public holidays; `null`: the organization's (a site).
    #[schema(required = true)]
    holiday_set: Option<HolidaySetView>,
    /// Resources of this site.
    #[schema(example = 12)]
    resource_count: i64,
    #[schema(example = 1)]
    version: i64,
}

fn view(language: &Language, calendar: Calendar) -> CalendarView {
    CalendarView {
        id: calendar.id,
        kind: match calendar.kind {
            CalendarKind::Tenant => CalendarKindView::Tenant,
            CalendarKind::Site => CalendarKindView::Site,
        },
        name: language.label(&calendar.name_translations),
        name_translations: calendar.name_translations,
        holiday_set: calendar.holiday_set.map(HolidaySetView::from),
        resource_count: calendar.resource_count,
        version: calendar.version,
    }
}

/// The site of a resource.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SiteRef)]
pub struct SiteRefView {
    id: Uuid,
    #[schema(example = "Paris")]
    name: Option<String>,
}

/// The site calendars of the tenant, by id, named in `language`.
pub(crate) async fn sites(
    tx: &mut TenantTx,
    language: &Language,
) -> Result<HashMap<Uuid, SiteRefView>, Problem> {
    Ok(repo::list(tx)
        .await?
        .into_iter()
        .filter(|c| c.kind == CalendarKind::Site)
        .map(|c| {
            (
                c.id,
                SiteRefView {
                    id: c.id,
                    name: language.label(&c.name_translations),
                },
            )
        })
        .collect())
}

/// The calendars of the tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CalendarList {
    /// The tenant's first, then the sites by creation.
    items: Vec<CalendarView>,
    /// The signed-in member manages calendars.
    can_manage: bool,
}

/// A site calendar to create, or a calendar's new state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = CalendarFields, example = json!({"nameTranslations": {"fr": "Strasbourg"}, "holidaySet": "fr-alsace-moselle"}))]
pub struct CalendarBody {
    /// One name per language, at least one; 100 characters at most, unique
    /// in the tenant.
    name_translations: BTreeMap<String, String>,
    /// Required for the tenant calendar; for a site, `null` uses the
    /// organization's.
    #[serde(default)]
    holiday_set: Option<HolidaySetView>,
}

impl CalendarBody {
    fn checked(&self) -> Result<CalendarInput, Problem> {
        domain::i18n::validate_translations(
            "nameTranslations",
            &self.name_translations,
            rules::NAME_MAX_CHARS,
        )
        .map_err(Problem::validation)?;
        Ok(CalendarInput {
            name_translations: self
                .name_translations
                .iter()
                .map(|(l, n)| (l.clone(), n.trim().to_owned()))
                .filter(|(_, n)| !n.is_empty())
                .collect(),
            holiday_set: self.holiday_set.map(HolidaySet::from),
        })
    }
}

/// What an exception makes of its day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CalendarExceptionKind)]
pub enum ExceptionKindView {
    /// A public holiday the set does not hold.
    Holiday,
    /// A closure, a bridge day.
    NonWorking,
    /// Worked anyway: cancels a holiday.
    Working,
}

impl From<ExceptionKindView> for ExceptionKind {
    fn from(kind: ExceptionKindView) -> Self {
        match kind {
            ExceptionKindView::Holiday => Self::Holiday,
            ExceptionKindView::NonWorking => Self::NonWorking,
            ExceptionKindView::Working => Self::Working,
        }
    }
}

impl From<ExceptionKind> for ExceptionKindView {
    fn from(kind: ExceptionKind) -> Self {
        match kind {
            ExceptionKind::Holiday => Self::Holiday,
            ExceptionKind::NonWorking => Self::NonWorking,
            ExceptionKind::Working => Self::Working,
        }
    }
}

/// A dated exception of a calendar or a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = CalendarException)]
pub struct ExceptionView {
    id: Uuid,
    /// A local date.
    #[schema(value_type = String, format = Date, example = "2026-12-24")]
    day: Date,
    kind: ExceptionKindView,
    #[schema(example = "Bridge day")]
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[schema(example = 1)]
    version: i64,
}

impl From<StoredException> for ExceptionView {
    fn from(e: StoredException) -> Self {
        Self {
            id: e.id,
            day: e.day,
            kind: e.kind.into(),
            label: e.label,
            version: e.version,
        }
    }
}

/// An exception to add, or its new state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = CalendarExceptionFields, example = json!({"day": "2026-12-24", "kind": "non_working", "label": "Bridge day"}))]
pub struct ExceptionBody {
    /// One exception per calendar (or resource) and day.
    #[schema(value_type = String, format = Date)]
    day: Date,
    kind: ExceptionKindView,
    /// Up to 100 characters.
    #[serde(default)]
    label: Option<String>,
}

impl ExceptionBody {
    fn checked(&self) -> Result<ExceptionInput<'_>, Problem> {
        validate_exception(self.label.as_deref()).map_err(Problem::validation)?;
        if local_date(self.day).is_none() {
            return Err(Problem::validation(vec![FieldError::new("day", "out_of_range")]));
        }
        Ok(ExceptionInput {
            day: self.day,
            kind: self.kind.into(),
            label: trimmed(self.label.as_deref()),
        })
    }
}

/// A public holiday of a calendar's year.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HolidayView {
    #[schema(value_type = String, format = Date, example = "2026-12-25")]
    day: Date,
    /// A stable key (`christmas`, `whit_monday`…), translated by the
    /// interface.
    #[schema(example = "christmas")]
    name: String,
}

/// A calendar's year: its holidays (its set, or the organization's for a
/// site without one) and its exceptions.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CalendarYear {
    #[schema(example = 2026)]
    year: i32,
    /// The set the holidays come from.
    holiday_set: HolidaySetView,
    /// The set is the organization's (a site without its own).
    inherited: bool,
    holidays: Vec<HolidayView>,
    /// By date.
    exceptions: Vec<ExceptionView>,
}

/// `?year=` of a calendar's year.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct YearParams {
    /// From 1970 to 2200.
    #[param(minimum = 1970, maximum = 2200)]
    pub year: i32,
}

/// `?from=&to=` of a range of days (366 at most).
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RangeParams {
    /// First day, a local date.
    #[param(value_type = String, format = Date)]
    pub from: Date,
    /// Last day, included.
    #[param(value_type = String, format = Date)]
    pub to: Date,
}

impl RangeParams {
    pub(crate) fn checked(&self) -> Result<(), Problem> {
        let days = (self.to - self.from).whole_days();
        if !(0..MAX_DAYS).contains(&days) {
            return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("from ≤ to, 366 days at most"));
        }
        Ok(())
    }
}

/// What a day of a resource is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = DayStatus)]
pub enum DayStatusView {
    /// Worked as the capacity says (0 minutes on a weekend).
    Working,
    Holiday,
    NonWorking,
}

/// Which level decided a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = DaySource)]
pub enum SourceView {
    Resource,
    Site,
    Tenant,
}

/// A day of a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceDay)]
pub struct DayView {
    #[schema(value_type = String, format = Date, example = "2026-12-25")]
    day: Date,
    status: DayStatusView,
    /// The level that decided it; `null`: the capacity alone.
    #[schema(required = true)]
    source: Option<SourceView>,
    /// Key of the public holiday (`christmas`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    holiday: Option<String>,
    /// Label of the exception.
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    /// The absence taking (part of) the day.
    #[serde(skip_serializing_if = "Option::is_none")]
    absence: Option<DayAbsenceView>,
    /// Capacity of the day: 0 when not worked, less an absence.
    #[schema(example = 420)]
    minutes: u16,
}

/// Which part of a day an absence takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = DayPart)]
pub enum DayPartView {
    Whole,
    Morning,
    Afternoon,
}

impl From<domain::absences::DayPart> for DayPartView {
    fn from(part: domain::absences::DayPart) -> Self {
        match part {
            domain::absences::DayPart::Whole => Self::Whole,
            domain::absences::DayPart::Morning => Self::Morning,
            domain::absences::DayPart::Afternoon => Self::Afternoon,
        }
    }
}

/// The status of a day, the key of its public holiday and its label.
pub(crate) fn status_view(status: Status) -> (DayStatusView, Option<String>, Option<String>) {
    match status {
        Status::Working => (DayStatusView::Working, None, None),
        Status::Holiday { name, label } => {
            (DayStatusView::Holiday, name.map(|n| n.as_str().to_owned()), label)
        }
        Status::NonWorking { label } => (DayStatusView::NonWorking, None, label),
    }
}

/// The absence of a day.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = DayAbsence)]
pub struct DayAbsenceView {
    kind: crate::absences::KindView,
    part: DayPartView,
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::NameTaken => Problem::new(ErrorCode::CalendarNameTaken)
            .with_errors(vec![FieldError::new("nameTranslations", "taken")]),
        WriteConflict::DayTaken => Problem::new(ErrorCode::CalendarExceptionDayTaken)
            .with_errors(vec![FieldError::new("day", "taken")]),
        WriteConflict::TenantCalendar => Problem::new(ErrorCode::CalendarTenant)
            .with_detail("the tenant calendar is never deleted and keeps a holiday set"),
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    response
}

async fn require_calendar(tx: &mut TenantTx, id: Uuid) -> Result<Calendar, Problem> {
    repo::find(tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such calendar"))
}

async fn require_resource(tx: &mut TenantTx, id: Uuid) -> Result<(), Problem> {
    infra::resources::find(tx, id)
        .await?
        .map(|_| ())
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such resource"))
}

/// `GET /api/v1/calendars`.
#[utoipa::path(
    get,
    path = "/calendars",
    tag = TAG,
    operation_id = "listCalendars",
    summary = "List the calendars of the tenant",
    description = "Any internal member: the tenant calendar first, then the site calendars.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of `name`")),
    responses(
        (status = 200, description = "The calendars", body = CalendarList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(mut tx: TenantTx, accept: AcceptLanguage) -> Result<Response, Problem> {
    let language = Language::of(&tx, &accept);
    let items = repo::list(&mut tx)
        .await?
        .into_iter()
        .map(|c| view(&language, c))
        .collect();
    let can_manage = manages_resources(tx.member().tenant_role);
    tx.commit().await?;
    let mut response = ApiJson(CalendarList { items, can_manage }).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/calendars`: a site calendar; idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/calendars",
    tag = TAG,
    operation_id = "createCalendar",
    summary = "Create a site calendar",
    description = "Tenant admins and resource managers. Its resources use its holidays and exceptions.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = CalendarBody,
    responses(
        (status = 201, description = "Created", body = CalendarView, headers(
            ("ETag" = String, description = "Version of the calendar"),
            ("Location" = String, description = "URL of the calendar"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar.name_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<CalendarBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, "POST /api/v1/calendars", &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = body.checked()?;
    let actor = tx.account_id();
    let calendar = repo::insert_site(&mut tx, infra::id::new_id(), &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, calendar = %calendar.id, "site calendar created");
    let language = Language::of(&tx, &accept);
    let (id, version) = (calendar.id, calendar.version);
    let view = view(&language, calendar);
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &view,
        format!("/api/v1/calendars/{id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `GET /api/v1/calendars/{id}`.
#[utoipa::path(
    get,
    path = "/calendars/{id}",
    tag = TAG,
    operation_id = "getCalendar",
    summary = "Read a calendar",
    description = "Any internal member.",
    params(
        ("id" = Uuid, Path, description = "Id of the calendar"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "The calendar", body = CalendarView, headers(("ETag" = String, description = "Version of the calendar"))),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let calendar = require_calendar(&mut tx, id).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let version = calendar.version;
    let mut response = ApiJson(view(&language, calendar)).into_response();
    language.headers(&mut response);
    Ok(with_etag(response, version))
}

/// `PUT /api/v1/calendars/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/calendars/{id}",
    tag = TAG,
    operation_id = "updateCalendar",
    summary = "Change a calendar's name and holidays",
    description = "Tenant admins and resource managers. The tenant calendar always keeps a holiday set.",
    params(
        ("id" = Uuid, Path, description = "Id of the calendar"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = CalendarBody,
    responses(
        (status = 200, description = "The calendar", body = CalendarView, headers(("ETag" = String, description = "New version"))),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar.name_taken`, or `calendar.tenant`: the tenant calendar without holiday set", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The calendar changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    headers: HeaderMap,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CalendarBody>,
) -> Result<Response, Problem> {
    require_manager(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = body.checked()?;
    let actor = tx.account_id();
    let calendar = repo::update(&mut tx, id, if_match.versions(), &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, calendar = %id, "calendar changed");
    let language = Language::of(&tx, &accept);
    let version = calendar.version;
    tx.commit().await?;
    let mut response = ApiJson(view(&language, calendar)).into_response();
    language.headers(&mut response);
    Ok(with_etag(response, version))
}

/// `DELETE /api/v1/calendars/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/calendars/{id}",
    tag = TAG,
    operation_id = "deleteCalendar",
    summary = "Delete a site calendar",
    description = "Tenant admins and resource managers. Its resources use the tenant calendar again; \
                   the tenant calendar is never deleted.",
    params(
        ("id" = Uuid, Path, description = "Id of the calendar"),
        ("If-Match" = String, Header, description = "ETag of the calendar, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar.tenant`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The calendar changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
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
    let actor = tx.account_id();
    repo::delete_site(&mut tx, id, if_match.versions(), Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, calendar = %id, "site calendar deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/calendars/{id}/year`.
#[utoipa::path(
    get,
    path = "/calendars/{id}/year",
    tag = TAG,
    operation_id = "getCalendarYear",
    summary = "Read a calendar's year",
    description = "Any internal member: the public holidays of the year (the calendar's set, or the \
                   organization's for a site without one) and the calendar's exceptions.",
    params(("id" = Uuid, Path, description = "Id of the calendar"), YearParams),
    responses(
        (status = 200, description = "The year", body = CalendarYear),
        (status = 400, description = "`invalid_query`: a year out of 1970–2200", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn year(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<YearParams>,
) -> Result<ApiJson<CalendarYear>, Problem> {
    if !(1970..=2200).contains(&params.year) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("year: 1970 to 2200"));
    }
    let calendar = require_calendar(&mut tx, id).await?;
    let (set, inherited) = match calendar.holiday_set {
        Some(set) => (set, false),
        None => (
            repo::tenant_calendar(&mut tx)
                .await?
                .and_then(|t| t.holiday_set)
                .unwrap_or(HolidaySet::France),
            true,
        ),
    };
    let bound = |month, day| {
        Date::from_calendar_date(params.year, month, day).map_err(|_| Problem::new(ErrorCode::InvalidQuery))
    };
    let (from, to) = (bound(time::Month::January, 1)?, bound(time::Month::December, 31)?);
    let exceptions = repo::exceptions(&mut tx, Owner::Calendar(id), from, to).await?;
    tx.commit().await?;
    let holidays = set
        .holidays(params.year)
        .into_iter()
        .filter_map(|h| {
            Some(HolidayView {
                day: infra::capacity::sql_date(h.date)?,
                name: h.name.as_str().to_owned(),
            })
        })
        .collect();
    Ok(ApiJson(CalendarYear {
        year: params.year,
        holiday_set: set.into(),
        inherited,
        holidays,
        exceptions: exceptions.into_iter().map(ExceptionView::from).collect(),
    }))
}

async fn create_exception(
    tx: &mut TenantTx,
    headers: &HeaderMap,
    owner: Owner,
    route: String,
    body: &ExceptionBody,
) -> Result<Response, Problem> {
    require_manager(tx)?;
    let key = match idempotency::start(tx, headers, &format!("POST {route}"), body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = body.checked()?;
    let actor = tx.account_id();
    let exception = repo::insert_exception(tx, infra::id::new_id(), owner, &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, exception = %exception.id, ?owner, "calendar exception added");
    let (id, version) = (exception.id, exception.version);
    let view = ExceptionView::from(exception);
    idempotency::created(tx, key.as_deref(), &view, format!("{route}/{id}"), version).await
}

async fn update_exception(
    tx: &mut TenantTx,
    headers: &HeaderMap,
    owner: Owner,
    id: Uuid,
    body: &ExceptionBody,
) -> Result<Response, Problem> {
    require_manager(tx)?;
    let if_match = require_if_match(headers)?;
    let input = body.checked()?;
    let actor = tx.account_id();
    let exception = repo::update_exception(tx, owner, id, if_match.versions(), &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, exception = %id, ?owner, "calendar exception changed");
    let version = exception.version;
    Ok(with_etag(
        ApiJson(ExceptionView::from(exception)).into_response(),
        version,
    ))
}

async fn delete_exception(
    tx: &mut TenantTx,
    headers: &HeaderMap,
    owner: Owner,
    id: Uuid,
) -> Result<StatusCode, Problem> {
    require_manager(tx)?;
    let if_match = require_if_match(headers)?;
    let actor = tx.account_id();
    repo::delete_exception(tx, owner, id, if_match.versions(), Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, exception = %id, ?owner, "calendar exception deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/calendars/{id}/exceptions`.
#[utoipa::path(
    post,
    path = "/calendars/{id}/exceptions",
    tag = TAG,
    operation_id = "createCalendarException",
    summary = "Add an exception to a calendar",
    description = "Tenant admins and resource managers: a holiday, a closure, or a day worked anyway \
                   (which cancels a holiday). One exception per calendar and day.",
    params(
        ("id" = Uuid, Path, description = "Id of the calendar"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation"),
    ),
    request_body = ExceptionBody,
    responses(
        (status = 201, description = "Added", body = ExceptionView),
        (status = 400, description = "`validation_failed` (`label`: `too_long`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar_exception.day_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create_calendar_exception(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ExceptionBody>,
) -> Result<Response, Problem> {
    let response = create_exception(
        &mut tx,
        &headers,
        Owner::Calendar(id),
        format!("/api/v1/calendars/{id}/exceptions"),
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `PUT /api/v1/calendars/{id}/exceptions/{exceptionId}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/calendars/{id}/exceptions/{exceptionId}",
    tag = TAG,
    operation_id = "updateCalendarException",
    summary = "Change an exception of a calendar",
    description = "Tenant admins and resource managers.",
    params(
        ("id" = Uuid, Path, description = "Id of the calendar"),
        ("exceptionId" = Uuid, Path, description = "Id of the exception"),
        ("If-Match" = String, Header, description = "ETag of the exception, or `*`"),
    ),
    request_body = ExceptionBody,
    responses(
        (status = 200, description = "The exception", body = ExceptionView),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar or exception", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar_exception.day_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The exception changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_calendar_exception(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, exception_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ExceptionBody>,
) -> Result<Response, Problem> {
    let response = update_exception(&mut tx, &headers, Owner::Calendar(id), exception_id, &body).await?;
    tx.commit().await?;
    Ok(response)
}

/// `DELETE /api/v1/calendars/{id}/exceptions/{exceptionId}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/calendars/{id}/exceptions/{exceptionId}",
    tag = TAG,
    operation_id = "deleteCalendarException",
    summary = "Delete an exception of a calendar",
    description = "Tenant admins and resource managers.",
    params(
        ("id" = Uuid, Path, description = "Id of the calendar"),
        ("exceptionId" = Uuid, Path, description = "Id of the exception"),
        ("If-Match" = String, Header, description = "ETag of the exception, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such calendar or exception", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The exception changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete_calendar_exception(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, exception_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    let status = delete_exception(&mut tx, &headers, Owner::Calendar(id), exception_id).await?;
    tx.commit().await?;
    Ok(status)
}

/// The exceptions of a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionList {
    /// By date.
    items: Vec<ExceptionView>,
    /// The signed-in member manages them.
    can_manage: bool,
}

/// `GET /api/v1/resources/{id}/exceptions`.
#[utoipa::path(
    get,
    path = "/resources/{id}/exceptions",
    tag = TAG,
    operation_id = "listResourceExceptions",
    summary = "List a resource's own calendar exceptions",
    description = "Any internal member, from `from` to `to` (366 days at most).",
    params(("id" = Uuid, Path, description = "Id of the resource"), RangeParams),
    responses(
        (status = 200, description = "The exceptions", body = ExceptionList),
        (status = 400, description = "`invalid_query`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn resource_exceptions(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(range): ApiQuery<RangeParams>,
) -> Result<ApiJson<ExceptionList>, Problem> {
    range.checked()?;
    require_resource(&mut tx, id).await?;
    let items = repo::exceptions(&mut tx, Owner::Resource(id), range.from, range.to)
        .await?
        .into_iter()
        .map(ExceptionView::from)
        .collect();
    let can_manage = manages_resources(tx.member().tenant_role);
    tx.commit().await?;
    Ok(ApiJson(ExceptionList { items, can_manage }))
}

/// `POST /api/v1/resources/{id}/exceptions`.
#[utoipa::path(
    post,
    path = "/resources/{id}/exceptions",
    tag = TAG,
    operation_id = "createResourceException",
    summary = "Add an exception to a resource's calendar",
    description = "Tenant admins and resource managers. It wins over its site's and the tenant's.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation"),
    ),
    request_body = ExceptionBody,
    responses(
        (status = 201, description = "Added", body = ExceptionView),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar_exception.day_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create_resource_exception(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ExceptionBody>,
) -> Result<Response, Problem> {
    let response = create_exception(
        &mut tx,
        &headers,
        Owner::Resource(id),
        format!("/api/v1/resources/{id}/exceptions"),
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `PUT /api/v1/resources/{id}/exceptions/{exceptionId}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/resources/{id}/exceptions/{exceptionId}",
    tag = TAG,
    operation_id = "updateResourceException",
    summary = "Change an exception of a resource",
    description = "Tenant admins and resource managers.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("exceptionId" = Uuid, Path, description = "Id of the exception"),
        ("If-Match" = String, Header, description = "ETag of the exception, or `*`"),
    ),
    request_body = ExceptionBody,
    responses(
        (status = 200, description = "The exception", body = ExceptionView),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource or exception", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`calendar_exception.day_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The exception changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_resource_exception(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, exception_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ExceptionBody>,
) -> Result<Response, Problem> {
    let response = update_exception(&mut tx, &headers, Owner::Resource(id), exception_id, &body).await?;
    tx.commit().await?;
    Ok(response)
}

/// `DELETE /api/v1/resources/{id}/exceptions/{exceptionId}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/resources/{id}/exceptions/{exceptionId}",
    tag = TAG,
    operation_id = "deleteResourceException",
    summary = "Delete an exception of a resource",
    description = "Tenant admins and resource managers.",
    params(
        ("id" = Uuid, Path, description = "Id of the resource"),
        ("exceptionId" = Uuid, Path, description = "Id of the exception"),
        ("If-Match" = String, Header, description = "ETag of the exception, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource or exception", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The exception changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete_resource_exception(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, exception_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    let status = delete_exception(&mut tx, &headers, Owner::Resource(id), exception_id).await?;
    tx.commit().await?;
    Ok(status)
}

/// `GET /api/v1/resources/{id}/days`.
#[utoipa::path(
    get,
    path = "/resources/{id}/days",
    tag = TAG,
    operation_id = "listResourceDays",
    summary = "Read the days of a resource",
    description = "Any internal member: for each day from `from` to `to` (366 days at most), whether \
                   the resource works it — its exceptions, then its site's, then the tenant's, and \
                   their public holidays —, its absences, and its capacity in minutes.",
    params(("id" = Uuid, Path, description = "Id of the resource"), RangeParams),
    responses(
        (status = 200, description = "The days", body = Vec<DayView>),
        (status = 400, description = "`invalid_query`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn resource_days(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(range): ApiQuery<RangeParams>,
) -> Result<ApiJson<Vec<DayView>>, Problem> {
    range.checked()?;
    let resource = infra::resources::find(&mut tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such resource"))?;
    let inputs =
        infra::day_capacity::load(&mut tx, &[(id, resource.calendar_id)], range.from, range.to).await?;
    tx.commit().await?;

    let (Some(from), Some(to)) = (local_date(range.from), local_date(range.to)) else {
        return Err(Problem::new(ErrorCode::InvalidQuery));
    };
    let levels = inputs.levels(id);
    let calendar = inputs
        .calendar(id, &levels)
        .ok_or_else(|| Problem::new(ErrorCode::NotFound))?;
    let days = calendar
        .days(from, to)
        .into_iter()
        .filter_map(|day| {
            let (status, holiday, label) = status_view(day.status);
            Some(DayView {
                day: infra::capacity::sql_date(day.date)?,
                status,
                source: day.source.map(|s| match s {
                    Source::Resource => SourceView::Resource,
                    Source::Site => SourceView::Site,
                    Source::Tenant => SourceView::Tenant,
                }),
                holiday,
                label,
                absence: day.absence.map(|(kind, part)| DayAbsenceView {
                    kind: kind.into(),
                    part: part.into(),
                }),
                minutes: day.minutes,
            })
        })
        .collect();
    Ok(ApiJson(days))
}
