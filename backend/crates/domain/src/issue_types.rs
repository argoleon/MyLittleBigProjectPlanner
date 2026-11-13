//! Issue types of a project configuration and their hierarchy
//! (specification §5, L04-US02).
//!
//! Each type lists the types a ticket of its type may sit under (its
//! allowed parents). A ticket of a type that is not a sub-task may also
//! have no parent; a sub-task always has one, and is never a parent itself.
//! The depth of the hierarchy follows from these rules: Epic, then Story,
//! then Sub-task. A type is never its own ancestor.

use std::collections::{BTreeMap, BTreeSet};

use crate::i18n;
use crate::validation::{FieldError, Violations};

/// Longest label of a type, in characters, per language.
pub const TYPE_LABEL_MAX_CHARS: usize = 60;

/// The color of a type: one of the `--app-type-*` tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Epic,
    Story,
    Task,
    Bug,
}

impl Color {
    pub const ALL: [Self; 4] = [Self::Epic, Self::Story, Self::Task, Self::Bug];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Epic => "epic",
            Self::Story => "story",
            Self::Task => "task",
            Self::Bug => "bug",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.as_str() == raw)
    }
}

/// Whether `key` is a stable identifier of a type: a lowercase letter, then
/// up to 39 lowercase letters, digits or underscores (`bug`, `user_story`).
pub fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && key.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `icon` names a Material Symbol: lowercase letters, digits and
/// underscores (`bug_report`).
pub fn is_icon(icon: &str) -> bool {
    !icon.is_empty()
        && icon.len() <= 60
        && icon
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// What an admin sets on a type; `key` only when creating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInput<'a> {
    pub key: Option<&'a str>,
    pub label_translations: &'a BTreeMap<String, String>,
    pub icon: &'a str,
}

/// The fields of a type, all errors at once.
pub fn validate_type(input: &TypeInput<'_>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if let Some(key) = input.key
        && !is_key(key)
    {
        v.add("key", "format");
    }
    if let Err(errors) = i18n::validate_translations(
        "labelTranslations",
        input.label_translations,
        TYPE_LABEL_MAX_CHARS,
    ) {
        for error in errors {
            v.add(error.field, error.code);
        }
    }
    if !is_icon(input.icon) {
        v.add("icon", "format");
    }
    v.into_result()
}

/// A type in its hierarchy: its identity, whether it is a sub-task, and the
/// types it may sit under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeNode {
    pub id: u128,
    pub is_subtask: bool,
    pub parents: Vec<u128>,
}

/// Why a hierarchy of types is refused, naming the type at fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyRefusal {
    /// It lists a parent that is no type of the configuration.
    UnknownParent(u128),
    /// It would be its own ancestor.
    OwnAncestor(u128),
    /// A sub-task with no parent type: its tickets would have nowhere to go.
    SubtaskWithoutParent(u128),
    /// A sub-task type is a parent of this type: sub-tasks are leaves.
    SubtaskAsParent(u128),
}

/// Why `types` do not make a valid hierarchy, if they do not; the type
/// `focus` (the one being changed) is reported first.
pub fn hierarchy_refusal(types: &[TypeNode], focus: Option<u128>) -> Option<HierarchyRefusal> {
    let by_id: BTreeMap<u128, &TypeNode> = types.iter().map(|t| (t.id, t)).collect();
    let mut order: Vec<&TypeNode> = types.iter().collect();
    order.sort_by_key(|t| Some(t.id) != focus);
    for node in &order {
        for parent in &node.parents {
            match by_id.get(parent) {
                None => return Some(HierarchyRefusal::UnknownParent(node.id)),
                Some(p) if p.is_subtask => return Some(HierarchyRefusal::SubtaskAsParent(node.id)),
                Some(_) => {}
            }
        }
        if node.is_subtask && node.parents.is_empty() {
            return Some(HierarchyRefusal::SubtaskWithoutParent(node.id));
        }
    }
    order
        .into_iter()
        .find(|node| is_own_ancestor(node.id, &by_id))
        .map(|node| HierarchyRefusal::OwnAncestor(node.id))
}

