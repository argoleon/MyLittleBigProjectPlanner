//! Field-level validation errors, shared by every business rule.
//!
//! A rule returns all the problems it finds at once, so the user can fix
//! every field in one go. The API turns them into the `errors` array of a
//! `validation_failed` Problem Details response.

/// One invalid field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// Field path as sent by the client, e.g. `code` or `labelTranslations.fr`.
    pub field: String,
    /// Stable code the client translates, e.g. `required`, `too_long`.
    pub code: &'static str,
}

impl FieldError {
    pub fn new(field: impl Into<String>, code: &'static str) -> Self {
        Self {
            field: field.into(),
            code,
        }
    }
}

/// Collects field errors while checking an input.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Violations(Vec<FieldError>);

impl Violations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, field: impl Into<String>, code: &'static str) {
        self.0.push(FieldError::new(field, code));
    }

    /// `Ok(())` when nothing was added, otherwise every error found.
    pub fn into_result(self) -> Result<(), Vec<FieldError>> {
        if self.0.is_empty() { Ok(()) } else { Err(self.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_violations_are_ok() {
        assert_eq!(Violations::new().into_result(), Ok(()));
    }

    #[test]
    fn every_violation_is_returned_in_order() {
        let mut v = Violations::new();
        v.add("code", "required");
        v.add("quantity", "out_of_range");
        assert_eq!(
            v.into_result(),
            Err(vec![
                FieldError::new("code", "required"),
                FieldError::new("quantity", "out_of_range")
            ])
        );
    }
}
