//! Planning dates (specification §7, §18): local calendar dates, never
//! instants. A Monday task stays on Monday whatever the time zone of who
//! reads it. The domain crate has no dependency, so it holds its own small
//! proleptic Gregorian date; the infrastructure converts it from and to the
//! database's `date`.

use std::fmt;

/// A day of the week, Monday first (ISO 8601).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// Every day, Monday first.
    pub const ALL: [Self; 7] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    /// 0 for Monday … 6 for Sunday.
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A calendar date, from year 1 to 9999.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// Days since 1970-01-01.
    days: i32,
}

impl Date {
    /// The date `year-month-day`, if it exists.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Option<Self> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
        {
            return None;
        }
        Some(Self {
            days: days_from_civil(year, month, day),
        })
    }

    /// Year, month (1–12) and day (1–31).
    pub fn ymd(self) -> (i32, u32, u32) {
        civil_from_days(self.days)
    }

    pub fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday.
        Weekday::ALL[usize::try_from((self.days + 3).rem_euclid(7)).unwrap_or(0)]
    }

    /// The date `days` later (earlier when negative).
    #[must_use]
    pub const fn add_days(self, days: i32) -> Self {
        Self {
            days: self.days + days,
        }
    }

    /// Days from `other` to this date.
    pub const fn days_since(self, other: Self) -> i32 {
        self.days - other.days
    }

    /// An ISO 8601 date, `YYYY-MM-DD`.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let number = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = text.get(range)?;
            part.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| part.parse().ok())?
        };
        let year = i32::try_from(number(0..4)?).ok()?;
        Self::from_ymd(year, number(5..7)?, number(8..10)?)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = self.ymd();
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

const fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

const fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 31,
    }
}

// Howard Hinnant's algorithms (public domain), for the proleptic Gregorian
// calendar.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
const fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i32 - 719_468
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
const fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { y + 1 } else { y }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    #[test]
    fn dates_round_trip_through_their_text() {
        for text in [
            "1970-01-01",
            "2026-03-01",
            "2024-02-29",
            "2000-02-29",
            "0001-01-01",
            "9999-12-31",
        ] {
            assert_eq!(date(text).to_string(), text);
        }
    }

    #[test]
    fn impossible_or_malformed_dates_are_refused() {
        for text in [
            "2026-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-04-31",
            "2026-00-10",
            "2026-1-01",
            "26-01-01",
            "2026/01/01",
            "2026-01-0a",
            "0000-01-01",
        ] {
            assert_eq!(Date::parse(text), None, "{text}");
        }
    }

    #[test]
    fn weekdays_follow_the_calendar() {
        assert_eq!(date("1970-01-01").weekday(), Weekday::Thursday);
        assert_eq!(date("2026-11-23").weekday(), Weekday::Monday);
        assert_eq!(date("2026-03-01").weekday(), Weekday::Sunday);
        assert_eq!(date("2024-02-29").weekday(), Weekday::Thursday);
        assert_eq!(date("1969-12-31").weekday(), Weekday::Wednesday);
    }

    #[test]
    fn days_are_added_across_months_years_and_leap_days() {
        assert_eq!(date("2024-02-28").add_days(1), date("2024-02-29"));
        assert_eq!(date("2025-02-28").add_days(1), date("2025-03-01"));
        assert_eq!(date("2026-12-31").add_days(1), date("2027-01-01"));
        assert_eq!(date("2026-03-01").add_days(-1), date("2026-02-28"));
        assert_eq!(date("2027-01-01").days_since(date("2026-01-01")), 365);
        assert!(date("2026-03-01") > date("2026-02-28"));
    }
}