/// Whether `id` is reached again by going up its parents.
fn is_own_ancestor(id: u128, by_id: &BTreeMap<u128, &TypeNode>) -> bool {
    let mut seen = BTreeSet::new();
    let mut pending: Vec<u128> = by_id.get(&id).map(|t| t.parents.clone()).unwrap_or_default();
    while let Some(current) = pending.pop() {
        if current == id {
            return true;
        }
        if seen.insert(current)
            && let Some(node) = by_id.get(&current)
        {
            pending.extend(node.parents.iter().copied());
        }
    }
    false
}

/// The level of each type in a valid hierarchy: 1 for a type with no
/// parent, else one more than its deepest parent (Epic 1, Story 2,
/// Sub-task 3).
pub fn levels(types: &[TypeNode]) -> BTreeMap<u128, u32> {
    let by_id: BTreeMap<u128, &TypeNode> = types.iter().map(|t| (t.id, t)).collect();
    let mut levels = BTreeMap::new();
    for node in types {
        level(node.id, &by_id, &mut levels, &mut BTreeSet::new());
    }
    levels
}

fn level(
    id: u128,
    by_id: &BTreeMap<u128, &TypeNode>,
    levels: &mut BTreeMap<u128, u32>,
    path: &mut BTreeSet<u128>,
) -> u32 {
    if let Some(known) = levels.get(&id) {
        return *known;
    }
    // Guards against a cycle, which a valid hierarchy has not.
    if !path.insert(id) {
        return 1;
    }
    let parents = by_id.get(&id).map(|t| t.parents.clone()).unwrap_or_default();
    let deepest = parents
        .into_iter()
        .filter(|p| by_id.contains_key(p))
        .map(|p| level(p, by_id, levels, path))
        .max()
        .unwrap_or(0);
    path.remove(&id);
    levels.insert(id, deepest + 1);
    deepest + 1
}

/// Why a type cannot be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalRefusal {
    /// It is the last type that is not a sub-task: tickets need one.
    LastType,
    /// A sub-task type has no other parent type: change it first.
    OrphanSubtask,
}

/// The hierarchy once `removed` is gone: no longer a parent of any type.
pub fn without(types: &[TypeNode], removed: u128) -> Vec<TypeNode> {
    types
        .iter()
        .filter(|t| t.id != removed)
        .map(|t| TypeNode {
            id: t.id,
            is_subtask: t.is_subtask,
            parents: t.parents.iter().copied().filter(|p| *p != removed).collect(),
        })
        .collect()
}

/// Why `removed` cannot leave `types`, if it cannot.
pub fn removal_refusal(types: &[TypeNode], removed: u128) -> Option<RemovalRefusal> {
    let rest = without(types, removed);
    if !rest.iter().any(|t| !t.is_subtask) {
        Some(RemovalRefusal::LastType)
    } else if rest.iter().any(|t| t.is_subtask && t.parents.is_empty()) {
        Some(RemovalRefusal::OrphanSubtask)
    } else {
        None
    }
}

/// Whether `order` lists each of `ids` exactly once.
pub fn is_permutation(order: &[u128], ids: &[u128]) -> bool {
    let wanted: BTreeSet<u128> = ids.iter().copied().collect();
    let given: BTreeSet<u128> = order.iter().copied().collect();
    order.len() == ids.len() && given.len() == order.len() && given == wanted
}

/// A type every new configuration starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultType {
    pub key: &'static str,
    pub label_fr: &'static str,
    pub label_en: &'static str,
    pub icon: &'static str,
    pub color: Color,
    pub is_subtask: bool,
    /// Keys of its allowed parents.
    pub parents: &'static [&'static str],
}

