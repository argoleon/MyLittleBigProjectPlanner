//! Rules of tenant settings (specification §3, §8), managed by the platform
//! super admin.

use crate::i18n;
use crate::tenancy::is_valid_slug;
use crate::validation::{FieldError, Violations};

pub const NAME_MAX_CHARS: usize = 200;

/// Default weekly capacity, in minutes per weekday from Monday: 7 h × 5 days.
pub const DEFAULT_CAPACITY_MINUTES: [i32; 7] = [420, 420, 420, 420, 420, 0, 0];

/// Minutes in a day: the most a capacity can hold.
pub const DAY_MINUTES: i64 = 24 * 60;

/// Highest overload tolerance, in percent of the capacity.
pub const MAX_OVERLOAD_TOLERANCE_PERCENT: i64 = 100;

/// Highest API rate quota, in requests per minute.
pub const MAX_API_RATE_PER_MINUTE: i64 = 1_000_000;

/// What happens when an assignment exceeds capacity plus tolerance (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverloadMode {
    /// The assignment is refused (default).
    Blocking,
    /// The assignment is accepted and the overload flagged.
    Warning,
}

impl OverloadMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blocking => "blocking",
            Self::Warning => "warning",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "blocking" => Some(Self::Blocking),
            "warning" => Some(Self::Warning),
            _ => None,
        }
    }
}

/// The settings of a tenant, as a client sends them. Numbers are wide so
/// that out-of-range values are reported as such, not as malformed JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantSettingsInput {
    pub name: String,
    pub default_locale: String,
    /// IANA name, e.g. `Europe/Paris`.
    pub time_zone: String,
    /// Minutes per weekday, Monday first.
    pub default_capacity_minutes: Vec<i64>,
    pub overload_tolerance_percent: i64,
    pub overload_mode: String,
    /// `None`: unlimited.
    pub storage_quota_bytes: Option<i64>,
    /// `None`: unlimited.
    pub api_rate_limit_per_minute: Option<i64>,
}

impl TenantSettingsInput {
    /// The defaults of a new tenant named `name`.
    pub fn defaults(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            default_locale: i18n::SUPPORTED_LOCALES[0].to_owned(),
            time_zone: "Europe/Paris".to_owned(),
            default_capacity_minutes: DEFAULT_CAPACITY_MINUTES.iter().map(|m| i64::from(*m)).collect(),
            overload_tolerance_percent: 0,
            overload_mode: OverloadMode::Blocking.as_str().to_owned(),
            storage_quota_bytes: None,
            api_rate_limit_per_minute: None,
        }
    }
}

/// Checks `settings`; field names are those of the JSON API. The time zone
/// is only checked for its shape here: the database knows the real list.
pub fn validate_settings(settings: &TenantSettingsInput) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    check_settings(settings, &mut v);
    v.into_result()
}

/// Checks a new tenant: its `slug` (its host name label, immutable) and
/// settings. In single-tenant mode, `single_slug` is the only slug served.
pub fn validate_new_tenant(
    slug: &str,
    settings: &TenantSettingsInput,
    single_slug: Option<&str>,
) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if slug.is_empty() {
        v.add("slug", "required");
    } else if !is_valid_slug(slug) {
        v.add("slug", "format");
    } else if single_slug.is_some_and(|only| only != slug) {
        v.add("slug", "not_default_slug");
    }
    check_settings(settings, &mut v);
    v.into_result()
}

fn check_settings(s: &TenantSettingsInput, v: &mut Violations) {
    let name_chars = s.name.trim().chars().count();
    if name_chars == 0 {
        v.add("name", "required");
    } else if name_chars > NAME_MAX_CHARS {
        v.add("name", "too_long");
    }
    if !i18n::is_supported(&s.default_locale) {
        v.add("defaultLocale", "unsupported_locale");
    }
    if !is_plausible_time_zone(&s.time_zone) {
        v.add("timeZone", "unknown_time_zone");
    }
    if s.default_capacity_minutes.len() == 7 {
        for (day, minutes) in s.default_capacity_minutes.iter().enumerate() {
            if !(0..=DAY_MINUTES).contains(minutes) {
                v.add(format!("defaultCapacityMinutes.{day}"), "out_of_range");
            }
        }
    } else {
        v.add("defaultCapacityMinutes", "length");
    }
    if !(0..=MAX_OVERLOAD_TOLERANCE_PERCENT).contains(&s.overload_tolerance_percent) {
        v.add("overloadTolerancePercent", "out_of_range");
    }
    if OverloadMode::parse(&s.overload_mode).is_none() {
        v.add("overloadMode", "format");
    }
    if s.storage_quota_bytes.is_some_and(|q| q < 1) {
        v.add("storageQuotaBytes", "out_of_range");
    }
    if s.api_rate_limit_per_minute
        .is_some_and(|q| !(1..=MAX_API_RATE_PER_MINUTE).contains(&q))
    {
        v.add("apiRateLimitPerMinute", "out_of_range");
    }
}

