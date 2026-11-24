//! Calendars (specification §7, §8, L07-US04): which days a resource works.
//!
//! - Working weekdays come from the capacity (`crate::capacity`): a day of
//!   0 minutes is not worked. Calendars only hold dated exceptions and a
//!   set of public holidays (`crate::holidays`), computed for any year.
//! - Three levels, the closest one deciding: the resource's own
//!   exceptions, then its site (a named calendar), then the tenant's
//!   calendar. At each level its exceptions come first, then its holiday
//!   set; a site without its own set uses the tenant's.
//! - A non-working day (holiday or closure) has no capacity; a `working`
//!   exception cancels a holiday, the capacity of the weekday applying.

use crate::absences::{self, Absence, AbsenceKind, DayPart};
use crate::capacity::{self, Period, WeekMinutes};
use crate::dates::Date;
use crate::holidays::{HolidayName, HolidaySet};
use crate::validation::{FieldError, Violations};

/// Longest label of an exception, in characters.
pub const LABEL_MAX_CHARS: usize = 100;

/// Longest calendar name, per language, in characters.
pub const NAME_MAX_CHARS: usize = 100;

/// What an exception makes of its day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExceptionKind {
    /// A public holiday the set does not hold.
    Holiday,
    /// A closure, a bridge day.
    NonWorking,
    /// Worked anyway: cancels a holiday.
    Working,
}

impl ExceptionKind {
    pub const ALL: [Self; 3] = [Self::Holiday, Self::NonWorking, Self::Working];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Holiday => "holiday",
            Self::NonWorking => "non_working",
            Self::Working => "working",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == value)
    }
}

/// A dated exception of a calendar or a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exception {
    pub date: Date,
    pub kind: ExceptionKind,
    pub label: Option<String>,
}

/// One level of a resource's calendar.
#[derive(Debug, Clone, Copy, Default)]
pub struct Level<'a> {
    pub exceptions: &'a [Exception],
    /// `None`: the level below decides on holidays (a site using the
    /// tenant's set; a resource).
    pub holidays: Option<HolidaySet>,
}

/// Where a day's status comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Resource,
    Site,
    Tenant,
}

/// What a day is, before the capacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Worked as the capacity says (possibly 0 minutes: a weekend).
    Working,
    /// A public holiday: of the set (`name`), or an exception (`label`).
    Holiday {
        name: Option<HolidayName>,
        label: Option<String>,
    },
    /// A closure.
    NonWorking { label: Option<String> },
}