/// The default types (`app_seed_default_issue_types`; an integration test
/// keeps both in step), in their order.
pub const DEFAULT_TYPES: [DefaultType; 5] = [
    DefaultType {
        key: "epic",
        label_fr: "Epic",
        label_en: "Epic",
        icon: "bolt",
        color: Color::Epic,
        is_subtask: false,
        parents: &[],
    },
    DefaultType {
        key: "story",
        label_fr: "Story",
        label_en: "Story",
        icon: "bookmark",
        color: Color::Story,
        is_subtask: false,
        parents: &["epic"],
    },
    DefaultType {
        key: "task",
        label_fr: "Tâche",
        label_en: "Task",
        icon: "task_alt",
        color: Color::Task,
        is_subtask: false,
        parents: &["epic"],
    },
    DefaultType {
        key: "bug",
        label_fr: "Bug",
        label_en: "Bug",
        icon: "bug_report",
        color: Color::Bug,
        is_subtask: false,
        parents: &["epic"],
    },
    DefaultType {
        key: "subtask",
        label_fr: "Sous-tâche",
        label_en: "Sub-task",
        icon: "subdirectory_arrow_right",
        color: Color::Task,
        is_subtask: true,
        parents: &["story", "task", "bug"],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const EPIC: u128 = 1;
    const STORY: u128 = 2;
    const TASK: u128 = 3;
    const BUG: u128 = 4;
    const SUBTASK: u128 = 5;

    fn node(id: u128, is_subtask: bool, parents: &[u128]) -> TypeNode {
        TypeNode {
            id,
            is_subtask,
            parents: parents.to_vec(),
        }
    }

    /// The default hierarchy, by id.
    fn defaults() -> Vec<TypeNode> {
        vec![
            node(EPIC, false, &[]),
            node(STORY, false, &[EPIC]),
            node(TASK, false, &[EPIC]),
            node(BUG, false, &[EPIC]),
            node(SUBTASK, true, &[STORY, TASK, BUG]),
        ]
    }

    fn with(types: &[TypeNode], changed: TypeNode) -> Vec<TypeNode> {
        let mut types: Vec<TypeNode> = types.iter().filter(|t| t.id != changed.id).cloned().collect();
        types.push(changed);
        types
    }

    #[test]
    fn keys_and_icons_follow_their_formats() {
        for key in ["bug", "user_story", "t2", "a"] {
            assert!(is_key(key), "{key}");
        }
        for key in [
            "",
            "Bug",
            "2bug",
            "_bug",
            "user-story",
            "bogue é",
            &"x".repeat(41),
        ] {
            assert!(!is_key(key), "{key}");
        }
        assert!(is_key(&"x".repeat(40)));
        assert!(is_icon("bug_report") && is_icon("looks_3"));
        assert!(!is_icon("") && !is_icon("Bug") && !is_icon("bug-report") && !is_icon(&"x".repeat(61)));
    }

    #[test]
    fn colors_round_trip() {
        for color in Color::ALL {
            assert_eq!(Color::parse(color.as_str()), Some(color));
        }
        assert_eq!(Color::parse("red"), None);
    }

    #[test]
    fn a_type_reports_every_invalid_field() {
        let labels: BTreeMap<String, String> = [("de".to_owned(), "Fehler".to_owned())].into();
        let errors = validate_type(&TypeInput {
            key: Some("Bug"),
            label_translations: &labels,
            icon: "bug-report",
        })
        .unwrap_err();
        let fields: Vec<(&str, &str)> = errors.iter().map(|e| (e.field.as_str(), e.code)).collect();
        assert_eq!(
            fields,
            [
                ("key", "format"),
                ("labelTranslations", "required"),
                ("labelTranslations.de", "unsupported_locale"),
                ("icon", "format"),
            ]
        );
        let labels: BTreeMap<String, String> = [("fr".to_owned(), "Bogue".to_owned())].into();
        let fine = TypeInput {
            key: None,
            label_translations: &labels,
            icon: "bug_report",
        };
        assert!(validate_type(&fine).is_ok(), "no key when changing a type");
    }

    #[test]
    fn the_default_hierarchy_is_valid_and_three_levels_deep() {
        assert_eq!(hierarchy_refusal(&defaults(), None), None);
        let levels = levels(&defaults());
        assert_eq!(levels[&EPIC], 1);
        assert_eq!((levels[&STORY], levels[&TASK], levels[&BUG]), (2, 2, 2));
        assert_eq!(levels[&SUBTASK], 3);
    }

    #[test]
    fn the_depth_follows_the_allowed_parents() {
        // Epic > Story > Task > Sub-task: a chain of four.
        let chain = vec![
            node(EPIC, false, &[]),
            node(STORY, false, &[EPIC]),
            node(TASK, false, &[STORY]),
            node(SUBTASK, true, &[TASK]),
        ];
        assert_eq!(hierarchy_refusal(&chain, None), None);
        assert_eq!(levels(&chain).values().copied().collect::<Vec<_>>(), [1, 2, 3, 4]);
        // Several parents: the deepest one counts.
        let mixed = with(&chain, node(BUG, false, &[EPIC, TASK]));
        assert_eq!(levels(&mixed)[&BUG], 4);
    }

    #[test]
    fn a_type_is_never_its_own_ancestor() {
        // Itself.
        let own = with(&defaults(), node(STORY, false, &[EPIC, STORY]));
        assert_eq!(
            hierarchy_refusal(&own, Some(STORY)),
            Some(HierarchyRefusal::OwnAncestor(STORY))
        );
        // Through another: Epic under Story, under Epic.
        let cycle = with(&defaults(), node(EPIC, false, &[STORY]));
        assert_eq!(
            hierarchy_refusal(&cycle, Some(EPIC)),
            Some(HierarchyRefusal::OwnAncestor(EPIC))
        );
        // The type being changed is reported, not the other one of the cycle.
        assert_eq!(
            hierarchy_refusal(&cycle, Some(STORY)),
            Some(HierarchyRefusal::OwnAncestor(STORY))
        );
    }

    #[test]
    fn sub_tasks_have_a_parent_and_are_none() {
        let orphan = with(&defaults(), node(SUBTASK, true, &[]));
        assert_eq!(
            hierarchy_refusal(&orphan, Some(SUBTASK)),
            Some(HierarchyRefusal::SubtaskWithoutParent(SUBTASK))
        );
        let under_subtask = with(&defaults(), node(BUG, false, &[SUBTASK]));
        assert_eq!(
            hierarchy_refusal(&under_subtask, Some(BUG)),
            Some(HierarchyRefusal::SubtaskAsParent(BUG))
        );
        // Story becomes a sub-task: it would be a parent of Sub-task.
        let story_subtask = with(&defaults(), node(STORY, true, &[EPIC]));
        assert_eq!(
            hierarchy_refusal(&story_subtask, Some(STORY)),
            Some(HierarchyRefusal::SubtaskAsParent(SUBTASK))
        );
        let unknown = with(&defaults(), node(BUG, false, &[99]));
        assert_eq!(
            hierarchy_refusal(&unknown, Some(BUG)),
            Some(HierarchyRefusal::UnknownParent(BUG))
        );
    }

    #[test]
    fn a_removed_type_leaves_the_parents_of_the_others() {
        let rest = without(&defaults(), EPIC);
        assert_eq!(rest.len(), 4);
        assert!(rest.iter().all(|t| !t.parents.contains(&EPIC)));
        assert_eq!(removal_refusal(&defaults(), EPIC), None);
        assert_eq!(removal_refusal(&defaults(), BUG), None);
        assert_eq!(removal_refusal(&defaults(), SUBTASK), None);
    }

    #[test]
    fn the_last_type_and_the_last_parent_of_a_sub_task_stay() {
        let last = vec![node(TASK, false, &[]), node(SUBTASK, true, &[TASK])];
        assert_eq!(removal_refusal(&last, TASK), Some(RemovalRefusal::LastType));
        let two = vec![
            node(EPIC, false, &[]),
            node(TASK, false, &[]),
            node(SUBTASK, true, &[TASK]),
        ];
        assert_eq!(removal_refusal(&two, TASK), Some(RemovalRefusal::OrphanSubtask));
        assert_eq!(removal_refusal(&two, EPIC), None);
    }

    #[test]
    fn an_order_lists_every_type_once() {
        let ids = [EPIC, STORY, TASK];
        assert!(is_permutation(&[TASK, EPIC, STORY], &ids));
        assert!(!is_permutation(&[TASK, EPIC], &ids), "one missing");
        assert!(!is_permutation(&[TASK, EPIC, EPIC], &ids), "one twice");
        assert!(!is_permutation(&[TASK, EPIC, BUG], &ids), "another one");
    }

    #[test]
    fn the_default_types_are_a_valid_hierarchy_by_key() {
        let id = |key: &str| {
            DEFAULT_TYPES
                .iter()
                .position(|t| t.key == key)
                .map(|i| i as u128)
                .unwrap_or(99)
        };
        let nodes: Vec<TypeNode> = DEFAULT_TYPES
            .iter()
            .map(|t| {
                node(
                    id(t.key),
                    t.is_subtask,
                    &t.parents.iter().map(|p| id(p)).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(hierarchy_refusal(&nodes, None), None);
        for t in DEFAULT_TYPES {
            assert!(is_key(t.key) && is_icon(t.icon), "{}", t.key);
        }
    }
}
