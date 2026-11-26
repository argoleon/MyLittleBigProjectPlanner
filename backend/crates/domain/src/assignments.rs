//! Assignments of resources to tickets (specification §8, L08-US01).
//!
//! An assignment links one resource to one ticket through successive
//! segments, each with its own period and mode:
//!
//! - **Percentage**: that share of the day's actual capacity (after public
//!   holidays, closures and absences): 50% of a 3.5 h Friday is 1 h 45, a
//!   public holiday 0.
//! - **Hours per day**: fixed minutes on each working day of the period.
//! - **Total effort**: minutes spread evenly over the working days of the
//!   period, the minutes left over going to the first days.
//!
//! A working day is one the calendar and the capacity give minutes to
//! (`Day::scheduled`), absences aside: hours falling on an absence stay
//! there and become a conflict (L08-US04), nothing moves by itself.
//! Segments of an assignment never overlap and stay within the ticket's
//! dates, when it has them.

use crate::calendars::{Day, ResourceCalendar};
use crate::dates::Date;
use crate::tenant::DAY_MINUTES;
use crate::validation::{FieldError, Violations};

/// Most segments in one assignment.
pub const MAX_SEGMENTS: usize = 50;
/// Longest segment, in days.
pub const MAX_SEGMENT_DAYS: i32 = 731;
/// Largest total effort: 9,999 hours.
pub const MAX_EFFORT_MINUTES: u32 = 9_999 * 60;

/// How a segment loads its days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// `value` is a percentage, 1 to 100.
    Percentage,
    /// `value` is minutes per working day, 1 to a full day.
    HoursPerDay,
    /// `value` is minutes over the whole period.
    TotalEffort,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::Percentage, Self::HoursPerDay, Self::TotalEffort];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Percentage => "percentage",
            Self::HoursPerDay => "hours_per_day",
            Self::TotalEffort => "total_effort",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == value)
    }
}

/// A segment: from `start` to `end` included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start: Date,
    pub end: Date,
    pub mode: Mode,
    /// A percentage, or minutes (see [`Mode`]).
    pub value: u32,
}

