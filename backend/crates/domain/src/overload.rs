//! The overload check (specification §8, L08-US03): when an assignment is
//! written, the resulting load of each day is compared with the resource's
//! capacity plus the tenant's tolerance.
//!
//! - The capacity of a day is the actual one: weekly pattern, less public
//!   holidays and closures, less absences (`Day::minutes`). Hours planned on
//!   a day of absence are an overload.
//! - The limit is the capacity plus the tolerance, rounded down to the
//!   minute: 7 h with 10% gives 7 h 42. A load equal to the limit fits.
//! - Provisional assignments count or not, by a tenant setting: on both
//!   sides, the existing load and the assignment written.
//! - A day overloads when the assignment puts more on it than before and
//!   the total goes over the limit: changing an assignment never trips on
//!   a day it does not add to (an overload forced earlier, a capacity
//!   lowered since).
//! - Blocking mode (the default) refuses the write, unless the author has
//!   the "Force overload" permission and forces it; warning mode saves it
//!   and flags the days.
//!
//! No I/O: infra reads the loads and the capacity, this module decides.

use std::collections::HashMap;

use crate::calendars::Day;
use crate::dates::Date;
use crate::tenant::OverloadMode;

/// How the tenant checks overloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Load allowed beyond the capacity, in percent: 0 to 100.
    pub tolerance_percent: u8,
    pub mode: OverloadMode,
    /// Provisional assignments count in the check.
    pub count_provisional: bool,
}

impl Default for Policy {
    /// The tenant defaults: no tolerance, blocking, provisional counted.
    fn default() -> Self {
        Self {
            tolerance_percent: 0,
            mode: OverloadMode::Blocking,
            count_provisional: true,
        }
    }
}

impl Policy {
    /// Whether the load of an assignment so flagged counts in the check.
    pub const fn counts(&self, provisional: bool) -> bool {
        !provisional || self.count_provisional
    }

    /// The most a day of `capacity` minutes takes: the capacity plus the
    /// tolerance, rounded down to the minute.
    pub fn limit(&self, capacity: u32) -> u32 {
        let limit = u64::from(capacity) * (100 + u64::from(self.tolerance_percent)) / 100;
        u32::try_from(limit).unwrap_or(u32::MAX)
    }
}

/// One day as the check sees it, every load already counted or not by the
/// policy (provisional assignments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayLoad {
    pub date: Date,
    /// Minutes of capacity, absences deducted.
    pub capacity: u32,
    /// Load of the resource's other assignments.
    pub existing: u32,
    /// Load of the assignment before the write: 0 for a new one.
    pub before: u32,
    /// Load of the assignment as written.
    pub after: u32,
}

impl DayLoad {
    /// The resource's load once the assignment is written.
    pub const fn total(&self) -> u32 {
        self.existing.saturating_add(self.after)
    }
}

/// A day over the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overload {
    pub date: Date,
    /// Minutes planned: the other assignments and this one.
    pub planned: u32,
    /// Minutes of capacity.
    pub available: u32,
    /// Minutes allowed: the capacity plus the tolerance.
    pub limit: u32,
}

impl Overload {
    fn of(day: &DayLoad, policy: &Policy) -> Self {
        Self {
            date: day.date,
            planned: day.total(),
            available: day.capacity,
            limit: policy.limit(day.capacity),
        }
    }
}

/// The days of a span, each with its capacity (`days`, as
/// `ResourceCalendar::days` gives them) and its loads in minutes, by date:
/// `existing` from the other assignments, `before` and `after` from the
/// assignment written. Days missing from a map have 0.
pub fn day_loads(
    days: &[Day],
    existing: &HashMap<Date, u32>,
    before: &HashMap<Date, u32>,
    after: &HashMap<Date, u32>,
) -> Vec<DayLoad> {
    let of = |map: &HashMap<Date, u32>, date: &Date| map.get(date).copied().unwrap_or(0);
    days.iter()
        .map(|day| DayLoad {
            date: day.date,
            capacity: u32::from(day.minutes),
            existing: of(existing, &day.date),
            before: of(before, &day.date),
            after: of(after, &day.date),
        })
        .collect()
}

