//! What the capacity of resources depends on (specification §8, L07-US06),
//! loaded for many resources at once in a fixed number of set-based
//! queries: the tenant's default week, the tenant calendar and the site
//! calendars with their exceptions, and each resource's own exceptions,
//! capacity periods and absences over a range of days. The rule itself is
//! `domain::calendars::ResourceCalendar`. Row-level security scopes every
//! query to the current tenant.

use std::collections::HashMap;

use domain::absences::{Absence, AbsenceKind};
use domain::calendars::{Exception, ExceptionKind, Level, ResourceCalendar, Source};
use domain::capacity::{Period, WeekMinutes};
use domain::holidays::HolidaySet;
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::capacity::local_date;

/// The default week when the tenant has none stored: 7 h × 5 days.
pub const FALLBACK_WEEK: WeekMinutes = WeekMinutes([420, 420, 420, 420, 420, 0, 0]);

/// A calendar's holiday set and its exceptions in the range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalendarLevel {
    /// `None`: a site using the tenant's set.
    pub holidays: Option<HolidaySet>,
    pub exceptions: Vec<Exception>,
}

/// What belongs to one resource.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceInputs {
    /// Its site; `None`: the tenant calendar only.
    pub calendar_id: Option<Uuid>,
    pub exceptions: Vec<Exception>,
    pub periods: Vec<Period>,
    pub absences: Vec<Absence>,
}

/// Everything the capacity of the loaded resources depends on, over the
/// loaded range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityInputs {
    pub default: WeekMinutes,
    pub tenant: Option<CalendarLevel>,
    pub sites: HashMap<Uuid, CalendarLevel>,
    pub resources: HashMap<Uuid, ResourceInputs>,
}

impl CapacityInputs {
    /// The calendar levels of `resource_id`, closest first; empty when it
    /// was not loaded.
    pub fn levels(&self, resource_id: Uuid) -> Vec<(Source, Level<'_>)> {
        let Some(resource) = self.resources.get(&resource_id) else {
            return Vec::new();
        };
        let mut levels = vec![(
            Source::Resource,
            Level {
                exceptions: &resource.exceptions,
                holidays: None,
            },
        )];
        if let Some(site) = resource.calendar_id.and_then(|id| self.sites.get(&id)) {
            levels.push((
                Source::Site,
                Level {
                    exceptions: &site.exceptions,
                    holidays: site.holidays,
                },
            ));
        }
        if let Some(tenant) = &self.tenant {
            levels.push((
                Source::Tenant,
                Level {
                    exceptions: &tenant.exceptions,
                    holidays: tenant.holidays,
                },
            ));
        }
        levels
    }

    /// The calendar of `resource_id` over `levels` (from [`Self::levels`]);
    /// `None` when it was not loaded.
    pub fn calendar<'a>(
        &'a self,
        resource_id: Uuid,
        levels: &'a [(Source, Level<'a>)],
    ) -> Option<ResourceCalendar<'a>> {
        let resource = self.resources.get(&resource_id)?;
        Some(ResourceCalendar {
            levels,
            periods: &resource.periods,
            default: self.default,
            absences: &resource.absences,
        })
    }
}

