//! Saved filters (specification §6, L06-US04): a query kept under a name and
//! an icon, personal or shared with groups or with the whole tenant.
//!
//! - The owner alone edits a filter; others it is shared with run it.
//! - A tenant admin may delete a shared filter (moderation, or a departed
//!   owner's); a private one is its owner's business only.
//! - The query is checked by the language (`query::parse`) before it is kept.

use crate::validation::{FieldError, Violations};

/// Longest name, in characters.
pub const NAME_MAX_CHARS: usize = 100;

/// Most groups a filter is shared with.
pub const MAX_GROUPS: usize = 50;

/// The icons a filter may show (Material Symbols names), the first by default.
pub const ICONS: [&str; 10] = [
    "filter_alt",
    "person",
    "bug_report",
    "schedule",
    "shield",
    "flag",
    "star",
    "support_agent",
    "bolt",
    "label",
];

/// What a member writes of a filter, besides its query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterFields {
    pub name: String,
    pub icon: String,
    /// Groups it is shared with (ids, as written).
    pub groups: usize,
}

/// Checks `fields`; the name is compared trimmed.
pub fn validate(fields: &FilterFields) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let name = fields.name.trim().chars().count();
    if name == 0 {
        v.add("name", "required");
    } else if name > NAME_MAX_CHARS {
        v.add("name", "too_long");
    }
    if !ICONS.contains(&fields.icon.as_str()) {
        v.add("icon", "not_allowed");
    }
    if fields.groups > MAX_GROUPS {
        v.add("sharing.groupIds", "too_many");
    }
    v.into_result()
}

/// Whether the reader may change a filter: its owner only.
pub const fn can_edit(is_owner: bool) -> bool {
    is_owner
}

/// Whether the reader may delete a filter: its owner, or a tenant admin when
/// it is shared.
pub const fn can_delete(is_owner: bool, is_tenant_admin: bool, shared: bool) -> bool {
    is_owner || (is_tenant_admin && shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(name: &str, icon: &str, groups: usize) -> FilterFields {
        FilterFields {
            name: name.to_owned(),
            icon: icon.to_owned(),
            groups,
        }
    }

    fn codes(fields: &FilterFields) -> Vec<(String, &'static str)> {
        validate(fields)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn a_name_an_icon_of_the_list_and_a_few_groups_pass() {
        assert!(validate(&fields("My open tickets", "person", 2)).is_ok());
        assert!(validate(&fields(&"x".repeat(NAME_MAX_CHARS), ICONS[0], MAX_GROUPS)).is_ok());
    }

    #[test]
    fn a_missing_or_long_name_an_unknown_icon_or_too_many_groups_are_refused() {
        assert_eq!(
            codes(&fields("  ", "person", 0)),
            [("name".to_owned(), "required")]
        );
        assert_eq!(
            codes(&fields(&"x".repeat(NAME_MAX_CHARS + 1), "person", 0)),
            [("name".to_owned(), "too_long")]
        );
        assert_eq!(
            codes(&fields("Mine", "rocket", 0)),
            [("icon".to_owned(), "not_allowed")]
        );
        assert_eq!(
            codes(&fields("Mine", "person", MAX_GROUPS + 1)),
            [("sharing.groupIds".to_owned(), "too_many")]
        );
    }

    #[test]
    fn the_owner_edits_an_admin_deletes_only_what_is_shared() {
        assert!(can_edit(true));
        assert!(!can_edit(false));
        assert!(can_delete(true, false, false));
        assert!(can_delete(false, true, true));
        assert!(
            !can_delete(false, true, false),
            "a private filter is its owner's business"
        );
        assert!(!can_delete(false, false, true));
    }
}