/// Whether the write overloads `day`: it adds to the day and the total
/// goes over the limit.
pub fn overloads_day(day: &DayLoad, policy: &Policy) -> bool {
    day.after > day.before && day.total() > policy.limit(day.capacity)
}

/// The days the write overloads, by date.
pub fn overloads(days: &[DayLoad], policy: &Policy) -> Vec<Overload> {
    days.iter()
        .filter(|day| overloads_day(day, policy))
        .map(|day| Overload::of(day, policy))
        .collect()
}

/// The days an assignment, as stored, loads while the resource is over the
/// limit (`before` ignored): what its ticket flags.
pub fn flagged(days: &[DayLoad], policy: &Policy) -> Vec<Overload> {
    days.iter()
        .filter(|day| day.after > 0 && day.total() > policy.limit(day.capacity))
        .map(|day| Overload::of(day, policy))
        .collect()
}

/// What happens to a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No overload: written.
    Accepted,
    /// Warning mode: written, the days flagged.
    Flagged(Vec<Overload>),
    /// Blocking mode: refused, the days listed.
    Refused(Vec<Overload>),
    /// Blocking mode, forced by a member allowed to: written and traced.
    Forced(Vec<Overload>),
}

impl Outcome {
    /// Whether the write goes ahead.
    pub const fn writes(&self) -> bool {
        !matches!(self, Self::Refused(_))
    }
}

