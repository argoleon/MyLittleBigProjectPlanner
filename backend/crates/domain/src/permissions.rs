//! Permissions and node roles (specification §14, L02-US07).
//!
//! The catalog of permissions is fixed by the product; each tenant defines
//! its roles as sets of them. Roles are granted to members or groups on
//! hierarchy nodes and inherited downwards (L03): a member's effective
//! permissions on a node are the union of the roles that reach them there.

use std::collections::BTreeMap;

use crate::i18n;
use crate::validation::{FieldError, Violations};

/// A permission of the catalog. Identifiers are stable: stored in roles and
/// sent by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Permission {
    View,
    CreateTicket,
    Edit,
    Comment,
    LogTime,
    AssignResources,
    ForceOverload,
    ManageSprints,
    ManageMembers,
    AdministerNode,
}

impl Permission {
    /// The whole catalog, in display order.
    pub const ALL: [Self; 10] = [
        Self::View,
        Self::CreateTicket,
        Self::Edit,
        Self::Comment,
        Self::LogTime,
        Self::AssignResources,
        Self::ForceOverload,
        Self::ManageSprints,
        Self::ManageMembers,
        Self::AdministerNode,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::CreateTicket => "create_ticket",
            Self::Edit => "edit",
            Self::Comment => "comment",
            Self::LogTime => "log_time",
            Self::AssignResources => "assign_resources",
            Self::ForceOverload => "force_overload",
            Self::ManageSprints => "manage_sprints",
            Self::ManageMembers => "manage_members",
            Self::AdministerNode => "administer_node",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }

    const fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// A set of permissions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PermissionSet(u32);

impl PermissionSet {
    pub const EMPTY: Self = Self(0);

    /// Every permission of the catalog.
    pub fn all() -> Self {
        Permission::ALL.into_iter().collect()
    }

    pub const fn contains(self, permission: Permission) -> bool {
        self.0 & permission.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn with(self, permission: Permission) -> Self {
        Self(self.0 | permission.bit())
    }

    /// Whether every permission of `other` is in this set.
    pub const fn contains_all(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Permissions in either set.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The permissions, in catalog order.
    pub fn iter(self) -> impl Iterator<Item = Permission> {
        Permission::ALL.into_iter().filter(move |p| self.contains(*p))
    }

    /// Compact form, for caches.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The set of `bits`, unknown bits dropped.
    pub fn from_bits(bits: u32) -> Self {
        Self(bits & Self::all().0)
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        iter.into_iter().fold(Self::EMPTY, Self::with)
    }
}

/// Effective permissions of a member from the roles that reach them (direct
/// grants, groups, inheritance: `crate::grants` decides which ones): their
/// union. A grant only adds permissions.
pub fn effective(roles: impl IntoIterator<Item = PermissionSet>) -> PermissionSet {
    roles.into_iter().fold(PermissionSet::EMPTY, PermissionSet::union)
}

/// A committed change in a tenant, as far as permissions go. Effective
/// permissions are cached (specification §14): every change that can alter
/// them forgets the tenant's cache ([`PermissionChange::alters_permissions`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChange {
    /// A role was granted to a member or a group on a node.
    GrantAdded,
    GrantRemoved,
    /// A role's permissions changed (or only its labels: the caller does not
    /// tell them apart).
    RoleChanged,
    RoleDeleted,
    /// A member joined or left a group, which may hold grants.
    GroupMemberAdded,
    GroupMemberRemoved,
    /// A group went, and its grants with it.
    GroupDeleted,
    /// A member's tenant role or account type changed.
    MemberAccessChanged,
    /// A member left the tenant or was erased: their grants went with them,
    /// and their cached permissions must not outlive them.
    MemberRemoved,
    /// A node got another parent, or lost one: what it inherits changed.
    ParentLinked,
    ParentUnlinked,
    /// A node became confidential, or stopped being so: what it inherits
    /// changed, and so did what its descendants inherit through it.
    ConfidentialityChanged,
    /// Only the weighting of an existing link changed: aggregates, not
    /// access. (A new node needs nothing either: nothing is cached for it.)
    LinkWeightChanged,
}

impl PermissionChange {
    pub const ALL: [Self; 13] = [
        Self::GrantAdded,
        Self::GrantRemoved,
        Self::RoleChanged,
        Self::RoleDeleted,
        Self::GroupMemberAdded,
        Self::GroupMemberRemoved,
        Self::GroupDeleted,
        Self::MemberAccessChanged,
        Self::MemberRemoved,
        Self::ParentLinked,
        Self::ParentUnlinked,
        Self::ConfidentialityChanged,
        Self::LinkWeightChanged,
    ];