impl Segment {
    /// Its number of days.
    pub fn days(&self) -> i32 {
        self.end.days_since(self.start) + 1
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// What makes segments wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentError {
    /// Field errors (`segments`, `segments.{i}.end`, `segments.{i}.value`).
    Invalid(Vec<FieldError>),
    /// Two segments take a same day (`segments.{i}`, the later one).
    Overlap(Vec<FieldError>),
    /// Segments start before the ticket's start or end after its due date
    /// (`segments.{i}.start`, `segments.{i}.end`).
    OutsideIssueDates(Vec<FieldError>),
}

/// Checks `segments` on their own, then against each other, then against
/// the ticket's dates (only those it has).
pub fn validate(
    segments: &[Segment],
    issue_start: Option<Date>,
    issue_due: Option<Date>,
) -> Result<(), SegmentError> {
    let mut v = Violations::new();
    if segments.is_empty() {
        v.add("segments", "required");
    } else if segments.len() > MAX_SEGMENTS {
        v.add("segments", "too_many");
    }
    for (i, segment) in segments.iter().enumerate() {
        if segment.end < segment.start {
            v.add(format!("segments.{i}.end"), "before_start");
        } else if segment.days() > MAX_SEGMENT_DAYS {
            v.add(format!("segments.{i}.end"), "out_of_range");
        }
        let range = match segment.mode {
            Mode::Percentage => 1..=100,
            Mode::HoursPerDay => 1..=u32::try_from(DAY_MINUTES).unwrap_or(1440),
            Mode::TotalEffort => 1..=MAX_EFFORT_MINUTES,
        };
        if !range.contains(&segment.value) {
            v.add(format!("segments.{i}.value"), "out_of_range");
        }
    }
    v.into_result().map_err(SegmentError::Invalid)?;

    let mut v = Violations::new();
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by_key(|i| segments[*i].start);
    for pair in order.windows(2) {
        if segments[pair[0]].overlaps(&segments[pair[1]]) {
            v.add(format!("segments.{}", pair[1]), "overlap");
        }
    }
    v.into_result().map_err(SegmentError::Overlap)?;

    let mut v = Violations::new();
    for (i, segment) in segments.iter().enumerate() {
        if issue_start.is_some_and(|start| segment.start < start) {
            v.add(format!("segments.{i}.start"), "outside_issue_dates");
        }
        if issue_due.is_some_and(|due| segment.end > due) {
            v.add(format!("segments.{i}.end"), "outside_issue_dates");
        }
    }
    v.into_result().map_err(SegmentError::OutsideIssueDates)
}

/// Whether some of `segments` fall outside the ticket's dates: shown on the
/// ticket once its dates changed after the assignment was saved.
pub fn outside_issue_dates(segments: &[Segment], issue_start: Option<Date>, issue_due: Option<Date>) -> bool {
    segments
        .iter()
        .any(|s| issue_start.is_some_and(|start| s.start < start) || issue_due.is_some_and(|due| s.end > due))
}

/// The minutes a segment puts on each of its days, from `start` to `end`,
/// given those days of the resource (`days`, one per day of the segment, as
/// `ResourceCalendar::days` gives them).
pub fn segment_load(segment: &Segment, days: &[Day]) -> Vec<u32> {
    match segment.mode {
        Mode::Percentage => days
            .iter()
            // Rounded to the nearest minute, half up.
            .map(|d| (u32::from(d.minutes) * segment.value + 50) / 100)
            .collect(),
        Mode::HoursPerDay => days
            .iter()
            .map(|d| if d.scheduled > 0 { segment.value } else { 0 })
            .collect(),
        Mode::TotalEffort => {
            let working = u32::try_from(days.iter().filter(|d| d.scheduled > 0).count()).unwrap_or(0);
            if working == 0 {
                return vec![0; days.len()];
            }
            let (each, mut left) = (segment.value / working, segment.value % working);
            days.iter()
                .map(|d| {
                    if d.scheduled == 0 {
                        return 0;
                    }
                    let extra = u32::from(left > 0);
                    left -= extra;
                    each + extra
                })
                .collect()
        }
    }
}

/// The load of a segment on `calendar`: each day with its minutes.
pub fn load(segment: &Segment, calendar: &ResourceCalendar<'_>) -> Vec<(Date, u32)> {
    let days = calendar.days(segment.start, segment.end);
    let minutes = segment_load(segment, &days);
    days.into_iter().map(|d| d.date).zip(minutes).collect()
}

/// The daily load of an assignment on `calendar` (L08-US02): each day its
/// segments load, by date, days of 0 left out. Segments never overlap; a
/// day two of them would share gets their sum.
pub fn assignment_load(segments: &[Segment], calendar: &ResourceCalendar<'_>) -> Vec<(Date, u32)> {
    let mut days: Vec<(Date, u32)> = segments
        .iter()
        .flat_map(|segment| load(segment, calendar))
        .filter(|(_, minutes)| *minutes > 0)
        .collect();
    days.sort_by_key(|(date, _)| *date);
    days.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            kept.1 += later.1;
        }
        same
    });
    days
}

