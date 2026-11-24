//! Public holidays (specification §7, L07-US04), computed for any year
//! rather than stored: a calendar names its holiday set, and only a
//! tenant's changes to it are kept (`crate::calendars`).
//!
//! France first; the Alsace-Moselle variant adds Good Friday and
//! St Stephen's Day.

use crate::dates::Date;

/// A set of public holidays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HolidaySet {
    /// Metropolitan France.
    France,
    /// France, Alsace and Moselle.
    FranceAlsaceMoselle,
    /// No public holiday.
    None,
}

impl HolidaySet {
    pub const ALL: [Self; 3] = [Self::France, Self::FranceAlsaceMoselle, Self::None];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::France => "fr",
            Self::FranceAlsaceMoselle => "fr-alsace-moselle",
            Self::None => "none",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }

    /// The holidays of `year`, by date.
    pub fn holidays(self, year: i32) -> Vec<Holiday> {
        let mut days = match self {
            Self::None => Vec::new(),
            Self::France => france(year),
            Self::FranceAlsaceMoselle => {
                let mut days = france(year);
                days.extend(easter(year).map(|e| holiday(e.add_days(-2), HolidayName::GoodFriday)));
                days.extend(fixed(year, 12, 26, HolidayName::StStephensDay));
                days
            }
        };
        days.sort_by_key(|h| h.date);
        days
    }

    /// The holiday of `date`, if it is one.
    pub fn holiday_on(self, date: Date) -> Option<Holiday> {
        let (year, _, _) = date.ymd();
        self.holidays(year).into_iter().find(|h| h.date == date)
    }
}

/// Which holiday a day is: a stable key, translated by the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HolidayName {
    NewYearsDay,
    GoodFriday,
    EasterMonday,
    LabourDay,
    VictoryInEurope,
    AscensionDay,
    WhitMonday,
    BastilleDay,
    Assumption,
    AllSaintsDay,
    Armistice,
    Christmas,
    StStephensDay,
}

impl HolidayName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NewYearsDay => "new_years_day",
            Self::GoodFriday => "good_friday",
            Self::EasterMonday => "easter_monday",
            Self::LabourDay => "labour_day",
            Self::VictoryInEurope => "victory_in_europe",
            Self::AscensionDay => "ascension_day",
            Self::WhitMonday => "whit_monday",
            Self::BastilleDay => "bastille_day",
            Self::Assumption => "assumption",
            Self::AllSaintsDay => "all_saints_day",
            Self::Armistice => "armistice",
            Self::Christmas => "christmas",
            Self::StStephensDay => "st_stephens_day",
        }
    }
}

/// A public holiday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holiday {
    pub date: Date,
    pub name: HolidayName,
}

const fn holiday(date: Date, name: HolidayName) -> Holiday {
    Holiday { date, name }
}

fn fixed(year: i32, month: u32, day: u32, name: HolidayName) -> Option<Holiday> {
    Date::from_ymd(year, month, day).map(|date| holiday(date, name))
}

fn france(year: i32) -> Vec<Holiday> {
    let mut days: Vec<Holiday> = [
        fixed(year, 1, 1, HolidayName::NewYearsDay),
        fixed(year, 5, 1, HolidayName::LabourDay),
        fixed(year, 5, 8, HolidayName::VictoryInEurope),
        fixed(year, 7, 14, HolidayName::BastilleDay),
        fixed(year, 8, 15, HolidayName::Assumption),
        fixed(year, 11, 1, HolidayName::AllSaintsDay),
        fixed(year, 11, 11, HolidayName::Armistice),
        fixed(year, 12, 25, HolidayName::Christmas),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(easter) = easter(year) {
        days.push(holiday(easter.add_days(1), HolidayName::EasterMonday));
        days.push(holiday(easter.add_days(39), HolidayName::AscensionDay));
        days.push(holiday(easter.add_days(50), HolidayName::WhitMonday));
    }
    days
}

/// Easter Sunday of `year` (Gregorian calendar, anonymous algorithm).
pub fn easter(year: i32) -> Option<Date> {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    Date::from_ymd(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    #[test]
    fn easter_falls_on_the_known_sundays() {
        for (year, day) in [
            (2024, "2024-03-31"),
            (2025, "2025-04-20"),
            (2026, "2026-04-05"),
            (2027, "2027-03-28"),
            (2038, "2038-04-25"),
            (2285, "2285-03-22"),
        ] {
            assert_eq!(easter(year), Some(date(day)), "{year}");
        }
    }

    #[test]
    fn france_has_eleven_holidays_in_2026() {
        let days: Vec<String> = HolidaySet::France
            .holidays(2026)
            .iter()
            .map(|h| h.date.to_string())
            .collect();
        assert_eq!(
            days,
            [
                "2026-01-01",
                "2026-04-06",
                "2026-05-01",
                "2026-05-08",
                "2026-05-14",
                "2026-05-25",
                "2026-07-14",
                "2026-08-15",
                "2026-11-01",
                "2026-11-11",
                "2026-12-25",
            ]
        );
        assert_eq!(
            HolidaySet::France.holiday_on(date("2026-12-25")).map(|h| h.name),
            Some(HolidayName::Christmas)
        );
        assert_eq!(HolidaySet::France.holiday_on(date("2026-12-24")), None);
    }

    #[test]
    fn alsace_moselle_adds_good_friday_and_st_stephen() {
        let days = HolidaySet::FranceAlsaceMoselle.holidays(2026);
        assert_eq!(days.len(), 13);
        assert_eq!(
            HolidaySet::FranceAlsaceMoselle
                .holiday_on(date("2026-04-03"))
                .map(|h| h.name),
            Some(HolidayName::GoodFriday)
        );
        assert_eq!(
            HolidaySet::FranceAlsaceMoselle
                .holiday_on(date("2026-12-26"))
                .map(|h| h.name),
            Some(HolidayName::StStephensDay)
        );
        assert!(HolidaySet::None.holidays(2026).is_empty());
    }

    #[test]
    fn sets_round_trip() {
        for set in HolidaySet::ALL {
            assert_eq!(HolidaySet::parse(set.as_str()), Some(set));
        }
        assert_eq!(HolidaySet::parse("de"), None);
    }
}