    /// Whether the cached permissions of the tenant must be forgotten.
    pub const fn alters_permissions(self) -> bool {
        !matches!(self, Self::LinkWeightChanged)
    }
}

/// Longest role label, in characters.
pub const ROLE_LABEL_MAX_CHARS: usize = 100;

/// What a tenant admin sets on a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleInput {
    /// One label per supported language, at least one.
    pub label_translations: BTreeMap<String, String>,
    pub permissions: PermissionSet,
}

/// Checks `input`; field names are those of the JSON API.
pub fn validate_role(input: &RoleInput) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let has_label = input
        .label_translations
        .iter()
        .any(|(locale, label)| i18n::is_supported(locale) && !label.trim().is_empty());
    if !has_label {
        v.add("labelTranslations", "required");
    }
    for (locale, label) in &input.label_translations {
        if !i18n::is_supported(locale) {
            v.add(format!("labelTranslations.{locale}"), "unsupported_locale");
        } else if label.trim().chars().count() > ROLE_LABEL_MAX_CHARS {
            v.add(format!("labelTranslations.{locale}"), "too_long");
        }
    }
    if input.permissions.is_empty() {
        v.add("permissions", "required");
    }
    v.into_result()
}

/// A role every new tenant starts with; the tenant may change or delete it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultRole {
    pub label_fr: &'static str,
    pub label_en: &'static str,
    pub permissions: &'static [Permission],
}

