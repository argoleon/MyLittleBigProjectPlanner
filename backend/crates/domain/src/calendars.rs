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
use crate::holidays::{Holiday, HolidayName, HolidaySet};
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
    status_with(levels, date, HolidaySet::holiday_on)
}

/// [`status_on`], public holidays coming from `holiday_on`.
fn status_with(
    levels: &[(Source, Level<'_>)],
    date: Date,
    holiday_on: impl Fn(HolidaySet, Date) -> Option<Holiday>,
) -> (Status, Option<Source>) {
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
        if let Some(holidays) = level.holidays {
            return match holiday_on(holidays, date) {
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
    /// Minutes the calendar and the capacity give the day, before absences:
    /// 0 on a non-working day. A day of more than 0 is a working day.
    pub scheduled: u16,
    /// Minutes of capacity: `scheduled` less an absence.
    pub minutes: u16,
}

/// Everything the capacity of a resource depends on (§8, L07-US06): its
/// calendar `levels` (resource, site, tenant), its capacity `periods` over
/// the tenant's `default` week, and its `absences`. Loaded by infra, it is
/// the single source of the capacity of a day: capacity = weekly pattern,
/// less the public holidays and closures, less the absences.
#[derive(Debug, Clone, Copy)]
pub struct ResourceCalendar<'a> {
    pub levels: &'a [(Source, Level<'a>)],
    pub periods: &'a [Period],
    pub default: WeekMinutes,
    pub absences: &'a [Absence],
}

impl ResourceCalendar<'_> {
    /// The day `date` of the resource.
    pub fn day(&self, date: Date) -> Day {
        self.day_with(date, status_on(self.levels, date))
    }

    /// The day `date`, of status and source `decided`.
    fn day_with(&self, date: Date, decided: (Status, Option<Source>)) -> Day {
        let (status, source) = decided;
        let mut absence = None;
        let (mut morning, mut afternoon) = (false, false);
        for (kind, part) in self
            .absences
            .iter()
            .filter_map(|a| a.part_of(date).map(|part| (a.kind, part)))
        {
            absence.get_or_insert((kind, part));
            morning |= matches!(part, DayPart::Whole | DayPart::Morning);
            afternoon |= matches!(part, DayPart::Whole | DayPart::Afternoon);
        }
        // A morning off and an afternoon off, of two absences: the whole day.
        let off = match (morning, afternoon) {
            (true, true) => Some(DayPart::Whole),
            (true, false) => Some(DayPart::Morning),
            (false, true) => Some(DayPart::Afternoon),
            (false, false) => None,
        };
        let scheduled = match status {
            Status::Working => capacity::minutes_on(self.periods, self.default, date),
            Status::Holiday { .. } | Status::NonWorking { .. } => 0,
        };
        Day {
            date,
            status,
            source,
            absence,
            scheduled,
            minutes: absences::minutes_left(scheduled, off),
        }
    }

    /// The capacity of `date`, in minutes.
    pub fn minutes_on(&self, date: Date) -> u16 {
        self.day(date).minutes
    }

    /// The days from `from` to `to` included; none when `to` is before `from`.
    pub fn days(&self, from: Date, to: Date) -> Vec<Day> {
        let count = to.days_since(from);
        if count < 0 {
            return Vec::new();
        }
        // The holidays of each set and year of the range, computed once.
        let mut known: Vec<(HolidaySet, i32, Vec<Holiday>)> = Vec::new();
        for holidays in self.levels.iter().filter_map(|(_, level)| level.holidays) {
            for year in from.ymd().0..=to.ymd().0 {
                if !known.iter().any(|(h, y, _)| *h == holidays && *y == year) {
                    known.push((holidays, year, holidays.holidays(year)));
                }
            }
        }
        let holiday_on = |holidays: HolidaySet, date: Date| {
            let year = date.ymd().0;
            known
                .iter()
                .find(|(h, y, _)| *h == holidays && *y == year)
                .and_then(|(_, _, days)| days.iter().find(|d| d.date == date).copied())
        };
        (0..=count)
            .map(|offset| {
                let date = from.add_days(offset);
                self.day_with(date, status_with(self.levels, date, holiday_on))
            })
            .collect()
    }

    /// The capacity of each day from `from` to `to` included, in minutes.
    pub fn minutes(&self, from: Date, to: Date) -> Vec<u16> {
        self.days(from, to).into_iter().map(|d| d.minutes).collect()
    }
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

    fn days(
        levels: &[(Source, Level<'_>)],
        periods: &[Period],
        default: WeekMinutes,
        absences: &[Absence],
        from: Date,
        to: Date,
    ) -> Vec<Day> {
        ResourceCalendar {
            levels,
            periods,
            default,
            absences,
        }
        .days(from, to)
    }

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
    fn capacity_over_christmas_takes_off_the_holiday_and_the_leave() {
        // L07-US06 AC1: 7 h pattern, Christmas, a leave on 28 December.
        let levels = [tenant(&[])];
        let leave = [absence("2026-12-28", false, "2026-12-28", false)];
        let calendar = ResourceCalendar {
            levels: &levels,
            periods: &[],
            default: WEEK,
            absences: &leave,
        };
        assert_eq!(
            calendar.minutes(date("2026-12-21"), date("2026-12-31")),
            [420, 420, 420, 420, 0, 0, 0, 0, 420, 420, 420],
            "Mon 21 – Thu 24 worked, Christmas, weekend, the leave, Tue 29 – Thu 31 worked"
        );
        assert_eq!(calendar.minutes_on(date("2026-12-25")), 0);
        assert_eq!(calendar.minutes_on(date("2026-12-28")), 0);
    }

    #[test]
    fn a_part_time_wednesday_has_no_capacity() {
        // L07-US06 AC2.
        let levels = [tenant(&[])];
        let periods = [Period {
            effective_from: date("2026-01-01"),
            week: WeekMinutes([420, 420, 0, 420, 420, 0, 0]),
        }];
        let calendar = ResourceCalendar {
            levels: &levels,
            periods: &periods,
            default: WEEK,
            absences: &[],
        };
        // 2026-11-23 is a Monday.
        assert_eq!(
            calendar.minutes(date("2026-11-23"), date("2026-11-29")),
            [420, 420, 0, 420, 420, 0, 0]
        );
    }

    #[test]
    fn a_half_day_off_a_short_friday_leaves_half_of_it() {
        // L07-US06 AC3: half of a 3.5 h Friday is 1 h 45.
        let levels = [tenant(&[])];
        let short_friday = WeekMinutes([420, 420, 420, 420, 210, 0, 0]);
        let afternoon = [absence("2026-11-27", true, "2026-11-27", false)];
        let calendar = ResourceCalendar {
            levels: &levels,
            periods: &[],
            default: short_friday,
            absences: &afternoon,
        };
        assert_eq!(calendar.minutes_on(date("2026-11-27")), 105);
        assert_eq!(
            calendar.day(date("2026-11-27")).absence,
            Some((AbsenceKind::Leave, DayPart::Afternoon))
        );
    }

    #[test]
    fn each_day_uses_the_period_in_force_that_day() {
        // L07-US06 AC4: the 80% period starts on Wednesday 2027-03-03.
        let levels = [tenant(&[])];
        let periods = [
            Period {
                effective_from: date("2026-01-01"),
                week: WEEK,
            },
            Period {
                effective_from: date("2027-03-03"),
                week: WeekMinutes([420, 420, 0, 420, 420, 0, 0]),
            },
        ];
        let calendar = ResourceCalendar {
            levels: &levels,
            periods: &periods,
            default: WeekMinutes([0; 7]),
            absences: &[],
        };
        assert_eq!(
            calendar.minutes(date("2027-03-01"), date("2027-03-10")),
            [420, 420, 0, 420, 420, 0, 0, 420, 420, 0],
            "Monday 1 and Tuesday 2 full time, then Wednesdays off"
        );
    }

    #[test]
    fn a_morning_off_and_an_afternoon_off_take_the_whole_day() {
        let levels = [tenant(&[])];
        let two_halves = [
            absence("2026-11-20", false, "2026-11-23", true),
            Absence {
                kind: AbsenceKind::Training,
                ..absence("2026-11-23", true, "2026-11-24", false)
            },
        ];
        let calendar = ResourceCalendar {
            levels: &levels,
            periods: &[],
            default: WEEK,
            absences: &two_halves,
        };
        assert_eq!(calendar.minutes_on(date("2026-11-23")), 0);
        // The first absence of the day is the one shown.
        assert_eq!(
            calendar.day(date("2026-11-23")).absence,
            Some((AbsenceKind::Leave, DayPart::Morning))
        );
        assert_eq!(calendar.minutes_on(date("2026-11-25")), 420);
    }

    #[test]
    fn days_around_a_daylight_saving_change_keep_their_capacity() {
        // Planning dates are local dates: the weekends when France changes
        // time (last Sundays of March and October) change nothing.
        let levels = [tenant(&[])];
        let calendar = ResourceCalendar {
            levels: &levels,
            periods: &[],
            default: WeekMinutes([420, 420, 420, 420, 420, 240, 120]),
            absences: &[],
        };
        for (friday, monday) in [("2026-03-27", "2026-03-30"), ("2026-10-23", "2026-10-26")] {
            let days = calendar.days(date(friday), date(monday));
            assert_eq!(
                days.iter().map(|d| d.minutes).collect::<Vec<_>>(),
                [420, 240, 120, 420],
                "{friday}"
            );
            assert_eq!(days[3].date, date(monday));
        }
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn any_date() -> impl Strategy<Value = Date> {
            // 2025-01-01 and the 4 years after it.
            (0..1461_i32).prop_map(|offset| date("2025-01-01").add_days(offset))
        }

        fn any_week() -> impl Strategy<Value = WeekMinutes> {
            proptest::array::uniform7(0..=1440_u16).prop_map(WeekMinutes)
        }

        fn any_absence() -> impl Strategy<Value = Absence> {
            (any_date(), 0..20_i32, any::<bool>(), any::<bool>(), 0..3_usize).prop_map(
                |(start, length, starts_at_noon, ends_at_noon, kind)| Absence {
                    kind: AbsenceKind::ALL[kind],
                    start,
                    starts_at_noon,
                    end: start.add_days(length),
                    ends_at_noon,
                },
            )
        }

        fn any_exception() -> impl Strategy<Value = Exception> {
            (any_date(), 0..3_usize).prop_map(|(date, kind)| Exception {
                date,
                kind: ExceptionKind::ALL[kind],
                label: None,
            })
        }

        proptest! {
            #[test]
            fn capacity_is_never_above_the_pattern_and_a_range_matches_its_days(
                default in any_week(),
                periods in proptest::collection::vec((any_date(), any_week()), 0..4),
                absences in proptest::collection::vec(any_absence(), 0..6),
                exceptions in proptest::collection::vec(any_exception(), 0..8),
                from in any_date(),
                length in 0..120_i32,
            ) {
                let periods: Vec<Period> = periods
                    .into_iter()
                    .map(|(effective_from, week)| Period { effective_from, week })
                    .collect();
                let levels = [
                    (Source::Resource, Level { exceptions: &exceptions, holidays: None }),
                    tenant(&[]),
                ];
                let calendar = ResourceCalendar {
                    levels: &levels,
                    periods: &periods,
                    default,
                    absences: &absences,
                };
                let to = from.add_days(length);
                let minutes = calendar.minutes(from, to);
                let days = calendar.days(from, to);
                prop_assert_eq!(minutes.len(), usize::try_from(length + 1).unwrap_or(0));
                for ((offset, day_minutes), ranged) in (0..).zip(&minutes).zip(&days) {
                    let date = from.add_days(offset);
                    let pattern = capacity::minutes_on(&periods, default, date);
                    prop_assert!(*day_minutes <= pattern, "{:?}: {} > {}", date, day_minutes, pattern);
                    prop_assert_eq!(*day_minutes, calendar.minutes_on(date));
                    let day = calendar.day(date);
                    // A range and a single day agree (holidays computed once per year).
                    prop_assert_eq!(ranged, &day);
                    if !matches!(day.status, Status::Working) {
                        prop_assert_eq!(*day_minutes, 0);
                    }
                    if day.absence.is_none() && matches!(day.status, Status::Working) {
                        prop_assert_eq!(*day_minutes, pattern);
                    }
                }
            }
        }
    }
}
