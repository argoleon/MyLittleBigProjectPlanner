//! Languages of the product and translatable labels (specification §5, §15).
//!
//! Labels are stored with one value per language (`{"fr": …, "en": …}`).
//! The API returns them in the language of the `Accept-Language` header,
//! falling back to the tenant's default language, then to any translation.

use std::collections::BTreeMap;

/// Languages supported by the MVP.
pub const SUPPORTED_LOCALES: [&str; 2] = ["fr", "en"];

/// Whether `locale` is one of [`SUPPORTED_LOCALES`].
pub fn is_supported(locale: &str) -> bool {
    SUPPORTED_LOCALES.contains(&locale)
}

/// Best supported language for an `Accept-Language` header (RFC 9110 §12.5.4),
/// or `default` when the header is absent or names no supported language
/// (the first supported language if `default` is not supported either).
///
/// Region subtags match their primary language (`fr-CA` → `fr`); `q=0`
/// excludes a language; on equal weights the header order wins.
pub fn negotiate(accept_language: Option<&str>, default: &str) -> &'static str {
    let default = SUPPORTED_LOCALES
        .into_iter()
        .find(|l| *l == default)
        .unwrap_or(SUPPORTED_LOCALES[0]);
    let Some(header) = accept_language else {
        return default;
    };
    let mut best: Option<(&'static str, u16)> = None;
    for item in header.split(',') {
        let mut parts = item.split(';');
        let tag = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let weight = parts
            .find_map(|p| p.trim().strip_prefix("q="))
            .map_or(1000, parse_weight);
        if weight == 0 {
            continue;
        }
        let primary = tag.split('-').next().unwrap_or_default();
        let Some(locale) = SUPPORTED_LOCALES.iter().find(|l| **l == primary) else {
            continue;
        };
        if best.is_none_or(|(_, w)| weight > w) {
            best = Some((locale, weight));
        }
    }
    best.map_or(default, |(locale, _)| locale)
}

/// Quality value in thousandths (`0.8` → 800); invalid values count as 0.
fn parse_weight(raw: &str) -> u16 {
    let raw = raw.trim();
    let (int, frac) = raw.split_once('.').unwrap_or((raw, ""));
    let digits: String = frac.chars().take(3).collect();
    match (int, format!("{digits:0<3}").parse::<u16>()) {
        ("1", Ok(_)) => 1000,
        ("0", Ok(thousandths)) => thousandths,
        _ => 0,
    }
}

/// The label in `locale`, else in `fallback`, else in any language.
pub fn resolve<'a>(
    translations: &'a BTreeMap<String, String>,
    locale: &str,
    fallback: &str,
) -> Option<&'a str> {
    let non_empty = |l: &str| translations.get(l).map(String::as_str).filter(|s| !s.is_empty());
    non_empty(locale)
        .or_else(|| non_empty(fallback))
        .or_else(|| translations.values().map(String::as_str).find(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_or_unsupported_header_falls_back_to_default() {
        assert_eq!(negotiate(None, "fr"), "fr");
        assert_eq!(
            negotiate(None, "de"),
            "fr",
            "unsupported default: first supported"
        );
        assert_eq!(negotiate(Some("de-DE, es"), "en"), "en");
        assert_eq!(negotiate(Some(""), "fr"), "fr");
    }

    #[test]
    fn region_subtags_match_their_language() {
        assert_eq!(negotiate(Some("en-GB"), "fr"), "en");
        assert_eq!(negotiate(Some("FR-ca"), "en"), "fr");
    }

    #[test]
    fn highest_weight_wins_and_ties_keep_header_order() {
        assert_eq!(negotiate(Some("fr;q=0.5, en;q=0.9"), "fr"), "en");
        assert_eq!(negotiate(Some("de, en, fr"), "fr"), "en");
        assert_eq!(negotiate(Some("en;q=0.8, fr;q=0.8"), "fr"), "en");
    }

    #[test]
    fn zero_weight_excludes_a_language() {
        assert_eq!(negotiate(Some("en;q=0, fr;q=0.1"), "en"), "fr");
        assert_eq!(negotiate(Some("en;q=0"), "fr"), "fr");
    }

    #[test]
    fn weights_parse_like_rfc_9110() {
        assert_eq!(parse_weight("1"), 1000);
        assert_eq!(parse_weight("1.000"), 1000);
        assert_eq!(parse_weight("0.8"), 800);
        assert_eq!(parse_weight("0.125"), 125);
        assert_eq!(parse_weight("0"), 0);
        assert_eq!(parse_weight("2"), 0);
        assert_eq!(parse_weight("abc"), 0);
    }

    #[test]
    fn resolve_prefers_locale_then_fallback_then_any() {
        let both: BTreeMap<String, String> = [
            ("fr".to_owned(), "Bonjour".to_owned()),
            ("en".to_owned(), "Hello".to_owned()),
        ]
        .into();
        assert_eq!(resolve(&both, "en", "fr"), Some("Hello"));

        let fr_only: BTreeMap<String, String> = [("fr".to_owned(), "Bonjour".to_owned())].into();
        assert_eq!(resolve(&fr_only, "en", "fr"), Some("Bonjour"));

        let other: BTreeMap<String, String> = [
            ("en".to_owned(), String::new()),
            ("fr".to_owned(), "Salut".to_owned()),
        ]
        .into();
        assert_eq!(
            resolve(&other, "en", "en"),
            Some("Salut"),
            "empty values are skipped"
        );

        assert_eq!(resolve(&BTreeMap::new(), "en", "fr"), None);
    }
}
