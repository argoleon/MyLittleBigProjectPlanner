//! Business rules: capacity, workload, recurrences, workflows, query language.
//!
//! This crate performs no I/O and depends on no database, HTTP or runtime
//! crate, so every rule can be unit-tested in isolation. `just check-domain`
//! enforces it.

/// Name of the product, shared by every service.
pub const PRODUCT_NAME: &str = "My Little Big Project Planner";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_name_is_set() {
        assert!(!PRODUCT_NAME.is_empty());
    }
}
