//! Rules of the reference resource, the template for new endpoints.
//! A real feature keeps the same shape: an input struct and a `validate`
//! function returning every field error at once.

use std::collections::BTreeMap;

use crate::i18n;
use crate::validation::{FieldError, Violations};

pub const CODE_MAX_LEN: usize = 20;
pub const LABEL_MAX_LEN: usize = 200;
pub const QUANTITY_MAX: i32 = 1_000_000;

/// What a client sends to create or replace a reference item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceItemInput {
    pub code: String,
    pub label_translations: BTreeMap<String, String>,
    pub quantity: i32,
}

/// Checks `input`; field names are those of the JSON API.
///
/// - `code`: 2 to 20 characters among `A-Z`, `0-9`, `-`, starting with a letter;
/// - `labelTranslations`: only supported languages, at least one non-empty,
///   each at most 200 characters;
/// - `quantity`: 0 to 1,000,000.
pub fn validate(input: &ReferenceItemInput) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();

    let code = input.code.as_str();
    if code.is_empty() {
        v.add("code", "required");
    } else if code.chars().count() < 2 || code.chars().count() > CODE_MAX_LEN {
        v.add("code", "length");
    } else if !code.starts_with(|c: char| c.is_ascii_uppercase())
        || !code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
    {
        v.add("code", "format");
    }

    let has_label = input
        .label_translations
        .iter()
        .any(|(locale, label)| i18n::is_supported(locale) && !label.is_empty());
    if !has_label {
        v.add("labelTranslations", "required");
    }
    for (locale, label) in &input.label_translations {
        if !i18n::is_supported(locale) {
            v.add(format!("labelTranslations.{locale}"), "unsupported_locale");
        } else if label.chars().count() > LABEL_MAX_LEN {
            v.add(format!("labelTranslations.{locale}"), "too_long");
        }
    }

    if !(0..=QUANTITY_MAX).contains(&input.quantity) {
        v.add("quantity", "out_of_range");
    }

    v.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> ReferenceItemInput {
        ReferenceItemInput {
            code: "REF-1".to_owned(),
            label_translations: [("fr".to_owned(), "Référence".to_owned())].into(),
            quantity: 3,
        }
    }

    fn errors(input: &ReferenceItemInput) -> Vec<(String, &'static str)> {
        validate(input)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn valid_input_passes() {
        assert_eq!(validate(&valid()), Ok(()));
    }

    #[test]
    fn code_rules() {
        for (code, expected) in [
            ("", "required"),
            ("A", "length"),
            ("ABCDEFGHIJKLMNOPQRSTU", "length"),
            ("ref-1", "format"),
            ("1REF", "format"),
            ("RÉF", "format"),
        ] {
            let input = ReferenceItemInput {
                code: code.to_owned(),
                ..valid()
            };
            assert_eq!(errors(&input), vec![("code".to_owned(), expected)], "{code:?}");
        }
    }

    #[test]
    fn label_needs_one_supported_non_empty_translation() {
        let mut input = valid();
        input.label_translations = BTreeMap::new();
        assert_eq!(errors(&input), vec![("labelTranslations".to_owned(), "required")]);

        input.label_translations = [("de".to_owned(), "Referenz".to_owned())].into();
        assert_eq!(
            errors(&input),
            vec![
                ("labelTranslations".to_owned(), "required"),
                ("labelTranslations.de".to_owned(), "unsupported_locale")
            ],
            "an unsupported language does not count as a label"
        );

        input.label_translations = [("fr".to_owned(), String::new())].into();
        assert_eq!(errors(&input), vec![("labelTranslations".to_owned(), "required")]);

        input.label_translations = [("en".to_owned(), "x".repeat(LABEL_MAX_LEN + 1))].into();
        assert_eq!(
            errors(&input),
            vec![("labelTranslations.en".to_owned(), "too_long")]
        );
    }

    #[test]
    fn quantity_range() {
        for quantity in [-1, QUANTITY_MAX + 1] {
            let input = ReferenceItemInput { quantity, ..valid() };
            assert_eq!(errors(&input), vec![("quantity".to_owned(), "out_of_range")]);
        }
        for quantity in [0, QUANTITY_MAX] {
            assert_eq!(validate(&ReferenceItemInput { quantity, ..valid() }), Ok(()));
        }
    }

    #[test]
    fn all_errors_are_reported_together() {
        let input = ReferenceItemInput {
            code: String::new(),
            label_translations: BTreeMap::new(),
            quantity: -5,
        };
        assert_eq!(errors(&input).len(), 3);
    }
}
