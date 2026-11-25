//! Resources (specification §8, L07-US01): what work is planned on, apart
//! from accounts.
//!
//! - A **person** may be linked to an internal member of the tenant, one
//!   resource per member; it then shows the member's name. Without an
//!   account (a subcontractor who never signs in), it has its own name.
//! - A **generic** resource is an unnamed role ("Rust developer"), never
//!   linked to an account.
//! - Tenant admins and resource managers manage resources; every internal
//!   member reads them.

use crate::account::{AccountType, TenantRole};
use crate::validation::{FieldError, Violations};

/// Longest name, in characters.
pub const NAME_MAX_CHARS: usize = 200;

/// Longest job title, in characters.
pub const JOB_TITLE_MAX_CHARS: usize = 100;

/// Kind of a resource. Equipment and rooms come after the MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Person,
    Generic,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Generic => "generic",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "person" => Some(Self::Person),
            "generic" => Some(Self::Generic),
            _ => None,
        }
    }
}

/// Whether a member with `role` creates, changes and deactivates resources.
pub const fn manages_resources(role: TenantRole) -> bool {
    matches!(role, TenantRole::TenantAdmin | TenantRole::ResourceManager)
}

/// Whether a member of `account_type` may be linked to a person resource:
/// internal members only, external accounts only reach the portal.
pub const fn can_be_linked(account_type: AccountType) -> bool {
    matches!(account_type, AccountType::Internal)
}

/// What a manager writes of a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceFields<'a> {
    pub kind: Kind,
    /// Required unless linked to an account, whose name it shows.
    pub display_name: Option<&'a str>,
    pub job_title: Option<&'a str>,
    /// Linked to an account.
    pub linked: bool,
}

/// Checks `fields`; texts are compared trimmed, an empty one is absent.
pub fn validate(fields: &ResourceFields<'_>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let name = trimmed(fields.display_name).map_or(0, |n| n.chars().count());
    if fields.linked {
        if fields.kind != Kind::Person {
            v.add("accountId", "person_only");
        }
    } else if name == 0 {
        v.add("displayName", "required");
    }
    if name > NAME_MAX_CHARS {
        v.add("displayName", "too_long");
    }
    if trimmed(fields.job_title).is_some_and(|t| t.chars().count() > JOB_TITLE_MAX_CHARS) {
        v.add("jobTitle", "too_long");
    }
    v.into_result()
}

/// `value` trimmed, `None` when empty.
pub fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>(
        kind: Kind,
        name: Option<&'a str>,
        title: Option<&'a str>,
        linked: bool,
    ) -> ResourceFields<'a> {
        ResourceFields {
            kind,
            display_name: name,
            job_title: title,
            linked,
        }
    }

    fn codes(fields: &ResourceFields<'_>) -> Vec<(String, &'static str)> {
        validate(fields)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [Kind::Person, Kind::Generic] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("room"), None);
    }

    #[test]
    fn tenant_admins_and_resource_managers_manage_resources() {
        assert!(manages_resources(TenantRole::TenantAdmin));
        assert!(manages_resources(TenantRole::ResourceManager));
        assert!(!manages_resources(TenantRole::ConfigurationAdmin));
        assert!(!manages_resources(TenantRole::Member));
    }

    #[test]
    fn only_internal_members_are_linked() {
        assert!(can_be_linked(AccountType::Internal));
        assert!(!can_be_linked(AccountType::External));
    }

    #[test]
    fn a_person_without_account_and_a_generic_resource_need_a_name() {
        assert!(
            validate(&fields(
                Kind::Person,
                Some("Camille Martin"),
                Some("Dev back"),
                false
            ))
            .is_ok()
        );
        assert!(validate(&fields(Kind::Generic, Some("Rust developer"), None, false)).is_ok());
        for kind in [Kind::Person, Kind::Generic] {
            assert_eq!(
                codes(&fields(kind, Some("   "), None, false)),
                [("displayName".to_owned(), "required")]
            );
            assert_eq!(
                codes(&fields(kind, None, None, false)),
                [("displayName".to_owned(), "required")]
            );
        }
    }

    #[test]
    fn a_linked_person_shows_its_account_name() {
        assert!(validate(&fields(Kind::Person, None, None, true)).is_ok());
        assert!(validate(&fields(Kind::Person, Some("ignored"), None, true)).is_ok());
    }

    #[test]
    fn a_generic_resource_is_never_linked() {
        assert_eq!(
            codes(&fields(Kind::Generic, Some("Rust developer"), None, true)),
            [("accountId".to_owned(), "person_only")]
        );
    }

    #[test]
    fn long_names_and_titles_are_refused() {
        let name = "x".repeat(NAME_MAX_CHARS);
        let title = "y".repeat(JOB_TITLE_MAX_CHARS);
        assert!(validate(&fields(Kind::Person, Some(&name), Some(&title), false)).is_ok());
        let name = "x".repeat(NAME_MAX_CHARS + 1);
        let title = "y".repeat(JOB_TITLE_MAX_CHARS + 1);
        assert_eq!(
            codes(&fields(Kind::Person, Some(&name), Some(&title), false)),
            [
                ("displayName".to_owned(), "too_long"),
                ("jobTitle".to_owned(), "too_long")
            ]
        );
    }

    #[test]
    fn texts_are_trimmed_and_empty_ones_are_absent() {
        assert_eq!(trimmed(Some("  Dev back ")), Some("Dev back"));
        assert_eq!(trimmed(Some("   ")), None);
        assert_eq!(trimmed(None), None);
    }
}