/// Whether a total-effort segment has no working day to go on: its
/// effort cannot be placed (field `segments.{i}.start`, `no_working_day`).
pub fn unplaceable(segments: &[Segment], calendar: &ResourceCalendar<'_>) -> Vec<FieldError> {
    segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.mode == Mode::TotalEffort)
        .filter(|(_, s)| calendar.days(s.start, s.end).iter().all(|d| d.scheduled == 0))
        .map(|(i, _)| FieldError::new(format!("segments.{i}.start"), "no_working_day"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::absences::{Absence, AbsenceKind};
    use crate::calendars::{Level, Source};
    use crate::capacity::{Period, WeekMinutes};
    use crate::holidays::HolidaySet;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn segment(start: &str, end: &str, mode: Mode, value: u32) -> Segment {
        Segment {
            start: date(start),
            end: date(end),
            mode,
            value,
        }
    }

    const FULL: WeekMinutes = WeekMinutes([420, 420, 420, 420, 420, 0, 0]);
    const SHORT_FRIDAY: WeekMinutes = WeekMinutes([420, 420, 420, 420, 210, 0, 0]);
    const FRANCE: [(Source, Level<'static>); 1] = [(
        Source::Tenant,
        Level {
            exceptions: &[],
            holidays: Some(HolidaySet::France),
        },
    )];

    fn calendar<'a>(
        periods: &'a [Period],
        default: WeekMinutes,
        absences: &'a [Absence],
    ) -> ResourceCalendar<'a> {
        ResourceCalendar {
            levels: &FRANCE,
            periods,
            default,
            absences,
        }
    }

    fn minutes(load: &[(Date, u32)]) -> Vec<u32> {
        load.iter().map(|(_, m)| *m).collect()
    }

    #[test]
    fn four_hours_a_day_on_each_working_day() {
        // AC1: 4 h/day from Monday 16 to Friday 20 November 2026.
        let load = load(
            &segment("2026-11-16", "2026-11-20", Mode::HoursPerDay, 240),
            &calendar(&[], FULL, &[]),
        );
        assert_eq!(minutes(&load), [240; 5]);
        assert_eq!(load[0].0, date("2026-11-16"));
        // Over a weekend and Armistice day (Wednesday 11 November): nothing.
        let load = load_of("2026-11-09", "2026-11-15", Mode::HoursPerDay, 240, FULL);
        assert_eq!(load, [240, 240, 0, 240, 240, 0, 0]);
    }

    fn load_of(start: &str, end: &str, mode: Mode, value: u32, week: WeekMinutes) -> Vec<u32> {
        minutes(&load(
            &segment(start, end, mode, value),
            &calendar(&[], week, &[]),
        ))
    }

    #[test]
    fn half_of_a_short_friday_and_nothing_on_a_holiday() {
        // AC2: 50% of a 3.5 h Friday is 1 h 45; Christmas (Friday 25
        // December 2026) carries nothing.
        assert_eq!(
            load_of("2026-12-14", "2026-12-18", Mode::Percentage, 50, SHORT_FRIDAY),
            [210, 210, 210, 210, 105]
        );
        assert_eq!(
            load_of("2026-12-24", "2026-12-25", Mode::Percentage, 50, SHORT_FRIDAY),
            [210, 0]
        );
        // 30% of 7 h is 2 h 06.
        assert_eq!(
            load_of("2026-12-14", "2026-12-14", Mode::Percentage, 30, FULL),
            [126]
        );
    }

    #[test]
    fn total_effort_is_spread_evenly_over_working_days() {
        // AC3: 40 h between Wednesday 3 and Sunday 28 March 2027 (no
        // holiday then): 18 working days of 2 h 13 or 2 h 14.
        let load = load_of("2027-03-03", "2027-03-28", Mode::TotalEffort, 40 * 60, FULL);
        assert_eq!(load.iter().sum::<u32>(), 2400);
        assert_eq!(load.iter().filter(|m| **m > 0).count(), 18);
        assert_eq!(
            load.iter().filter(|m| **m == 134).count(),
            6,
            "2,400 = 18 × 133 + 6"
        );
        assert!(load.iter().all(|m| *m == 0 || *m == 133 || *m == 134));
        // 10 h over 3 working days: 200, 200, 200 min; 1,001 min: the
        // extra minutes go to the first days.
        assert_eq!(
            load_of("2027-03-03", "2027-03-05", Mode::TotalEffort, 1001, FULL),
            [334, 334, 333]
        );
    }

    #[test]
    fn part_time_days_and_absences() {
        let part_time = [Period {
            effective_from: date("2026-01-01"),
            week: WeekMinutes([420, 420, 0, 420, 420, 0, 0]),
        }];
        let leave = [Absence {
            kind: AbsenceKind::Leave,
            start: date("2026-11-19"),
            starts_at_noon: true,
            end: date("2026-11-19"),
            ends_at_noon: false,
        }];
        let calendar = calendar(&part_time, FULL, &leave);
        let week = |mode, value| {
            minutes(&load(
                &segment("2026-11-16", "2026-11-20", mode, value),
                &calendar,
            ))
        };
        // Wednesday off; Thursday afternoon off halves the percentage only.
        assert_eq!(week(Mode::Percentage, 100), [420, 420, 0, 210, 420]);
        assert_eq!(week(Mode::HoursPerDay, 240), [240, 240, 0, 240, 240]);
        assert_eq!(week(Mode::TotalEffort, 960), [240, 240, 0, 240, 240]);
    }

    #[test]
    fn a_daylight_saving_week_changes_nothing() {
        // France changes time on Sunday 29 March and 25 October 2026.
        for (monday, sunday) in [("2026-03-23", "2026-03-29"), ("2026-10-19", "2026-10-25")] {
            assert_eq!(
                load_of(monday, sunday, Mode::HoursPerDay, 300, FULL),
                [300, 300, 300, 300, 300, 0, 0]
            );
            assert_eq!(
                load_of(monday, sunday, Mode::TotalEffort, 1500, FULL),
                [300, 300, 300, 300, 300, 0, 0]
            );
        }
    }

    #[test]
    fn the_daily_load_of_an_assignment_keeps_the_days_it_loads_by_date() {
        let calendar = calendar(&[], SHORT_FRIDAY, &[]);
        // Given out of order: the week after at 50%, then 4 h/day over a
        // week with a weekend; Christmas (Friday 25 December 2026) at 50%.
        let segments = [
            segment("2026-12-21", "2026-12-25", Mode::Percentage, 50),
            segment("2026-12-14", "2026-12-20", Mode::HoursPerDay, 240),
        ];
        let rows = assignment_load(&segments, &calendar);
        let shown: Vec<(String, u32)> = rows.iter().map(|(d, m)| (d.to_string(), *m)).collect();
        assert_eq!(
            shown,
            [
                ("2026-12-14".to_owned(), 240),
                ("2026-12-15".to_owned(), 240),
                ("2026-12-16".to_owned(), 240),
                ("2026-12-17".to_owned(), 240),
                ("2026-12-18".to_owned(), 240),
                ("2026-12-21".to_owned(), 210),
                ("2026-12-22".to_owned(), 210),
                ("2026-12-23".to_owned(), 210),
                ("2026-12-24".to_owned(), 210),
            ],
            "no row for the weekend nor for Christmas"
        );
        // Days two segments would share are summed.
        let overlapping = [
            segment("2026-12-14", "2026-12-14", Mode::HoursPerDay, 60),
            segment("2026-12-14", "2026-12-15", Mode::HoursPerDay, 30),
        ];
        assert_eq!(
            assignment_load(&overlapping, &calendar)
                .iter()
                .map(|(_, m)| *m)
                .collect::<Vec<_>>(),
            [90, 30]
        );
        assert!(assignment_load(&[], &calendar).is_empty());
    }

    #[test]
    fn a_total_effort_without_working_day_cannot_be_placed() {
        let weekend = [segment("2026-11-21", "2026-11-22", Mode::TotalEffort, 60)];
        let calendar = calendar(&[], FULL, &[]);
        assert_eq!(
            load(&weekend[0], &calendar).iter().map(|(_, m)| m).sum::<u32>(),
            0
        );
        assert_eq!(unplaceable(&weekend, &calendar)[0].code, "no_working_day");
        assert!(
            unplaceable(
                &[segment("2026-11-21", "2026-11-23", Mode::TotalEffort, 60)],
                &calendar
            )
            .is_empty()
        );
    }

    #[test]
    fn segments_are_checked_alone_then_together_then_against_the_ticket() {
        let first = segment("2026-11-16", "2026-11-20", Mode::HoursPerDay, 240);
        let second = segment("2026-11-23", "2026-11-27", Mode::Percentage, 30);
        assert_eq!(
            validate(
                &[first, second],
                Some(date("2026-11-16")),
                Some(date("2026-11-27"))
            ),
            Ok(())
        );
        assert_eq!(
            validate(&[first, second], None, None),
            Ok(()),
            "an undated ticket"
        );

        let codes = |result: Result<(), SegmentError>| match result {
            Err(SegmentError::Invalid(e) | SegmentError::Overlap(e) | SegmentError::OutsideIssueDates(e)) => {
                e.into_iter().map(|e| (e.field, e.code)).collect::<Vec<_>>()
            }
            Ok(()) => Vec::new(),
        };
        assert_eq!(
            codes(validate(&[], None, None)),
            [("segments".to_owned(), "required")]
        );
        let bad = [
            segment("2026-11-20", "2026-11-16", Mode::HoursPerDay, 240),
            segment("2026-11-16", "2026-11-16", Mode::Percentage, 101),
            segment("2026-11-16", "2026-11-16", Mode::HoursPerDay, 1441),
            segment("2026-11-16", "2026-11-16", Mode::TotalEffort, 0),
            segment("2026-01-01", "2028-01-02", Mode::Percentage, 10),
        ];
        assert_eq!(
            codes(validate(&bad, None, None)),
            [
                ("segments.0.end".to_owned(), "before_start"),
                ("segments.1.value".to_owned(), "out_of_range"),
                ("segments.2.value".to_owned(), "out_of_range"),
                ("segments.3.value".to_owned(), "out_of_range"),
                ("segments.4.end".to_owned(), "out_of_range"),
            ]
        );
        // AC6: overlapping segments, whatever their order.
        let overlapping = segment("2026-11-20", "2026-11-24", Mode::Percentage, 30);
        let result = validate(&[second, overlapping, first], None, None);
        assert!(matches!(result, Err(SegmentError::Overlap(_))));
        assert_eq!(
            codes(result),
            [
                ("segments.1".to_owned(), "overlap"),
                ("segments.0".to_owned(), "overlap")
            ]
        );
        // AC4: beyond the ticket's dates; a bound the ticket lacks is not checked.
        let result = validate(
            &[first, second],
            Some(date("2026-11-17")),
            Some(date("2026-11-26")),
        );
        assert!(matches!(result, Err(SegmentError::OutsideIssueDates(_))));
        assert_eq!(
            codes(result),
            [
                ("segments.0.start".to_owned(), "outside_issue_dates"),
                ("segments.1.end".to_owned(), "outside_issue_dates")
            ]
        );
        assert_eq!(validate(&[first, second], None, Some(date("2026-11-27"))), Ok(()));
        assert!(outside_issue_dates(
            &[first, second],
            None,
            Some(date("2026-11-26"))
        ));
        assert!(!outside_issue_dates(&[first, second], None, None));
        assert_eq!(
            codes(validate(&[first; MAX_SEGMENTS + 1], None, None)),
            [("segments".to_owned(), "too_many")]
        );
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
    }

    mod properties {
        use super::*;
        use crate::calendars::{Exception, ExceptionKind};
        use proptest::prelude::*;

        fn any_date() -> impl Strategy<Value = Date> {
            (0..1461_i32).prop_map(|offset| date("2025-01-01").add_days(offset))
        }

        proptest! {
            #[test]
            fn effort_is_fully_spread_and_no_minute_lands_on_a_non_working_day(
                week in proptest::array::uniform7(0..=600_u16).prop_map(WeekMinutes),
                start in any_date(),
                length in 0..120_i32,
                closed in proptest::collection::vec(0..120_i32, 0..6),
                mode in 0..3_usize,
                value in 1..=MAX_EFFORT_MINUTES,
                off in proptest::option::of((0..120_i32, 0..3_i32)),
            ) {
                let exceptions: Vec<Exception> = closed
                    .iter()
                    .map(|offset| Exception { date: start.add_days(*offset), kind: ExceptionKind::NonWorking, label: None })
                    .collect();
                let levels = [
                    (Source::Resource, Level { exceptions: &exceptions, holidays: None }),
                    FRANCE[0],
                ];
                let absences: Vec<Absence> = off
                    .map(|(offset, days)| Absence {
                        kind: AbsenceKind::Leave,
                        start: start.add_days(offset),
                        starts_at_noon: false,
                        end: start.add_days(offset + days),
                        ends_at_noon: false,
                    })
                    .into_iter()
                    .collect();
                let calendar = ResourceCalendar { levels: &levels, periods: &[], default: week, absences: &absences };
                let mode = Mode::ALL[mode];
                let value = match mode {
                    Mode::Percentage => value % 100 + 1,
                    Mode::HoursPerDay => value % 1440 + 1,
                    Mode::TotalEffort => value,
                };
                let segment = Segment { start, end: start.add_days(length), mode, value };
                let days = calendar.days(segment.start, segment.end);
                let load = segment_load(&segment, &days);
                prop_assert_eq!(load.len(), days.len());
                for (day, minutes) in days.iter().zip(&load) {
                    if day.scheduled == 0 {
                        prop_assert_eq!(*minutes, 0, "{:?}", day.date);
                    }
                    if mode == Mode::Percentage {
                        prop_assert!(*minutes <= u32::from(day.minutes));
                    }
                }
                let working = days.iter().any(|d| d.scheduled > 0);
                if mode == Mode::TotalEffort && working {
                    prop_assert_eq!(load.iter().sum::<u32>(), value);
                    let placed: Vec<u32> = load.iter().zip(&days).filter(|(_, d)| d.scheduled > 0).map(|(m, _)| *m).collect();
                    let (min, max) = (placed.iter().min().copied().unwrap_or(0), placed.iter().max().copied().unwrap_or(0));
                    prop_assert!(max - min <= 1, "evenly: {} to {}", min, max);
                }
            }
        }
    }
}