/// Whether `name` has the shape of an IANA time zone: `UTC`, or segments of
/// letters, digits, `_`, `+` and `-` separated by `/` (`Europe/Paris`,
/// `America/Argentina/Buenos_Aires`, `Etc/GMT+5`).
pub fn is_plausible_time_zone(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(result: Result<(), Vec<FieldError>>) -> Vec<(String, &'static str)> {
        result
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn the_defaults_are_valid_and_follow_the_specification() {
        let defaults = TenantSettingsInput::defaults("Acme");
        assert_eq!(validate_settings(&defaults), Ok(()));
        assert_eq!(defaults.default_locale, "fr");
        assert_eq!(defaults.default_capacity_minutes, [420, 420, 420, 420, 420, 0, 0]);
        assert_eq!(defaults.overload_tolerance_percent, 0, "0% by default (§8)");
        assert_eq!(defaults.overload_mode, "blocking", "blocking by default (§8)");
    }

    #[test]
    fn every_invalid_setting_is_reported_at_once() {
        let settings = TenantSettingsInput {
            name: "  ".to_owned(),
            default_locale: "de".to_owned(),
            time_zone: "Europe//Paris".to_owned(),
            default_capacity_minutes: vec![420, -1, 1441, 0, 0, 0, 0],
            overload_tolerance_percent: 101,
            overload_mode: "lenient".to_owned(),
            storage_quota_bytes: Some(0),
            api_rate_limit_per_minute: Some(1_000_001),
        };
        assert_eq!(
            errors(validate_settings(&settings)),
            [
                ("name".to_owned(), "required"),
                ("defaultLocale".to_owned(), "unsupported_locale"),
                ("timeZone".to_owned(), "unknown_time_zone"),
                ("defaultCapacityMinutes.1".to_owned(), "out_of_range"),
                ("defaultCapacityMinutes.2".to_owned(), "out_of_range"),
                ("overloadTolerancePercent".to_owned(), "out_of_range"),
                ("overloadMode".to_owned(), "format"),
                ("storageQuotaBytes".to_owned(), "out_of_range"),
                ("apiRateLimitPerMinute".to_owned(), "out_of_range"),
            ]
        );
    }

    #[test]
    fn the_capacity_has_one_value_per_weekday_up_to_a_full_day() {
        let mut settings = TenantSettingsInput::defaults("Acme");
        settings.default_capacity_minutes = vec![420; 5];
        assert_eq!(
            errors(validate_settings(&settings)),
            [("defaultCapacityMinutes".to_owned(), "length")]
        );
        // Part time and a full day are fine.
        settings.default_capacity_minutes = vec![1440, 210, 0, 0, 0, 0, 0];
        assert_eq!(validate_settings(&settings), Ok(()));
    }

    #[test]
    fn tolerance_and_quotas_have_bounds() {
        let mut settings = TenantSettingsInput::defaults("Acme");
        for tolerance in [0, 15, 100] {
            settings.overload_tolerance_percent = tolerance;
            assert_eq!(validate_settings(&settings), Ok(()), "{tolerance}");
        }
        settings.overload_tolerance_percent = -1;
        assert_eq!(
            errors(validate_settings(&settings)),
            [("overloadTolerancePercent".to_owned(), "out_of_range")]
        );
        settings.overload_tolerance_percent = 0;
        settings.storage_quota_bytes = Some(10 * 1024 * 1024 * 1024);
        settings.api_rate_limit_per_minute = Some(600);
        settings.overload_mode = "warning".to_owned();
        assert_eq!(validate_settings(&settings), Ok(()));
    }

    #[test]
    fn time_zones_need_the_iana_shape() {
        for good in [
            "UTC",
            "Europe/Paris",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+5",
            "America/Port-au-Prince",
        ] {
            assert!(is_plausible_time_zone(good), "{good}");
        }
        for bad in [
            "",
            "Europe/",
            "/Paris",
            "Europe Paris",
            "Europe/Paris;DROP",
            &"A".repeat(65),
        ] {
            assert!(!is_plausible_time_zone(bad), "{bad}");
        }
    }

    #[test]
    fn a_new_tenant_needs_a_host_name_label() {
        let settings = TenantSettingsInput::defaults("Acme");
        assert_eq!(validate_new_tenant("acme", &settings, None), Ok(()));
        assert_eq!(
            errors(validate_new_tenant("", &settings, None)),
            [("slug".to_owned(), "required")]
        );
        assert_eq!(
            errors(validate_new_tenant("Acme Corp", &settings, None)),
            [("slug".to_owned(), "format")]
        );
    }

    #[test]
    fn single_tenant_mode_serves_its_default_slug_only() {
        let settings = TenantSettingsInput::defaults("Acme");
        assert_eq!(validate_new_tenant("default", &settings, Some("default")), Ok(()));
        assert_eq!(
            errors(validate_new_tenant("acme", &settings, Some("default"))),
            [("slug".to_owned(), "not_default_slug")]
        );
    }

    #[test]
    fn overload_modes_round_trip() {
        for mode in [OverloadMode::Blocking, OverloadMode::Warning] {
            assert_eq!(OverloadMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(OverloadMode::parse("Blocking"), None);
    }
}
