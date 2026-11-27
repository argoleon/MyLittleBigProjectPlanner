//! Free capacity of a resource over a period (specification §8, L08-US10):
//! "who has 3 free days next week in team X?".
//!
//! - The free time of a day is its capacity (absences, public holidays and
//!   closures deducted) less its load, never below zero. The overload
//!   tolerance is a margin, not availability: it is left out.
//! - A free day is a working day entirely free: nothing planned on it, and
//!   no absence taking part of it. Free days need not follow each other.
//! - Results are sorted by free time, most first, then by name.
//!
//! No I/O.

use std::collections::HashMap;

use crate::calendars::Day;
use crate::dates::Date;

/// What a search asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// At least these free minutes over the period.
    Minutes(u32),
    /// At least this many free days.
    Days(u32),
}

/// The free capacity of a resource over a period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Availability {
    /// Minutes of capacity left: per day, capacity less load, at least 0.
    pub free_minutes: u32,
    /// Working days entirely free.
    pub free_days: u32,
    /// Minutes of capacity, absences deducted.
    pub capacity_minutes: u32,
    /// Minutes planned.
    pub load_minutes: u32,
    /// Days with capacity.
    pub working_days: u32,
}

impl Availability {
    /// Whether it meets `need`; any availability meets no need.
    pub const fn meets(&self, need: Option<Need>) -> bool {
        match need {
            None => true,
            Some(Need::Minutes(minutes)) => self.free_minutes >= minutes,
            Some(Need::Days(days)) => self.free_days >= days,
        }
    }
}

/// The availability of a resource over `days` (its calendar days), with
/// `load` its minutes planned by date.
pub fn availability(days: &[Day], load: &HashMap<Date, u32>) -> Availability {
    let mut result = Availability::default();
    for day in days {
        let capacity = u32::from(day.minutes);
        let planned = load.get(&day.date).copied().unwrap_or(0);
        result.capacity_minutes += capacity;
        result.load_minutes += planned;
        result.free_minutes += capacity.saturating_sub(planned);
        if capacity > 0 {
            result.working_days += 1;
            if planned == 0 && day.minutes == day.scheduled {
                result.free_days += 1;
            }
        }
    }
    result
}

/// Sorts `found` (name and availability of each) by free minutes, most
/// first, then by name, case ignored.
pub fn rank<T>(found: &mut [(String, Availability, T)]) {
    found.sort_by(|a, b| {
        b.1.free_minutes
            .cmp(&a.1.free_minutes)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::absences::{Absence, AbsenceKind};
    use crate::calendars::{Level, ResourceCalendar, Source};
    use crate::capacity::{Period, WeekMinutes};
    use crate::holidays::HolidaySet;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    const FULL: WeekMinutes = WeekMinutes([420, 420, 420, 420, 420, 0, 0]);
    const FRANCE: [(Source, Level<'static>); 1] = [(
        Source::Tenant,
        Level {
            exceptions: &[],
            holidays: Some(HolidaySet::France),
        },
    )];

    fn load(days: &[(&str, u32)]) -> HashMap<Date, u32> {
        days.iter().map(|(d, m)| (date(d), *m)).collect()
    }

    fn week(calendar: &ResourceCalendar<'_>, from: &str, to: &str) -> Vec<Day> {
        calendar.days(date(from), date(to))
    }

    #[test]
    fn free_time_is_capacity_less_load_and_free_days_are_untouched_ones() {
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &[],
            default: FULL,
            absences: &[],
        };
        // Week of 16 November 2026: 35 h; 4 h Monday, 7 h Tuesday, 9 h Wednesday.
        let days = week(&calendar, "2026-11-16", "2026-11-22");
        let found = availability(
            &days,
            &load(&[("2026-11-16", 240), ("2026-11-17", 420), ("2026-11-18", 540)]),
        );
        assert_eq!(
            found,
            Availability {
                // Monday 3 h, Tuesday and Wednesday none (never below 0), Thursday and Friday 7 h.
                free_minutes: 180 + 420 + 420,
                free_days: 2,
                capacity_minutes: 2100,
                load_minutes: 1200,
                working_days: 5,
            }
        );
        // Load on a weekend day takes no free time and gives no free day.
        let weekend = availability(&days, &load(&[("2026-11-21", 120)]));
        assert_eq!((weekend.free_minutes, weekend.free_days), (2100, 5));
    }

    #[test]
    fn public_holidays_part_time_and_absences_reduce_the_free_capacity() {
        // Fridays at 3 h 30; Wednesday 11 November 2026 is Armistice Day;
        // Thursday 12 off in the afternoon.
        let part_time = [Period {
            effective_from: date("2026-01-01"),
            week: WeekMinutes([420, 420, 420, 420, 210, 0, 0]),
        }];
        let absences = [Absence {
            kind: AbsenceKind::Leave,
            start: date("2026-11-12"),
            starts_at_noon: true,
            end: date("2026-11-12"),
            ends_at_noon: false,
        }];
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &part_time,
            default: FULL,
            absences: &absences,
        };
        let days = week(&calendar, "2026-11-09", "2026-11-15");
        let found = availability(&days, &HashMap::new());
        // Mon 7 h, Tue 7 h, Wed holiday, Thu 3 h 30 (afternoon off), Fri 3 h 30.
        assert_eq!(found.capacity_minutes, 420 + 420 + 210 + 210);
        assert_eq!(found.free_minutes, found.capacity_minutes);
        assert_eq!(found.working_days, 4);
        // The half day off is no full free day; the part-time Friday is.
        assert_eq!(found.free_days, 3);
    }

    #[test]
    fn a_search_asks_for_free_minutes_or_free_days() {
        let found = Availability {
            free_minutes: 600,
            free_days: 1,
            ..Availability::default()
        };
        assert!(found.meets(None));
        assert!(found.meets(Some(Need::Minutes(600))));
        assert!(!found.meets(Some(Need::Minutes(601))));
        assert!(found.meets(Some(Need::Days(1))));
        assert!(!found.meets(Some(Need::Days(2))));
    }

    #[test]
    fn results_come_by_free_time_then_by_name() {
        let with = |free_minutes| Availability {
            free_minutes,
            ..Availability::default()
        };
        let mut found = vec![
            ("nathan".to_owned(), with(600), 1),
            ("Camille".to_owned(), with(1200), 2),
            ("Alice".to_owned(), with(600), 3),
        ];
        rank(&mut found);
        assert_eq!(found.iter().map(|f| f.2).collect::<Vec<_>>(), [2, 3, 1]);
    }
}
