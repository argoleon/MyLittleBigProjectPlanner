//! Selection of returned fields (`?fields=id,code,label`, specification §15).
//!
//! Without `fields`, a resource returns its default fields. With it, only the
//! listed ones; an unknown name is a 400 so typos do not go unnoticed.

use domain::validation::FieldError;
use serde_json::Value;

use crate::problem::{ErrorCode, Problem};

/// The fields to return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSelection(Vec<String>);

impl FieldSelection {
    /// Parses `?fields=` against the resource's `available` fields;
    /// `None` or empty selects `defaults`.
    pub fn parse(raw: Option<&str>, available: &[&str], defaults: &[&str]) -> Result<Self, Problem> {
        let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
            return Ok(Self(defaults.iter().map(|f| (*f).to_owned()).collect()));
        };
        let mut selected = Vec::new();
        let mut unknown = Vec::new();
        for name in raw.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            if !available.contains(&name) {
                unknown.push(FieldError::new(format!("fields.{name}"), "unknown_field"));
            } else if !selected.iter().any(|s| s == name) {
                selected.push(name.to_owned());
            }
        }
        if unknown.is_empty() {
            Ok(Self(selected))
        } else {
            Err(Problem::new(ErrorCode::InvalidQuery).with_errors(unknown))
        }
    }

    /// Keeps only the selected keys of a serialized object.
    pub fn apply(&self, mut value: Value) -> Value {
        if let Value::Object(map) = &mut value {
            map.retain(|key, _| self.0.iter().any(|f| f == key));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AVAILABLE: &[&str] = &["id", "code", "label", "labelTranslations"];
    const DEFAULTS: &[&str] = &["id", "code", "label"];

    #[test]
    fn absent_or_empty_selects_defaults() {
        let expected = FieldSelection(vec!["id".into(), "code".into(), "label".into()]);
        assert_eq!(
            FieldSelection::parse(None, AVAILABLE, DEFAULTS).unwrap(),
            expected
        );
        assert_eq!(
            FieldSelection::parse(Some(" "), AVAILABLE, DEFAULTS).unwrap(),
            expected
        );
    }

    #[test]
    fn selected_fields_are_kept_once() {
        let selection = FieldSelection::parse(Some("id, labelTranslations,id"), AVAILABLE, DEFAULTS).unwrap();
        let value = json!({"id": 1, "code": "A", "label": "x", "labelTranslations": {"fr": "x"}});
        assert_eq!(
            selection.apply(value),
            json!({"id": 1, "labelTranslations": {"fr": "x"}})
        );
    }

    #[test]
    fn unknown_fields_are_reported() {
        let err = FieldSelection::parse(Some("id,secret,cod"), AVAILABLE, DEFAULTS).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidQuery);
        let fields: Vec<_> = err.errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["fields.secret", "fields.cod"]);
    }
}
