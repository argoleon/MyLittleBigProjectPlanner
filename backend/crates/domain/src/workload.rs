//! The workload heatmap (specification §8, L08-US06): resources by day,
//! week or month, each cell the load against the capacity.
//!
//! - Periods are aligned: a week runs from Monday to Sunday (ISO 8601), a
//!   month from its first to its last day. A range covers every period it
//!   meets.
//! - Weekly and monthly sums are precomputed (`load_aggregate`) over a
//!   horizon of 12 months back and 24 ahead; days are always summed from
//!   the daily load.
//! - The occupation level of a cell: a non-working period, a period off
//!   (the capacity scheduled, taken by absences), or the load against the
//!   capacity, up to the overload, beyond the capacity plus the tenant's
//!   tolerance.
//!
//! No I/O.

use crate::dates::Date;

/// The span of a heatmap column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Day,
    Week,
    Month,
}

impl Scale {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Self::Day, Self::Week, Self::Month]
            .into_iter()
            .find(|scale| scale.as_str() == raw)
    }

    /// The most columns a heatmap reads at once: about three months of
    /// days, a year of weeks, two years of months.
    pub const fn max_periods(self) -> usize {
        match self {
            Self::Day => 92,
            Self::Week => 53,
            Self::Month => 24,
        }
    }
}

/// A period, from `start` to `end` included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Period {
    pub start: Date,
    pub end: Date,
}

impl Period {
    pub fn contains(&self, date: Date) -> bool {
        self.start <= date && date <= self.end
    }
}

fn first_of_month(date: Date) -> Date {
    let (year, month, _) = date.ymd();
    Date::from_ymd(year, month, 1).unwrap_or(date)
}

fn next_month(date: Date) -> Date {
    let (year, month, _) = date.ymd();
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Date::from_ymd(year, month, 1).unwrap_or(date)
}

/// The period of `scale` holding `date`.
pub fn period_of(scale: Scale, date: Date) -> Period {
    match scale {
        Scale::Day => Period {
            start: date,
            end: date,
        },
        Scale::Week => {
            let monday = date.add_days(-i32::try_from(date.weekday().index()).unwrap_or(0));
            Period {
                start: monday,
                end: monday.add_days(6),
            }
        }
        Scale::Month => {
            let start = first_of_month(date);
            Period {
                start,
                end: next_month(start).add_days(-1),
            }
        }
    }
}

/// The periods of `scale` meeting `from`–`to`, in order; none when `to`
/// comes before `from`.
pub fn periods(scale: Scale, from: Date, to: Date) -> Vec<Period> {
    let mut found = Vec::new();
    let mut day = from;
    while day <= to {
        let period = period_of(scale, day);
        found.push(period);
        day = period.end.add_days(1);
    }
    found
}

/// The sum of `values` (minutes by day) over each of `periods`; days
/// outside them are left out.
pub fn sum_by_period(values: &[(Date, u32)], periods: &[Period]) -> Vec<u32> {
    let mut sums = vec![0_u32; periods.len()];
    for (date, minutes) in values {
        // Periods are in order and never overlap.
        let index = periods.partition_point(|p| p.end < *date);
        if let Some(period) = periods.get(index)
            && period.contains(*date)
        {
            sums[index] = sums[index].saturating_add(*minutes);
        }
    }
    sums
}

/// The days whose weekly and monthly sums are kept precomputed, around
/// `today`: from the first day of the month a year before to the last day
/// of the month two years after.
pub fn horizon(today: Date) -> Period {
    let mut start = first_of_month(today);
    for _ in 0..12 {
        start = first_of_month(start.add_days(-1));
    }
    let mut end = first_of_month(today);
    for _ in 0..25 {
        end = next_month(end);
    }
    Period {
        start,
        end: end.add_days(-1),
    }
}

/// How busy a resource is over a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// No working day: nothing was to be planned.
    Off,
    /// Working days, all taken by absences.
    Absent,
    /// Free: nothing planned.
    Free,
    /// Up to 40% of the capacity.
    Low,
    /// Up to 70%.
    Medium,
    /// Up to 95%.
    High,
    /// Up to the capacity plus the tolerance.
    Full,
    /// Beyond the capacity plus the tolerance.
    Over,
}

/// The level of a period of `load` minutes against `capacity` (absences
/// deducted) and `scheduled` (before absences), with the tenant's
/// `tolerance_percent`.
pub fn level(load: u32, capacity: u32, scheduled: u32, tolerance_percent: u8) -> Level {
    let limit = u64::from(capacity) * (100 + u64::from(tolerance_percent)) / 100;
    if u64::from(load) > limit {
        return Level::Over;
    }
    if capacity == 0 {
        return if scheduled == 0 { Level::Off } else { Level::Absent };
    }
    if load == 0 {
        return Level::Free;
    }
    let percent = u64::from(load) * 100 / u64::from(capacity);
    match percent {
        0..=40 => Level::Low,
        41..=70 => Level::Medium,
        71..=95 => Level::High,
        _ => Level::Full,
    }
}

