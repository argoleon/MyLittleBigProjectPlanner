//! Teams (specification §8, L07-US02): resources grouped for views and
//! filters. A resource may belong to several teams; assignments always
//! target a resource, never a team. Tenant admins and resource managers
//! manage teams (`crate::resources::manages_resources`); every internal
//! member reads them.

use std::collections::BTreeMap;

use crate::i18n::validate_translations;
use crate::validation::{FieldError, Violations};

/// Longest name, per language, in characters.
pub const NAME_MAX_CHARS: usize = 100;

/// Longest description, in characters.
pub const DESCRIPTION_MAX_CHARS: usize = 1000;

/// What a manager writes of a team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamInput {
    /// One name per language, at least one.
    pub name_translations: BTreeMap<String, String>,
    pub description: Option<String>,
}

impl TeamInput {
    /// Names trimmed, empty ones dropped.
    pub fn names(&self) -> BTreeMap<String, String> {
        self.name_translations
            .iter()
            .map(|(locale, name)| (locale.clone(), name.trim().to_owned()))
            .filter(|(_, name)| !name.is_empty())
            .collect()
    }

    /// The description trimmed, `None` when empty.
    pub fn description(&self) -> Option<&str> {
        self.description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }
}

/// Checks `input`.
pub fn validate(input: &TeamInput) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if let Err(errors) = validate_translations("nameTranslations", &input.name_translations, NAME_MAX_CHARS) {
        for e in errors {
            v.add(e.field, e.code);
        }
    }
    if input
        .description()
        .is_some_and(|d| d.chars().count() > DESCRIPTION_MAX_CHARS)
    {
        v.add("description", "too_long");
    }
    v.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(names: &[(&str, &str)], description: Option<&str>) -> TeamInput {
        TeamInput {
            name_translations: names
                .iter()
                .map(|(l, n)| ((*l).to_owned(), (*n).to_owned()))
                .collect(),
            description: description.map(str::to_owned),
        }
    }

    fn codes(input: &TeamInput) -> Vec<(String, &'static str)> {
        validate(input)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn a_name_in_one_language_is_enough() {
        assert!(validate(&input(&[("fr", "Plateforme")], None)).is_ok());
        assert!(
            validate(&input(
                &[("fr", "Plateforme"), ("en", "Platform")],
                Some("Back end")
            ))
            .is_ok()
        );
    }

    #[test]
    fn a_missing_long_or_foreign_name_and_a_long_description_are_refused() {
        assert_eq!(
            codes(&input(&[("fr", "  ")], None)),
            [("nameTranslations".to_owned(), "required")]
        );
        assert_eq!(
            codes(&input(&[("fr", &"x".repeat(NAME_MAX_CHARS + 1))], None)),
            [("nameTranslations.fr".to_owned(), "too_long")]
        );
        assert_eq!(
            codes(&input(&[("fr", "Plateforme"), ("de", "Plattform")], None)),
            [("nameTranslations.de".to_owned(), "unsupported_locale")]
        );
        assert_eq!(
            codes(&input(
                &[("fr", "Plateforme")],
                Some(&"d".repeat(DESCRIPTION_MAX_CHARS + 1))
            )),
            [("description".to_owned(), "too_long")]
        );
    }

    #[test]
    fn names_and_description_are_trimmed() {
        let team = input(&[("fr", " Plateforme "), ("en", "  ")], Some("   "));
        assert_eq!(
            team.names(),
            BTreeMap::from([("fr".to_owned(), "Plateforme".to_owned())])
        );
        assert_eq!(team.description(), None);
    }
}
