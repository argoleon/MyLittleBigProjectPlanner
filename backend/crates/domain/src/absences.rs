//! Absences (specification §8, L07-US05): leave, training or other time
//! off, by whole or half days, declared by the person or their manager.
//!
//! - An absence runs from its first day to its last, both included; it may
//!   start at noon (the morning of its first day is worked) and end at noon
//!   (the afternoon of its last day is worked).
//! - A half day off removes half of that day's capacity; a whole day, all
//!   of it. Absences of a resource never overlap, half day by half day.
//! - The person linked to a resource manages its absences, and so do tenant
//!   admins and resource managers. Every internal member sees the dates and
//!   kind; the comment is for the person and the managers only.

use crate::account::TenantRole;
use crate::dates::Date;
use crate::resources::manages_resources;
use crate::validation::{FieldError, Violations};

/// Longest comment, in characters.
pub const COMMENT_MAX_CHARS: usize = 500;

/// Longest absence, in days.
pub const MAX_DAYS: i32 = 366;

/// Kind of an absence. No "sick leave": health data has no place here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AbsenceKind {
    Leave,
    Training,
    Other,
}

impl AbsenceKind {
    pub const ALL: [Self; 3] = [Self::Leave, Self::Training, Self::Other];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Leave => "leave",
            Self::Training => "training",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == value)
    }
}

/// An absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Absence {
    pub kind: AbsenceKind,
    pub start: Date,
    /// The first day's morning is worked.
    pub starts_at_noon: bool,
    pub end: Date,
    /// The last day's afternoon is worked.
    pub ends_at_noon: bool,
}

/// Which part of a day an absence takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayPart {
    Whole,
    Morning,
    Afternoon,
}

impl Absence {
    /// Half days since 1970-01-01, the first one included and the last
    /// excluded: two absences overlap when these meet.
    fn halves(&self) -> (i64, i64) {
        let epoch = Date::from_ymd(1970, 1, 1).map_or(0, |e| i64::from(self.start.days_since(e)));
        let from = epoch * 2 + i64::from(self.starts_at_noon);
        let last = i64::from(self.end.days_since(self.start)) + epoch;
        let until = last * 2 + if self.ends_at_noon { 1 } else { 2 };
        (from, until)
    }

    /// Whether the two absences take a same half day.
    pub fn overlaps(&self, other: &Self) -> bool {
        let (a, b) = (self.halves(), other.halves());
        a.0 < b.1 && b.0 < a.1
    }

    /// The part of `date` it takes, if any.
    pub fn part_of(&self, date: Date) -> Option<DayPart> {
        if date < self.start || date > self.end {
            return None;
        }
        let morning = !(date == self.start && self.starts_at_noon);
        let afternoon = !(date == self.end && self.ends_at_noon);
        match (morning, afternoon) {
            (true, true) => Some(DayPart::Whole),
            (true, false) => Some(DayPart::Morning),
            (false, true) => Some(DayPart::Afternoon),
            (false, false) => None,
        }
    }
}

/// The capacity left of a day of `minutes` once `part` of it is off.
pub const fn minutes_left(minutes: u16, part: Option<DayPart>) -> u16 {
    match part {
        None => minutes,
        Some(DayPart::Whole) => 0,
        // The other half stays: 3 h 30 of a 7 h day.
        Some(DayPart::Morning | DayPart::Afternoon) => minutes - minutes / 2,
    }
}

/// Checks an absence and its comment.
pub fn validate(absence: &Absence, comment: Option<&str>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if absence.end < absence.start {
        v.add("end", "before_start");
    } else if absence.end.days_since(absence.start) >= MAX_DAYS {
        v.add("end", "out_of_range");
    } else if absence.start == absence.end && absence.starts_at_noon && absence.ends_at_noon {
        // From noon to noon the same day: nothing.
        v.add("endsAtNoon", "not_allowed");
    }
    if comment
        .map(str::trim)
        .is_some_and(|c| c.chars().count() > COMMENT_MAX_CHARS)
    {
        v.add("comment", "too_long");
    }
    v.into_result()
}