/// The status of `date` and the level that decided it; `None` when no
/// level says anything (a plain day).
pub fn status_on(levels: &[(Source, Level<'_>)], date: Date) -> (Status, Option<Source>) {
    for (source, level) in levels {
        if let Some(exception) = level.exceptions.iter().find(|e| e.date == date) {
            let label = exception.label.clone();
            let status = match exception.kind {
                ExceptionKind::Working => Status::Working,
                ExceptionKind::Holiday => Status::Holiday { name: None, label },
                ExceptionKind::NonWorking => Status::NonWorking { label },
            };
            return (status, Some(*source));
        }
        if let Some(set) = level.holidays {
            return match set.holiday_on(date) {
                Some(holiday) => (
                    Status::Holiday {
                        name: Some(holiday.name),
                        label: None,
                    },
                    Some(*source),
                ),
                None => (Status::Working, None),
            };
        }
    }
    (Status::Working, None)
}

/// A day of a resource: its status and its capacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    pub date: Date,
    pub status: Status,
    pub source: Option<Source>,
    /// The absence taking (part of) the day, if any.
    pub absence: Option<(AbsenceKind, DayPart)>,
    /// Minutes of capacity: 0 on a non-working day, less an absence.
    pub minutes: u16,
}

/// The days of a resource from `from` to `to` included: its calendar
/// `levels` (resource, site, tenant) over its capacity `periods` and the
/// tenant's `default` week, less its `absences`.
pub fn days(
    levels: &[(Source, Level<'_>)],
    periods: &[Period],
    default: WeekMinutes,
    absences: &[Absence],
    from: Date,
    to: Date,
) -> Vec<Day> {
    let count = to.days_since(from);
    (0..=count.max(-1))
        .map(|offset| {
            let date = from.add_days(offset);
            let (status, source) = status_on(levels, date);
            let absence = absences
                .iter()
                .find_map(|a| a.part_of(date).map(|part| (a.kind, part)));
            let minutes = match status {
                Status::Working => absences::minutes_left(
                    capacity::minutes_on(periods, default, date),
                    absence.map(|(_, part)| part),
                ),
                Status::Holiday { .. } | Status::NonWorking { .. } => 0,
            };
            Day {
                date,
                status,
                source,
                absence,
                minutes,
            }
        })
        .collect()
}

/// Checks the label of an exception (field `label`).
pub fn validate_exception(label: Option<&str>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if label
        .map(str::trim)
        .is_some_and(|l| l.chars().count() > LABEL_MAX_CHARS)
    {
        v.add("label", "too_long");
    }
    v.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn exception(day: &str, kind: ExceptionKind, label: Option<&str>) -> Exception {
        Exception {
            date: date(day),
            kind,
            label: label.map(str::to_owned),
        }
    }

    const WEEK: WeekMinutes = WeekMinutes([420, 420, 420, 420, 420, 0, 0]);

    fn tenant(exceptions: &[Exception]) -> (Source, Level<'_>) {
        (
            Source::Tenant,
            Level {
                exceptions,
                holidays: Some(HolidaySet::France),
            },
        )
    }

    #[test]
    fn a_french_holiday_has_no_capacity_and_a_plain_day_its_weekday_hours() {
        let levels = [tenant(&[])];
        let (status, source) = status_on(&levels, date("2026-12-25"));
        assert_eq!(
            status,
            Status::Holiday {
                name: Some(HolidayName::Christmas),
                label: None
            }
        );
        assert_eq!(source, Some(Source::Tenant));
        let week = days(&levels, &[], WEEK, &[], date("2026-12-21"), date("2026-12-27"));
        assert_eq!(
            week.iter().map(|d| d.minutes).collect::<Vec<_>>(),
            [420, 420, 420, 420, 0, 0, 0],
            "Christmas on Friday, then the weekend"
        );
        assert_eq!(week[0].source, None);
    }

    #[test]
    fn the_resource_overrides_its_site_which_overrides_the_tenant() {
        let tenant_closure = [exception("2026-12-24", ExceptionKind::NonWorking, Some("Closed"))];
        let paris = [exception("2026-12-24", ExceptionKind::Working, None)];
        let camille = [exception(
            "2026-12-23",
            ExceptionKind::NonWorking,
            Some("Training"),
        )];
        let levels = [
            (
                Source::Resource,
                Level {
                    exceptions: &camille,
                    holidays: None,
                },
            ),
            (
                Source::Site,
                Level {
                    exceptions: &paris,
                    holidays: None,
                },
            ),
            tenant(&tenant_closure),
        ];
        assert_eq!(
            status_on(&levels, date("2026-12-24")),
            (Status::Working, Some(Source::Site))
        );
        assert_eq!(
            status_on(&levels, date("2026-12-23")),
            (
                Status::NonWorking {
                    label: Some("Training".to_owned())
                },
                Some(Source::Resource)
            )
        );
        // The site uses the tenant's holidays.
        assert!(matches!(
            status_on(&levels, date("2026-12-25")).0,
            Status::Holiday { .. }
        ));
        // Without the site's exception, the tenant's closure applies.
        let without_site = [levels[0], tenant(&tenant_closure)];
        assert!(matches!(
            status_on(&without_site, date("2026-12-24")).0,
            Status::NonWorking { .. }
        ));
    }

    #[test]
    fn a_holiday_removed_by_the_tenant_is_worked_and_a_site_may_use_another_set() {
        let worked = [exception("2026-05-25", ExceptionKind::Working, None)];
        let levels = [tenant(&worked)];
        let (status, source) = status_on(&levels, date("2026-05-25"));
        assert_eq!((status, source), (Status::Working, Some(Source::Tenant)));
        assert_eq!(
            days(&levels, &[], WEEK, &[], date("2026-05-25"), date("2026-05-25"))[0].minutes,
            420
        );

        let strasbourg = (
            Source::Site,
            Level {
                exceptions: &[],
                holidays: Some(HolidaySet::FranceAlsaceMoselle),
            },
        );
        let levels = [strasbourg, tenant(&[])];
        assert!(matches!(
            status_on(&levels, date("2026-12-26")).0,
            Status::Holiday { .. }
        ));
        // A site without holidays at all.
        let abroad = (
            Source::Site,
            Level {
                exceptions: &[],
                holidays: Some(HolidaySet::None),
            },
        );
        assert_eq!(
            status_on(&[abroad, tenant(&[])], date("2026-12-25")),
            (Status::Working, None)
        );
    }

    #[test]
    fn a_custom_holiday_carries_its_label_and_the_capacity_follows_its_periods() {
        let custom = [exception(
            "2026-06-12",
            ExceptionKind::Holiday,
            Some("Company day"),
        )];
        let levels = [tenant(&custom)];
        let periods = [Period {
            effective_from: date("2026-06-01"),
            week: WeekMinutes([240; 7]),
        }];
        let days = days(
            &levels,
            &periods,
            WEEK,
            &[],
            date("2026-06-11"),
            date("2026-06-13"),
        );
        assert_eq!(days.iter().map(|d| d.minutes).collect::<Vec<_>>(), [240, 0, 240]);
        assert_eq!(
            days[1].status,
            Status::Holiday {
                name: None,
                label: Some("Company day".to_owned())
            }
        );
    }

    #[test]
    fn an_empty_range_has_no_day_and_labels_are_checked() {
        assert!(
            days(
                &[tenant(&[])],
                &[],
                WEEK,
                &[],
                date("2026-01-02"),
                date("2026-01-01")
            )
            .is_empty()
        );
        assert!(validate_exception(Some("Bridge day")).is_ok());
        assert_eq!(
            validate_exception(Some(&"x".repeat(LABEL_MAX_CHARS + 1))).unwrap_err()[0].code,
            "too_long"
        );
        for kind in ExceptionKind::ALL {
            assert_eq!(ExceptionKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn an_absence_takes_its_part_of_a_worked_day_and_nothing_of_a_holiday() {
        let levels = [tenant(&[])];
        let leave = Absence {
            kind: AbsenceKind::Leave,
            start: date("2026-12-24"),
            starts_at_noon: true,
            end: date("2026-12-28"),
            ends_at_noon: false,
        };
        let week = days(
            &levels,
            &[],
            WEEK,
            &[leave],
            date("2026-12-23"),
            date("2026-12-28"),
        );
        assert_eq!(
            week.iter().map(|d| d.minutes).collect::<Vec<_>>(),
            [420, 210, 0, 0, 0, 0],
            "Wednesday worked, Thursday afternoon off, Christmas, the weekend, Monday off"
        );
        assert_eq!(week[1].absence, Some((AbsenceKind::Leave, DayPart::Afternoon)));
        assert_eq!(week[5].absence, Some((AbsenceKind::Leave, DayPart::Whole)));
        assert_eq!(week[0].absence, None);
    }
}