/// The load of a period in percent of its capacity, rounded to the
/// nearest; none without capacity.
pub fn percent(load: u32, capacity: u32) -> Option<u32> {
    (capacity > 0).then(|| {
        let rounded = (u64::from(load) * 100 + u64::from(capacity) / 2) / u64::from(capacity);
        u32::try_from(rounded).unwrap_or(u32::MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn period(start: &str, end: &str) -> Period {
        Period {
            start: date(start),
            end: date(end),
        }
    }

    #[test]
    fn weeks_run_from_monday_and_months_from_their_first_day() {
        assert_eq!(
            period_of(Scale::Week, date("2026-11-18")),
            period("2026-11-16", "2026-11-22")
        );
        assert_eq!(
            period_of(Scale::Week, date("2026-11-22")),
            period("2026-11-16", "2026-11-22")
        );
        // A week across the new year.
        assert_eq!(
            period_of(Scale::Week, date("2027-01-01")),
            period("2026-12-28", "2027-01-03")
        );
        assert_eq!(
            period_of(Scale::Month, date("2028-02-10")),
            period("2028-02-01", "2028-02-29")
        );
        assert_eq!(
            period_of(Scale::Month, date("2026-12-31")),
            period("2026-12-01", "2026-12-31")
        );
        assert_eq!(
            period_of(Scale::Day, date("2026-11-18")),
            period("2026-11-18", "2026-11-18")
        );
    }

    #[test]
    fn a_range_covers_every_period_it_meets() {
        let weeks = periods(Scale::Week, date("2026-11-04"), date("2026-11-17"));
        assert_eq!(
            weeks,
            vec![
                period("2026-11-02", "2026-11-08"),
                period("2026-11-09", "2026-11-15"),
                period("2026-11-16", "2026-11-22"),
            ]
        );
        assert_eq!(
            periods(Scale::Month, date("2026-11-30"), date("2027-01-01")).len(),
            3
        );
        assert_eq!(
            periods(Scale::Day, date("2026-11-16"), date("2026-11-22")).len(),
            7
        );
        assert!(periods(Scale::Day, date("2026-11-16"), date("2026-11-15")).is_empty());
        assert_eq!(Scale::parse("week"), Some(Scale::Week));
        assert_eq!(Scale::parse("year"), None);
        assert_eq!(Scale::Day.max_periods(), 92);
    }

    #[test]
    fn days_add_up_by_period_and_those_outside_are_left_out() {
        let weeks = periods(Scale::Week, date("2026-11-16"), date("2026-11-29"));
        let values = [
            (date("2026-11-15"), 60),
            (date("2026-11-16"), 240),
            (date("2026-11-22"), 30),
            (date("2026-11-23"), 420),
            (date("2026-11-30"), 60),
        ];
        assert_eq!(sum_by_period(&values, &weeks), vec![270, 420]);
        assert_eq!(sum_by_period(&[], &weeks), vec![0, 0]);
    }

    #[test]
    fn the_horizon_spans_a_year_back_and_two_ahead_in_whole_months() {
        assert_eq!(horizon(date("2026-11-26")), period("2025-11-01", "2028-11-30"));
        assert_eq!(horizon(date("2026-01-31")), period("2025-01-01", "2028-01-31"));
    }

    #[test]
    fn levels_follow_the_load_against_the_capacity_and_the_tolerance() {
        // 35 h a week.
        assert_eq!(level(0, 2100, 2100, 0), Level::Free);
        assert_eq!(level(840, 2100, 2100, 0), Level::Low, "40%");
        assert_eq!(level(841, 2100, 2100, 0), Level::Low, "rounded down to 40%");
        assert_eq!(level(1470, 2100, 2100, 0), Level::Medium, "70%");
        assert_eq!(level(1995, 2100, 2100, 0), Level::High, "95%");
        assert_eq!(level(2100, 2100, 2100, 0), Level::Full, "100%");
        // AC2: 112% overloads; with 15% of tolerance it is still full.
        assert_eq!(level(2352, 2100, 2100, 0), Level::Over);
        assert_eq!(level(2352, 2100, 2100, 15), Level::Full);
        // A week off, a week-end, and work planned on either.
        assert_eq!(level(0, 0, 2100, 0), Level::Absent);
        assert_eq!(level(0, 0, 0, 0), Level::Off);
        assert_eq!(level(60, 0, 0, 0), Level::Over);
        assert_eq!(percent(2352, 2100), Some(112));
        assert_eq!(percent(1, 3), Some(33));
        assert_eq!(percent(60, 0), None);
    }
}