/// The default roles, from the narrowest to the widest.
pub const DEFAULT_ROLES: [DefaultRole; 4] = [
    DefaultRole {
        label_fr: "Lecteur",
        label_en: "Reader",
        permissions: &[Permission::View],
    },
    DefaultRole {
        label_fr: "Contributeur",
        label_en: "Contributor",
        permissions: &[
            Permission::View,
            Permission::CreateTicket,
            Permission::Edit,
            Permission::Comment,
            Permission::LogTime,
        ],
    },
    DefaultRole {
        label_fr: "Chef de projet",
        label_en: "Project manager",
        permissions: &[
            Permission::View,
            Permission::CreateTicket,
            Permission::Edit,
            Permission::Comment,
            Permission::LogTime,
            Permission::AssignResources,
            Permission::ForceOverload,
            Permission::ManageSprints,
        ],
    },
    DefaultRole {
        label_fr: "Administrateur de nœud",
        label_en: "Node admin",
        permissions: &Permission::ALL,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_change_of_access_forgets_the_cache_a_weighting_does_not() {
        use PermissionChange::{
            ConfidentialityChanged, GrantAdded, GrantRemoved, GroupDeleted, GroupMemberAdded,
            GroupMemberRemoved, LinkWeightChanged, MemberAccessChanged, MemberRemoved, ParentLinked,
            ParentUnlinked, RoleChanged, RoleDeleted,
        };
        let expected = [
            (GrantAdded, true),
            (GrantRemoved, true),
            (RoleChanged, true),
            (RoleDeleted, true),
            (GroupMemberAdded, true),
            (GroupMemberRemoved, true),
            (GroupDeleted, true),
            (MemberAccessChanged, true),
            (MemberRemoved, true),
            (ParentLinked, true),
            (ParentUnlinked, true),
            (ConfidentialityChanged, true),
            (LinkWeightChanged, false),
        ];
        assert_eq!(expected.map(|(change, _)| change), PermissionChange::ALL);
        for (change, alters) in expected {
            assert_eq!(change.alters_permissions(), alters, "{change:?}");
        }
    }

    #[test]
    fn a_set_contains_its_subsets() {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        let contributor = reader.with(Permission::Edit);
        assert!(contributor.contains_all(reader) && contributor.contains_all(contributor));
        assert!(!reader.contains_all(contributor));
        assert!(reader.contains_all(PermissionSet::EMPTY) && PermissionSet::all().contains_all(contributor));
    }

    #[test]
    fn identifiers_are_stable_and_parsed_back() {
        let ids: Vec<&str> = Permission::ALL.iter().map(|p| p.as_str()).collect();
        assert_eq!(
            ids,
            [
                "view",
                "create_ticket",
                "edit",
                "comment",
                "log_time",
                "assign_resources",
                "force_overload",
                "manage_sprints",
                "manage_members",
                "administer_node"
            ]
        );
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.as_str()), Some(p));
        }
        assert_eq!(Permission::parse("delete_everything"), None);
    }

    #[test]
    fn sets_contain_and_unite() {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        let commenter: PermissionSet = [Permission::View, Permission::Comment].into_iter().collect();
        assert!(reader.contains(Permission::View));
        assert!(!reader.contains(Permission::Comment));
        let both = reader.union(commenter);
        assert_eq!(both, commenter);
        assert_eq!(
            both.iter().collect::<Vec<_>>(),
            [Permission::View, Permission::Comment]
        );
        assert!(PermissionSet::EMPTY.is_empty());
        assert_eq!(PermissionSet::all().iter().count(), Permission::ALL.len());
    }

    #[test]
    fn bits_round_trip_and_drop_unknown_ones() {
        let set: PermissionSet = [Permission::Edit, Permission::AdministerNode]
            .into_iter()
            .collect();
        assert_eq!(PermissionSet::from_bits(set.bits()), set);
        assert_eq!(PermissionSet::from_bits(u32::MAX), PermissionSet::all());
    }

    #[test]
    fn effective_permissions_are_the_union_of_the_roles() {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        let sprints: PermissionSet = [Permission::ManageSprints].into_iter().collect();
        assert_eq!(effective([]), PermissionSet::EMPTY);
        let both = effective([reader, sprints]);
        assert!(both.contains(Permission::View) && both.contains(Permission::ManageSprints));
        // Adding a role never removes a permission.
        assert_eq!(effective([reader, sprints, PermissionSet::EMPTY]), both);
    }

    fn role(labels: &[(&str, &str)], permissions: &[Permission]) -> RoleInput {
        RoleInput {
            label_translations: labels
                .iter()
                .map(|(l, v)| ((*l).to_owned(), (*v).to_owned()))
                .collect(),
            permissions: permissions.iter().copied().collect(),
        }
    }

    #[test]
    fn a_role_has_a_label_and_permissions() {
        assert_eq!(
            validate_role(&role(
                &[("fr", "Chef de projet"), ("en", "Project manager")],
                &[Permission::View]
            )),
            Ok(())
        );
        let codes = |input: RoleInput| -> Vec<(String, &'static str)> {
            validate_role(&input)
                .unwrap_err()
                .into_iter()
                .map(|e| (e.field, e.code))
                .collect()
        };
        assert_eq!(
            codes(role(&[("fr", " "), ("de", "Projektleiter")], &[])),
            [
                ("labelTranslations".to_owned(), "required"),
                ("labelTranslations.de".to_owned(), "unsupported_locale"),
                ("permissions".to_owned(), "required"),
            ]
        );
        assert_eq!(
            codes(role(
                &[("fr", "Chef"), ("en", &"x".repeat(101))],
                &[Permission::View]
            )),
            [("labelTranslations.en".to_owned(), "too_long")]
        );
    }

    #[test]
    fn default_roles_grow_from_reader_to_node_admin() {
        let sets: Vec<PermissionSet> = DEFAULT_ROLES
            .iter()
            .map(|r| r.permissions.iter().copied().collect())
            .collect();
        for pair in sets.windows(2) {
            assert_eq!(
                pair[0].union(pair[1]),
                pair[1],
                "each role includes the previous one"
            );
        }
        assert_eq!(sets[3], PermissionSet::all());
        // The project manager administers neither members nor the node.
        assert!(!sets[2].contains(Permission::ManageMembers));
        assert!(!sets[2].contains(Permission::AdministerNode));
        for r in DEFAULT_ROLES {
            assert!(validate_role(&role(&[("fr", r.label_fr), ("en", r.label_en)], r.permissions)).is_ok());
        }
    }
}
