//! Rules of project configurations (specification §5, L04-US01).
//!
//! A configuration gathers what a project works with (issue types, fields,
//! workflows, screens, notifications) and is shared by any number of
//! projects. Its admins edit a draft, which they publish once they have seen
//! the projects it affects.

use std::collections::{BTreeMap, BTreeSet};

use crate::account::TenantRole;
use crate::i18n;
use crate::validation::{FieldError, Violations};

/// Longest name of a configuration, in characters, per language.
pub const CONFIG_NAME_MAX_CHARS: usize = 100;

/// Whether a member with `role` manages the project configurations: tenant
/// admins and configuration admins (§3).
pub const fn manages_configurations(role: TenantRole) -> bool {
    matches!(role, TenantRole::TenantAdmin | TenantRole::ConfigurationAdmin)
}

/// The name of a configuration: one per language (`fr`, `en`), at least one,
/// [`CONFIG_NAME_MAX_CHARS`] at most once trimmed.
pub fn validate_name(translations: &BTreeMap<String, String>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let named = translations
        .iter()
        .any(|(locale, name)| i18n::is_supported(locale) && !name.trim().is_empty());
    if !named {
        v.add("nameTranslations", "required");
    }
    for (locale, name) in translations {
        if !i18n::is_supported(locale) {
            v.add(format!("nameTranslations.{locale}"), "unsupported_locale");
        } else if name.trim().chars().count() > CONFIG_NAME_MAX_CHARS {
            v.add(format!("nameTranslations.{locale}"), "too_long");
        }
    }
    v.into_result()
}

/// What a configuration holds, edited in its draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemKind {
    IssueType,
    Field,
    Workflow,
    Screen,
    Notification,
}

impl ItemKind {
    pub const ALL: [Self; 5] = [
        Self::IssueType,
        Self::Field,
        Self::Workflow,
        Self::Screen,
        Self::Notification,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueType => "issue_type",
            Self::Field => "field",
            Self::Workflow => "workflow",
            Self::Screen => "screen",
            Self::Notification => "notification",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }
}

/// One item of a configuration, by its kind and its identifier, which stays
/// the same from one version to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Item {
    pub kind: ItemKind,
    pub id: u128,
}

/// What a draft changed: each item once, however many times it was edited
/// (the "2 changes" of the draft chip).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftChanges(BTreeSet<Item>);

impl DraftChanges {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an edit of `item`; false if the draft had already changed it.
    pub fn record(&mut self, item: Item) -> bool {
        self.0.insert(item)
    }

    /// The number of items changed.
    pub fn count(&self) -> usize {
        self.0.len()
    }
}

impl FromIterator<Item> for DraftChanges {
    fn from_iter<I: IntoIterator<Item = Item>>(items: I) -> Self {
        Self(items.into_iter().collect())
    }
}

/// What publishing a draft does to the projects of the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishImpact<P> {
    /// Every project that uses the configuration, archived ones included:
    /// they get the new version too.
    pub projects: Vec<P>,
    /// Whether tickets must move first (a type or a status they use is gone):
    /// the status and type mapping wizard (L04-US08).
    pub needs_migration: bool,
}

/// The impact of publishing a draft that removes `removed` items from the
/// published version, on `projects`. Only a removal calls for a migration,
/// and only when a project could have tickets on it.
pub fn publish_impact<P>(projects: Vec<P>, removed: &BTreeSet<Item>) -> PublishImpact<P> {
    let needs_migration = !projects.is_empty()
        && removed
            .iter()
            .any(|item| matches!(item.kind, ItemKind::IssueType | ItemKind::Workflow));
    PublishImpact {
        projects,
        needs_migration,
    }
}

/// Why a configuration cannot be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionRefusal {
    /// The default configuration of the tenant: new projects get it.
    IsDefault,
    /// Projects use it: they move to another one first.
    InUse,
}

