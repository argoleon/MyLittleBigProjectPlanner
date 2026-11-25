//! Capacity of resources (specification §8, L07-US03): hours per weekday,
//! historized by effective dates, in minutes to avoid rounding.
//!
//! - A resource's capacity on a date comes from its latest period starting
//!   on or before that date.
//! - Without such a period (none at all, or a date before the first one),
//!   the tenant's default capacity applies, as it stands: it is never
//!   copied.
//! - The capacity of a day, after public holidays, closures and absences,
//!   is `crate::calendars::ResourceCalendar` (L07-US06).

use crate::dates::{Date, Weekday};
use crate::tenant::DAY_MINUTES;
use crate::validation::{FieldError, Violations};

/// Longest label of a period, in characters.
pub const LABEL_MAX_CHARS: usize = 100;

/// Minutes per weekday, Monday first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WeekMinutes(pub [u16; 7]);

impl WeekMinutes {
    /// The minutes of `day`.
    pub const fn on(&self, day: Weekday) -> u16 {
        self.0[day.index()]
    }

    /// The minutes of the whole week.
    pub fn total(&self) -> u32 {
        self.0.iter().map(|m| u32::from(*m)).sum()
    }

    /// From seven values of 0 to a full day, as stored; `None` otherwise.
    pub fn from_stored(minutes: &[i32]) -> Option<Self> {
        let values: Vec<u16> = minutes
            .iter()
            .map(|m| u16::try_from(*m).ok().filter(|m| i64::from(*m) <= DAY_MINUTES))
            .collect::<Option<_>>()?;
        Some(Self(values.try_into().ok()?))
    }
}

/// A capacity period: from `effective_from` until the next period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    pub effective_from: Date,
    pub week: WeekMinutes,
}

/// The week applying on `date`: that of the latest of `periods` starting on
/// or before it, else `default` (the tenant's).
pub fn week_on(periods: &[Period], default: WeekMinutes, date: Date) -> WeekMinutes {
    periods
        .iter()
        .filter(|p| p.effective_from <= date)
        .max_by_key(|p| p.effective_from)
        .map_or(default, |p| p.week)
}

/// The capacity of `date`, in minutes.
pub fn minutes_on(periods: &[Period], default: WeekMinutes, date: Date) -> u16 {
    week_on(periods, default, date).on(date.weekday())
}

/// The days whose capacity may change when periods starting on `changed`
/// dates are added, changed or removed, given every start date after the
/// change: from the earliest changed date, until (excluded) the first start
/// after the latest changed one; `None`: no end.
pub fn affected(changed: &[Date], starts_after: &[Date]) -> Option<(Date, Option<Date>)> {
    let from = changed.iter().min().copied()?;
    let last = changed.iter().max().copied()?;
    let until = starts_after.iter().filter(|d| **d > last).min().copied();
    Some((from, until))
}

/// Checks the minutes and label of a period (fields `minutes`, `label`).
pub fn validate(minutes: &[i64], label: Option<&str>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if minutes.len() == 7 {
        for (day, value) in minutes.iter().enumerate() {
            if !(0..=DAY_MINUTES).contains(value) {
                v.add(format!("minutes.{day}"), "out_of_range");
            }
        }
    } else {
        v.add("minutes", "length");
    }
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

    const DEFAULT: WeekMinutes = WeekMinutes([420, 420, 420, 420, 420, 0, 0]);

    #[test]
    fn without_a_period_the_tenant_default_applies() {
        // 2026-11-23 is a Monday.
        let monday = date("2026-11-23");
        assert_eq!(minutes_on(&[], DEFAULT, monday), 420);
        assert_eq!(minutes_on(&[], DEFAULT, monday.add_days(5)), 0, "Saturday");
        assert_eq!(minutes_on(&[], DEFAULT, monday.add_days(6)), 0, "Sunday");
    }

    #[test]
    fn a_new_period_changes_the_days_from_its_effective_date_only() {
        let periods = [
            Period {
                effective_from: date("2026-01-01"),
                week: DEFAULT,
            },
            Period {
                effective_from: date("2026-03-01"),
                week: WeekMinutes([420, 420, 0, 420, 420, 0, 0]),
            },
        ];
        // Wednesdays before and after 1 March 2026.
        assert_eq!(minutes_on(&periods, DEFAULT, date("2026-02-25")), 420);
        assert_eq!(minutes_on(&periods, DEFAULT, date("2026-03-04")), 0);
        // Before the first period: the tenant default, here part time.
        let part_time = WeekMinutes([240; 7]);
        assert_eq!(minutes_on(&periods, part_time, date("2025-12-31")), 240);
        // The order of the periods does not matter.
        let reversed = [periods[1], periods[0]];
        assert_eq!(minutes_on(&reversed, DEFAULT, date("2026-03-04")), 0);
    }

    #[test]
    fn a_friday_of_three_and_a_half_hours_is_210_minutes() {
        let periods = [Period {
            effective_from: date("2026-01-01"),
            week: WeekMinutes([420, 420, 420, 420, 210, 0, 0]),
        }];
        assert_eq!(minutes_on(&periods, DEFAULT, date("2026-11-27")), 210);
        assert_eq!(periods[0].week.total(), 1890);
    }

    #[test]
    fn stored_weeks_have_seven_days_of_a_full_day_at_most() {
        assert_eq!(
            WeekMinutes::from_stored(&[420, 420, 420, 420, 420, 0, 0]),
            Some(DEFAULT)
        );
        assert_eq!(WeekMinutes::from_stored(&[420; 6]), None);
        assert_eq!(WeekMinutes::from_stored(&[1441, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(WeekMinutes::from_stored(&[-1, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn a_change_affects_the_days_until_the_next_period() {
        let (jan, mar, jun) = (date("2026-01-01"), date("2026-03-01"), date("2026-06-01"));
        assert_eq!(affected(&[mar], &[jan, mar, jun]), Some((mar, Some(jun))));
        assert_eq!(affected(&[jun], &[jan, mar, jun]), Some((jun, None)));
        // Moved from March to January: both stretches.
        assert_eq!(affected(&[mar, jan], &[jan, jun]), Some((jan, Some(jun))));
        // Removed: until the next one still there.
        assert_eq!(affected(&[mar], &[jan, jun]), Some((mar, Some(jun))));
        assert_eq!(affected(&[], &[jan]), None);
    }

    #[test]
    fn minutes_and_labels_are_checked() {
        assert!(validate(&[420, 420, 420, 420, 210, 0, 0], Some("Temps partiel 80 %")).is_ok());
        assert!(validate(&[1440, 0, 0, 0, 0, 0, 0], None).is_ok());
        let codes = |minutes: &[i64], label: Option<&str>| -> Vec<(String, &'static str)> {
            validate(minutes, label)
                .err()
                .unwrap_or_default()
                .into_iter()
                .map(|e| (e.field, e.code))
                .collect()
        };
        assert_eq!(codes(&[420; 5], None), [("minutes".to_owned(), "length")]);
        assert_eq!(
            codes(&[420, -1, 1441, 0, 0, 0, 0], None),
            [
                ("minutes.1".to_owned(), "out_of_range"),
                ("minutes.2".to_owned(), "out_of_range")
            ]
        );
        assert_eq!(
            codes(&[0; 7], Some(&"x".repeat(LABEL_MAX_CHARS + 1))),
            [("label".to_owned(), "too_long")]
        );
    }
}