/// Loads what the capacity of `resources` (id and site of each) depends on
/// from `from` to `to`, in six queries whatever their number.
pub async fn load(
    conn: &mut PgConnection,
    resources: &[(Uuid, Option<Uuid>)],
    from: Date,
    to: Date,
) -> Result<CapacityInputs, sqlx::Error> {
    let ids: Vec<Uuid> = resources.iter().map(|(id, _)| *id).collect();
    let mut site_ids: Vec<Uuid> = resources.iter().filter_map(|(_, site)| *site).collect();
    site_ids.sort_unstable();
    site_ids.dedup();

    let default =
        sqlx::query_scalar!("SELECT default_capacity_minutes FROM tenant WHERE id = app_current_tenant()")
            .fetch_optional(&mut *conn)
            .await?
            .and_then(|minutes| WeekMinutes::from_stored(&minutes))
            .unwrap_or(FALLBACK_WEEK);

    // The tenant calendar and the sites, with their exceptions.
    let calendars = sqlx::query!(
        "SELECT id, kind, holiday_set FROM calendar WHERE kind = 'tenant' OR id = ANY($1)",
        &site_ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let calendar_ids: Vec<Uuid> = calendars.iter().map(|c| c.id).collect();
    let calendar_exceptions = sqlx::query!(
        r#"
        SELECT calendar_id AS "calendar_id!", day, kind, label
        FROM calendar_exception
        WHERE calendar_id = ANY($1) AND day BETWEEN $2 AND $3
        ORDER BY day
        "#,
        &calendar_ids,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut levels: HashMap<Uuid, CalendarLevel> = calendars
        .iter()
        .map(|c| {
            (
                c.id,
                CalendarLevel {
                    holidays: c.holiday_set.as_deref().and_then(HolidaySet::parse),
                    exceptions: Vec::new(),
                },
            )
        })
        .collect();
    for row in calendar_exceptions {
        if let (Some(level), Some(exception)) = (
            levels.get_mut(&row.calendar_id),
            exception(row.day, &row.kind, row.label),
        ) {
            level.exceptions.push(exception);
        }
    }
    let tenant = calendars
        .iter()
        .find(|c| c.kind == "tenant")
        .and_then(|c| levels.remove(&c.id));

    let mut inputs: HashMap<Uuid, ResourceInputs> = resources
        .iter()
        .map(|(id, site)| {
            (
                *id,
                ResourceInputs {
                    calendar_id: *site,
                    ..ResourceInputs::default()
                },
            )
        })
        .collect();

    let own = sqlx::query!(
        r#"
        SELECT resource_id AS "resource_id!", day, kind, label
        FROM calendar_exception
        WHERE resource_id = ANY($1) AND day BETWEEN $2 AND $3
        ORDER BY day
        "#,
        &ids,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in own {
        if let (Some(resource), Some(exception)) = (
            inputs.get_mut(&row.resource_id),
            exception(row.day, &row.kind, row.label),
        ) {
            resource.exceptions.push(exception);
        }
    }

    // Every period: the one in force on `from` may start long before it.
    let periods = sqlx::query!(
        r#"
        SELECT resource_id, effective_from, minutes
        FROM capacity_pattern
        WHERE resource_id = ANY($1) AND effective_from <= $2
        ORDER BY effective_from
        "#,
        &ids,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in periods {
        let period = local_date(row.effective_from).and_then(|effective_from| {
            Some(Period {
                effective_from,
                week: WeekMinutes::from_stored(&row.minutes)?,
            })
        });
        if let (Some(resource), Some(period)) = (inputs.get_mut(&row.resource_id), period) {
            resource.periods.push(period);
        }
    }

    let absences = sqlx::query!(
        r#"
        SELECT resource_id, kind, start_date, starts_at_noon, end_date, ends_at_noon
        FROM absence
        WHERE resource_id = ANY($1) AND start_date <= $3 AND end_date >= $2
        ORDER BY start_date, id
        "#,
        &ids,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in absences {
        let absence = (|| {
            Some(Absence {
                kind: AbsenceKind::parse(&row.kind)?,
                start: local_date(row.start_date)?,
                starts_at_noon: row.starts_at_noon,
                end: local_date(row.end_date)?,
                ends_at_noon: row.ends_at_noon,
            })
        })();
        if let (Some(resource), Some(absence)) = (inputs.get_mut(&row.resource_id), absence) {
            resource.absences.push(absence);
        }
    }

    Ok(CapacityInputs {
        default,
        tenant,
        sites: levels,
        resources: inputs,
    })
}

fn exception(day: Date, kind: &str, label: Option<String>) -> Option<Exception> {
    Some(Exception {
        date: local_date(day)?,
        kind: ExceptionKind::parse(kind)?,
        label,
    })
}