/// Why the configuration is not deleted, if it is not.
pub const fn deletion_refusal(is_default: bool, project_count: u64) -> Option<DeletionRefusal> {
    if is_default {
        Some(DeletionRefusal::IsDefault)
    } else if project_count > 0 {
        Some(DeletionRefusal::InUse)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(l, n)| ((*l).to_owned(), (*n).to_owned()))
            .collect()
    }

    fn codes(result: Result<(), Vec<FieldError>>) -> Vec<(String, &'static str)> {
        result
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn tenant_and_configuration_admins_manage_configurations() {
        assert!(manages_configurations(TenantRole::TenantAdmin));
        assert!(manages_configurations(TenantRole::ConfigurationAdmin));
        assert!(!manages_configurations(TenantRole::Member));
    }

    #[test]
    fn a_name_in_one_supported_language_is_enough() {
        assert!(validate_name(&names(&[("fr", "Développement logiciel")])).is_ok());
        assert!(validate_name(&names(&[("fr", "Développement"), ("en", "Software")])).is_ok());
        assert_eq!(
            codes(validate_name(&names(&[]))),
            [("nameTranslations".to_owned(), "required")]
        );
        assert_eq!(
            codes(validate_name(&names(&[("fr", "  "), ("en", "")]))),
            [("nameTranslations".to_owned(), "required")]
        );
    }

    #[test]
    fn a_name_is_short_and_in_a_supported_language() {
        let long = "x".repeat(CONFIG_NAME_MAX_CHARS + 1);
        assert_eq!(
            codes(validate_name(&names(&[("fr", "Support"), ("en", &long)]))),
            [("nameTranslations.en".to_owned(), "too_long")]
        );
        let padded = format!("  {}  ", "x".repeat(CONFIG_NAME_MAX_CHARS));
        assert!(
            validate_name(&names(&[("fr", &padded)])).is_ok(),
            "counted once trimmed"
        );
        assert_eq!(
            codes(validate_name(&names(&[("fr", "Support"), ("de", "Hilfe")]))),
            [("nameTranslations.de".to_owned(), "unsupported_locale")]
        );
    }

    #[test]
    fn item_kinds_round_trip() {
        for kind in ItemKind::ALL {
            assert_eq!(ItemKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ItemKind::parse("permission"), None);
    }

    #[test]
    fn a_draft_counts_each_changed_item_once() {
        let mut changes = DraftChanges::new();
        assert_eq!(changes.count(), 0);
        let bug = Item {
            kind: ItemKind::IssueType,
            id: 1,
        };
        assert!(changes.record(bug));
        assert!(!changes.record(bug), "edited again: still one change");
        assert_eq!(changes.count(), 1);
        // Another item, or the same identifier of another kind.
        assert!(changes.record(Item {
            kind: ItemKind::Workflow,
            id: 2
        }));
        assert!(changes.record(Item {
            kind: ItemKind::Field,
            id: 1
        }));
        assert_eq!(changes.count(), 3);
        let again: DraftChanges = [bug, bug].into_iter().collect();
        assert_eq!(again.count(), 1);
    }

    #[test]
    fn publishing_lists_every_project_and_migrates_only_on_removals() {
        let projects = vec!["PLAT", "CYBP"];
        let none = publish_impact(projects.clone(), &BTreeSet::new());
        assert_eq!(none.projects, ["PLAT", "CYBP"]);
        assert!(!none.needs_migration);

        let gone = |kind| BTreeSet::from([Item { kind, id: 7 }]);
        assert!(publish_impact(projects.clone(), &gone(ItemKind::IssueType)).needs_migration);
        assert!(publish_impact(projects.clone(), &gone(ItemKind::Workflow)).needs_migration);
        assert!(!publish_impact(projects.clone(), &gone(ItemKind::Field)).needs_migration);
        assert!(!publish_impact(projects, &gone(ItemKind::Screen)).needs_migration);
        // Nobody uses it: nothing to migrate.
        assert!(!publish_impact(Vec::<&str>::new(), &gone(ItemKind::IssueType)).needs_migration);
    }

    #[test]
    fn the_default_and_used_configurations_stay() {
        assert_eq!(deletion_refusal(true, 0), Some(DeletionRefusal::IsDefault));
        assert_eq!(deletion_refusal(true, 3), Some(DeletionRefusal::IsDefault));
        assert_eq!(deletion_refusal(false, 6), Some(DeletionRefusal::InUse));
        assert_eq!(deletion_refusal(false, 0), None);
    }
}
