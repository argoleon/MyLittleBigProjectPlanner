//! Absence conflicts (specification §8, L08-US04): hours planned in hours
//! per day or total effort stay on their days when the resource is away,
//! nothing moves by itself; the hours an absence leaves no room for are to
//! be rescheduled by the project manager.
//!
//! - A day of an assignment is in conflict when an absence takes (part of)
//!   it and the assignment's load that day exceeds the capacity the
//!   absence leaves: a whole day off puts all its hours in conflict, 4 h
//!   on a day with 3 h 30 left after a half day off puts 30 min.
//! - Percentage segments never conflict: their load follows the capacity
//!   down, to 0 on a day off.
//! - Public holidays and closures are not absences: those days are not
//!   working days, nothing is planned on them.
//!
//! No I/O: infra keeps the conflicts of each assignment up to date.

use crate::assignments::{Mode, Segment, segment_load};
use crate::calendars::ResourceCalendar;
use crate::dates::Date;

/// The minutes of an assignment of `segments` to reschedule on each day,
/// by date, on the resource's `calendar`; days without conflict left out.
pub fn absence_conflicts(segments: &[Segment], calendar: &ResourceCalendar<'_>) -> Vec<(Date, u32)> {
    let mut conflicts: Vec<(Date, u32)> = Vec::new();
    for segment in segments.iter().filter(|s| s.mode != Mode::Percentage) {
        let days = calendar.days(segment.start, segment.end);
        let load = segment_load(segment, &days);
        for (day, minutes) in days.iter().zip(load) {
            let left = u32::from(day.minutes);
            if day.absence.is_some() && minutes > left {
                conflicts.push((day.date, minutes - left));
            }
        }
    }
    conflicts.sort_by_key(|(date, _)| *date);
    // Segments never overlap; a day two of them would share gets their sum.
    conflicts.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            kept.1 += later.1;
        }
        same
    });
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::absences::{Absence, AbsenceKind};
    use crate::calendars::{Level, Source};
    use crate::capacity::WeekMinutes;
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
    const FRANCE: [(Source, Level<'static>); 1] = [(
        Source::Tenant,
        Level {
            exceptions: &[],
            holidays: Some(HolidaySet::France),
        },
    )];

    fn leave(start: &str, end: &str, starts_at_noon: bool, ends_at_noon: bool) -> Absence {
        Absence {
            kind: AbsenceKind::Leave,
            start: date(start),
            starts_at_noon,
            end: date(end),
            ends_at_noon,
        }
    }

    fn conflicts(segments: &[Segment], absences: &[Absence]) -> Vec<(Date, u32)> {
        let calendar = ResourceCalendar {
            levels: &FRANCE,
            periods: &[],
            default: FULL,
            absences,
        };
        absence_conflicts(segments, &calendar)
    }

    #[test]
    fn a_day_of_leave_puts_all_its_planned_hours_in_conflict() {
        // AC1: 7 h/day on PROJ-42, a leave on Wednesday 18 November 2026.
        let found = conflicts(
            &[segment("2026-11-16", "2026-11-20", Mode::HoursPerDay, 420)],
            &[leave("2026-11-18", "2026-11-18", false, false)],
        );
        assert_eq!(found, vec![(date("2026-11-18"), 420)]);
    }

    #[test]
    fn a_half_day_off_leaves_room_for_part_of_the_hours() {
        // 4 h planned, the afternoon off: 3 h 30 left, 30 min to reschedule.
        let afternoon = [leave("2026-11-18", "2026-11-18", true, false)];
        let found = conflicts(
            &[segment("2026-11-18", "2026-11-18", Mode::HoursPerDay, 240)],
            &afternoon,
        );
        assert_eq!(found, vec![(date("2026-11-18"), 30)]);
        // 3 h fit in what is left.
        assert!(
            conflicts(
                &[segment("2026-11-18", "2026-11-18", Mode::HoursPerDay, 180)],
                &afternoon
            )
            .is_empty()
        );
    }

    #[test]
    fn percentages_never_conflict() {
        // AC2: the load drops to 0 by itself.
        let found = conflicts(
            &[segment("2026-11-16", "2026-11-20", Mode::Percentage, 100)],
            &[leave("2026-11-16", "2026-11-20", false, false)],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn total_effort_keeps_its_share_on_the_days_off() {
        // 35 h over a week: 7 h a day, the Thursday and Friday off.
        let found = conflicts(
            &[segment("2026-11-16", "2026-11-20", Mode::TotalEffort, 2100)],
            &[leave("2026-11-19", "2026-11-20", false, false)],
        );
        assert_eq!(found, vec![(date("2026-11-19"), 420), (date("2026-11-20"), 420)]);
    }

    #[test]
    fn public_holidays_are_not_absences_and_segments_add_up_by_day() {
        // Armistice Day: nothing is planned on it, nothing conflicts.
        assert!(
            conflicts(
                &[segment("2026-11-09", "2026-11-13", Mode::HoursPerDay, 420)],
                &[]
            )
            .is_empty()
        );
        // Two segments around a leave spanning both.
        let found = conflicts(
            &[
                segment("2026-11-16", "2026-11-17", Mode::HoursPerDay, 120),
                segment("2026-11-18", "2026-11-20", Mode::TotalEffort, 600),
            ],
            &[leave("2026-11-17", "2026-11-18", false, false)],
        );
        assert_eq!(found, vec![(date("2026-11-17"), 120), (date("2026-11-18"), 200)]);
    }
}