/// The outcome of a write that overloads `overloads` (from [`overloads`]),
/// `force` set when the author, allowed to, forces it.
pub fn decide(policy: &Policy, overloads: Vec<Overload>, force: bool) -> Outcome {
    if overloads.is_empty() {
        return Outcome::Accepted;
    }
    match policy.mode {
        OverloadMode::Warning => Outcome::Flagged(overloads),
        OverloadMode::Blocking if force => Outcome::Forced(overloads),
        OverloadMode::Blocking => Outcome::Refused(overloads),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::absences::{Absence, AbsenceKind};
    use crate::assignments::{Mode, Segment, assignment_load};
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

    fn policy(tolerance_percent: u8, mode: OverloadMode) -> Policy {
        Policy {
            tolerance_percent,
            mode,
            count_provisional: true,
        }
    }

    fn day(text: &str, capacity: u32, existing: u32, before: u32, after: u32) -> DayLoad {
        DayLoad {
            date: date(text),
            capacity,
            existing,
            before,
            after,
        }
    }

    /// The day loads of `segments` written over `existing` on `calendar`,
    /// from `from` to `to`.
    fn checked(
        calendar: &ResourceCalendar<'_>,
        segments: &[Segment],
        existing: &[(&str, u32)],
        (from, to): (&str, &str),
    ) -> Vec<DayLoad> {
        let existing: HashMap<Date, u32> = existing.iter().map(|(d, m)| (date(d), *m)).collect();
        let after: HashMap<Date, u32> = assignment_load(segments, calendar).into_iter().collect();
        day_loads(
            &calendar.days(date(from), date(to)),
            &existing,
            &HashMap::new(),
            &after,
        )
    }

    fn hours_per_day(start: &str, end: &str, minutes: u32) -> Segment {
        Segment {
            start: date(start),
            end: date(end),
            mode: Mode::HoursPerDay,
            value: minutes,
        }
    }

    #[test]
    fn four_more_hours_on_a_day_loaded_with_four_are_refused_with_the_day() {
        // AC1: 7 h on 18 November, 4 h already planned, 4 h/day added.
        let blocking = policy(0, OverloadMode::Blocking);
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &[],
            default: FULL,
            absences: &[],
        };
        let days = checked(
            &calendar,
            &[hours_per_day("2026-11-16", "2026-11-20", 240)],
            &[("2026-11-18", 240), ("2026-11-19", 180)],
            ("2026-11-16", "2026-11-20"),
        );
        let found = overloads(&days, &blocking);
        assert_eq!(
            found,
            vec![Overload {
                date: date("2026-11-18"),
                planned: 480,
                available: 420,
                limit: 420,
            }]
        );
        assert!(!decide(&blocking, found, false).writes());
    }

    #[test]
    fn the_tolerance_lets_a_load_up_to_the_capacity_plus_its_share() {
        // AC2: 10% of 7 h is 42 min, so 7 h 30 fits and 7 h 43 does not.
        let tolerant = policy(10, OverloadMode::Blocking);
        assert_eq!(tolerant.limit(420), 462);
        assert!(overloads(&[day("2026-11-18", 420, 240, 0, 210)], &tolerant).is_empty());
        assert!(
            overloads(&[day("2026-11-18", 420, 240, 0, 222)], &tolerant).is_empty(),
            "equal to the limit"
        );
        assert_eq!(
            overloads(&[day("2026-11-18", 420, 240, 0, 223)], &tolerant).len(),
            1
        );
        // Rounded down: 10% of 7 h 05 is 42.5 min.
        assert_eq!(tolerant.limit(425), 467);
        assert_eq!(policy(100, OverloadMode::Blocking).limit(420), 840);
    }

    #[test]
    fn exactly_the_capacity_fits_one_minute_more_does_not() {
        let strict = policy(0, OverloadMode::Blocking);
        assert!(overloads(&[day("2026-11-18", 420, 180, 0, 240)], &strict).is_empty());
        assert_eq!(
            overloads(&[day("2026-11-18", 420, 181, 0, 240)], &strict).len(),
            1
        );
    }

    #[test]
    fn warning_mode_writes_and_flags_and_forcing_writes_in_blocking_mode() {
        // AC3, AC4.
        let over = vec![Overload {
            date: date("2026-11-18"),
            planned: 480,
            available: 420,
            limit: 420,
        }];
        let warning = policy(0, OverloadMode::Warning);
        let blocking = policy(0, OverloadMode::Blocking);
        assert_eq!(
            decide(&warning, over.clone(), false),
            Outcome::Flagged(over.clone())
        );
        assert_eq!(
            decide(&warning, over.clone(), true),
            Outcome::Flagged(over.clone()),
            "nothing to force"
        );
        assert_eq!(
            decide(&blocking, over.clone(), false),
            Outcome::Refused(over.clone())
        );
        assert_eq!(
            decide(&blocking, over.clone(), true),
            Outcome::Forced(over.clone())
        );
        assert!(decide(&blocking, over, true).writes());
        assert_eq!(decide(&blocking, Vec::new(), true), Outcome::Accepted);
    }

    #[test]
    fn a_change_only_trips_on_the_days_it_adds_to() {
        let strict = policy(0, OverloadMode::Blocking);
        // Forced earlier at 8 h: kept as is, or lowered, it passes.
        assert!(overloads(&[day("2026-11-18", 420, 240, 240, 240)], &strict).is_empty());
        assert!(overloads(&[day("2026-11-18", 420, 240, 240, 200)], &strict).is_empty());
        // Raised, it trips.
        assert_eq!(
            overloads(&[day("2026-11-18", 420, 240, 240, 241)], &strict).len(),
            1
        );
        // Yet the ticket still flags it.
        assert_eq!(
            flagged(&[day("2026-11-18", 420, 240, 240, 240)], &strict).len(),
            1
        );
        // A day the assignment does not load is never its overload.
        assert!(flagged(&[day("2026-11-18", 420, 600, 0, 0)], &strict).is_empty());
    }

    #[test]
    fn provisional_assignments_count_by_the_tenant_setting() {
        let counted = Policy::default();
        assert!(counted.counts(true));
        assert!(counted.counts(false));
        let ignored = Policy {
            count_provisional: false,
            ..counted
        };
        assert!(!ignored.counts(true));
        assert!(ignored.counts(false));
    }

    #[test]
    fn part_time_days_and_public_holidays_have_their_own_capacity() {
        // Friday at 3.5 h; Wednesday 11 November 2026 is Armistice Day.
        let part_time = [Period {
            effective_from: date("2026-01-01"),
            week: WeekMinutes([420, 420, 420, 420, 210, 0, 0]),
        }];
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &part_time,
            default: FULL,
            absences: &[],
        };
        let strict = policy(0, OverloadMode::Blocking);
        // 4 h/day: the Friday overloads, the holiday gets nothing.
        let days = checked(
            &calendar,
            &[hours_per_day("2026-11-09", "2026-11-13", 240)],
            &[],
            ("2026-11-09", "2026-11-13"),
        );
        let wednesday = days.iter().find(|d| d.date == date("2026-11-11"));
        assert_eq!(wednesday.map(|d| (d.capacity, d.after)), Some((0, 0)));
        let found = overloads(&days, &strict);
        assert_eq!(
            found.iter().map(|o| o.date).collect::<Vec<_>>(),
            vec![date("2026-11-13")]
        );
        assert_eq!(found[0].available, 210);
        // Work planned on the holiday by another assignment is not this one's.
        let days = checked(
            &calendar,
            &[hours_per_day("2026-11-09", "2026-11-12", 60)],
            &[("2026-11-11", 120)],
            ("2026-11-09", "2026-11-12"),
        );
        assert!(overloads(&days, &strict).is_empty());
    }

    #[test]
    fn hours_on_a_day_of_absence_overload_it_percentages_never_do() {
        let absences = [Absence {
            kind: AbsenceKind::Leave,
            start: date("2026-11-18"),
            starts_at_noon: false,
            end: date("2026-11-18"),
            ends_at_noon: true,
        }];
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &[],
            default: FULL,
            absences: &absences,
        };
        let strict = policy(0, OverloadMode::Blocking);
        // Half a day of absence: 3.5 h left, 4 h planned.
        let days = checked(
            &calendar,
            &[hours_per_day("2026-11-16", "2026-11-20", 240)],
            &[],
            ("2026-11-16", "2026-11-20"),
        );
        let found = overloads(&days, &strict);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].date, found[0].planned, found[0].available),
            (date("2026-11-18"), 240, 210)
        );
        // 100% follows the capacity down.
        let full_time = Segment {
            start: date("2026-11-16"),
            end: date("2026-11-20"),
            mode: Mode::Percentage,
            value: 100,
        };
        let days = checked(&calendar, &[full_time], &[], ("2026-11-16", "2026-11-20"));
        assert!(overloads(&days, &strict).is_empty());
    }

    #[test]
    fn a_week_with_a_clock_change_has_five_ordinary_days() {
        // Summer time ends on Sunday 25 October 2026 and starts on Sunday
        // 28 March 2027: local dates, the days keep their 7 h.
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &[],
            default: FULL,
            absences: &[],
        };
        let strict = policy(0, OverloadMode::Blocking);
        // Easter Monday 29 March 2027 is the one day off of the second span.
        for (start, end, working) in [("2026-10-19", "2026-10-30", 10), ("2027-03-22", "2027-04-02", 9)] {
            let days = checked(&calendar, &[hours_per_day(start, end, 420)], &[], (start, end));
            assert_eq!(
                days.iter().filter(|d| d.capacity == 420).count(),
                working,
                "{start}"
            );
            assert!(overloads(&days, &strict).is_empty(), "{start}");
            let days = checked(&calendar, &[hours_per_day(start, end, 421)], &[], (start, end));
            assert_eq!(overloads(&days, &strict).len(), working, "{start}");
        }
    }

    #[test]
    fn day_loads_follow_the_calendar_days_with_missing_loads_at_zero() {
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &[],
            default: FULL,
            absences: &[],
        };
        let days = calendar.days(date("2026-11-20"), date("2026-11-23"));
        let existing = HashMap::from([(date("2026-11-23"), 60)]);
        let before = HashMap::from([(date("2026-11-20"), 30)]);
        let loads = day_loads(&days, &existing, &before, &HashMap::new());
        assert_eq!(
            loads,
            vec![
                day("2026-11-20", 420, 0, 30, 0),
                day("2026-11-21", 0, 0, 0, 0),
                day("2026-11-22", 0, 0, 0, 0),
                day("2026-11-23", 420, 60, 0, 0),
            ]
        );
        assert_eq!(loads[3].total(), 60);
    }
}