/// Whether a member manages the absences of a resource: the person it is,
/// or a tenant admin or resource manager.
pub const fn manages(is_the_person: bool, role: TenantRole) -> bool {
    is_the_person || manages_resources(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn absence(start: &str, starts_at_noon: bool, end: &str, ends_at_noon: bool) -> Absence {
        Absence {
            kind: AbsenceKind::Leave,
            start: date(start),
            starts_at_noon,
            end: date(end),
            ends_at_noon,
        }
    }

    #[test]
    fn a_whole_day_off_leaves_no_capacity_and_a_half_day_half_of_it() {
        let leave = absence("2026-12-04", false, "2026-12-04", false);
        assert_eq!(leave.part_of(date("2026-12-04")), Some(DayPart::Whole));
        assert_eq!(minutes_left(420, leave.part_of(date("2026-12-04"))), 0);
        assert_eq!(leave.part_of(date("2026-12-05")), None);

        let training = absence("2026-12-07", true, "2026-12-07", false);
        assert_eq!(training.part_of(date("2026-12-07")), Some(DayPart::Afternoon));
        assert_eq!(
            minutes_left(420, Some(DayPart::Afternoon)),
            210,
            "3 h 30 of a 7 h day"
        );
        assert_eq!(minutes_left(211, Some(DayPart::Morning)), 106);
        assert_eq!(minutes_left(420, None), 420);
    }

    #[test]
    fn a_range_may_start_and_end_at_noon() {
        let trip = absence("2026-12-07", true, "2026-12-09", true);
        assert_eq!(trip.part_of(date("2026-12-07")), Some(DayPart::Afternoon));
        assert_eq!(trip.part_of(date("2026-12-08")), Some(DayPart::Whole));
        assert_eq!(trip.part_of(date("2026-12-09")), Some(DayPart::Morning));
        assert_eq!(trip.part_of(date("2026-12-10")), None);
    }

    #[test]
    fn absences_overlap_half_day_by_half_day() {
        let morning = absence("2026-12-07", false, "2026-12-07", true);
        let afternoon = absence("2026-12-07", true, "2026-12-07", false);
        let whole = absence("2026-12-07", false, "2026-12-07", false);
        assert!(
            !morning.overlaps(&afternoon),
            "a morning and an afternoon fit the same day"
        );
        assert!(whole.overlaps(&morning) && whole.overlaps(&afternoon));
        let week = absence("2026-12-07", false, "2026-12-11", false);
        let next = absence("2026-12-11", true, "2026-12-14", false);
        assert!(week.overlaps(&next));
        let after = absence("2026-12-12", false, "2026-12-14", false);
        assert!(!week.overlaps(&after));
        // Ends at noon on Friday, the afternoon is free.
        let to_noon = absence("2026-12-07", false, "2026-12-11", true);
        assert!(!to_noon.overlaps(&next));
    }

    #[test]
    fn an_absence_ends_after_it_starts_within_a_year_and_its_comment_is_short() {
        let codes = |a: &Absence, comment: Option<&str>| -> Vec<(String, &'static str)> {
            validate(a, comment)
                .err()
                .unwrap_or_default()
                .into_iter()
                .map(|e| (e.field, e.code))
                .collect()
        };
        assert!(validate(&absence("2026-12-04", false, "2026-12-04", true), Some("Dentist")).is_ok());
        assert_eq!(
            codes(&absence("2026-12-05", false, "2026-12-04", false), None),
            [("end".to_owned(), "before_start")]
        );
        assert_eq!(
            codes(&absence("2026-01-01", false, "2027-01-02", false), None),
            [("end".to_owned(), "out_of_range")]
        );
        assert_eq!(
            codes(&absence("2026-12-04", true, "2026-12-04", true), None),
            [("endsAtNoon".to_owned(), "not_allowed")]
        );
        assert_eq!(
            codes(
                &absence("2026-12-04", false, "2026-12-04", false),
                Some(&"x".repeat(COMMENT_MAX_CHARS + 1))
            ),
            [("comment".to_owned(), "too_long")]
        );
    }

    #[test]
    fn the_person_and_the_managers_manage_absences() {
        assert!(manages(true, TenantRole::Member));
        assert!(manages(false, TenantRole::ResourceManager));
        assert!(manages(false, TenantRole::TenantAdmin));
        assert!(!manages(false, TenantRole::Member));
        assert!(!manages(false, TenantRole::ConfigurationAdmin));
        for kind in AbsenceKind::ALL {
            assert_eq!(AbsenceKind::parse(kind.as_str()), Some(kind));
        }
    }
}
