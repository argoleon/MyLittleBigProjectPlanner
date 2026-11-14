//! Workflows of a project configuration (specification §5, L04-US04): the
//! statuses a ticket goes through and the transitions between them.
//!
//! Every status falls into one category (To do, In progress, Done), which
//! aggregates, boards and burndown read whatever its label. One status is
//! the initial one; every other must be reachable from it. A transition
//! leads to a status from one status, or from any (`from` none).
//!
//! A draft may hold an incomplete workflow while it is being built:
//! [`problems`] lists what keeps it from being published.

use std::collections::{BTreeMap, BTreeSet};

/// The category of a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Todo,
    InProgress,
    Done,
}

impl Category {
    pub const ALL: [Self; 3] = [Self::Todo, Self::InProgress, Self::Done];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.as_str() == raw)
    }
}

/// Longest label of a workflow, a status or a transition, in characters.
pub const WORKFLOW_LABEL_MAX_CHARS: usize = 60;

/// A status in its workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusNode {
    pub id: u128,
    pub is_initial: bool,
}

/// A transition to `to`, from `from` or from any status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionEdge {
    pub id: u128,
    pub from: Option<u128>,
    pub to: u128,
}

/// What keeps a workflow from being published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Problem {
    /// It has no status.
    NoStatus,
    /// None of its statuses is the initial one.
    NoInitial,
    /// Nothing leads to this status from the initial one.
    Unreachable(u128),
    /// This transition repeats another one: same origin, same target.
    Duplicate(u128),
}

impl Problem {
    /// A stable code, for the API and the translations.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoStatus => "no_status",
            Self::NoInitial => "no_initial",
            Self::Unreachable(_) => "unreachable",
            Self::Duplicate(_) => "duplicate_transition",
        }
    }
}

/// The statuses reached from the initial one, itself included. A
/// transition from any status leads to its target as soon as one status is
/// reached.
pub fn reachable(statuses: &[StatusNode], transitions: &[TransitionEdge]) -> BTreeSet<u128> {
    let mut reached: BTreeSet<u128> = statuses.iter().filter(|s| s.is_initial).map(|s| s.id).collect();
    let mut pending: Vec<u128> = reached.iter().copied().collect();
    if !reached.is_empty() {
        for transition in transitions.iter().filter(|t| t.from.is_none()) {
            if reached.insert(transition.to) {
                pending.push(transition.to);
            }
        }
    }
    while let Some(current) = pending.pop() {
        for transition in transitions.iter().filter(|t| t.from == Some(current)) {
            if reached.insert(transition.to) {
                pending.push(transition.to);
            }
        }
    }
    reached
}

/// What keeps the workflow from being published, in a stable order;
/// none: it is ready.
pub fn problems(statuses: &[StatusNode], transitions: &[TransitionEdge]) -> Vec<Problem> {
    let mut found = Vec::new();
    if statuses.is_empty() {
        found.push(Problem::NoStatus);
        return found;
    }
    if !statuses.iter().any(|s| s.is_initial) {
        found.push(Problem::NoInitial);
    } else {
        let reached = reachable(statuses, transitions);
        found.extend(
            statuses
                .iter()
                .filter(|s| !reached.contains(&s.id))
                .map(|s| Problem::Unreachable(s.id)),
        );
    }
    let mut seen: BTreeMap<(Option<u128>, u128), u128> = BTreeMap::new();
    for transition in transitions {
        if seen
            .insert((transition.from, transition.to), transition.id)
            .is_some()
        {
            found.push(Problem::Duplicate(transition.id));
        }
    }
    found
}

/// The transitions a ticket in status `from` may take: those from it and
/// those from any status, except to itself.
pub fn available(transitions: &[TransitionEdge], from: u128) -> Vec<u128> {
    transitions
        .iter()
        .filter(|t| (t.from == Some(from) || t.from.is_none()) && t.to != from)
        .map(|t| t.id)
        .collect()
}

/// Why a workflow cannot be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalRefusal {
    /// Issue types follow it: they move to another workflow first.
    InUse,
    /// It is the last workflow of the configuration.
    Last,
}

/// Why a workflow followed by `types` cannot leave a configuration of
/// `workflows`, if it cannot.
pub const fn removal_refusal(workflows: usize, types: usize) -> Option<RemovalRefusal> {
    if types > 0 {
        Some(RemovalRefusal::InUse)
    } else if workflows <= 1 {
        Some(RemovalRefusal::Last)
    } else {
        None
    }
}

/// A status of the standard workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultStatus {
    pub key: &'static str,
    pub label_fr: &'static str,
    pub label_en: &'static str,
    pub portal_fr: &'static str,
    pub portal_en: &'static str,
    pub category: Category,
    pub is_initial: bool,
}

/// A transition of the standard workflow; `from` none: from any status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultTransition {
    pub name_fr: &'static str,
    pub name_en: &'static str,
    pub from: Option<&'static str>,
    pub to: &'static str,
}

/// The standard workflow of mockup 11, which every new configuration and
/// its default types start with (`app_seed_default_workflow`; an
/// integration test keeps both in step).
pub const STANDARD_KEY: &str = "standard";

pub const STANDARD_STATUSES: [DefaultStatus; 5] = [
    DefaultStatus {
        key: "todo",
        label_fr: "À faire",
        label_en: "To do",
        portal_fr: "Reçue",
        portal_en: "Received",
        category: Category::Todo,
        is_initial: true,
    },
    DefaultStatus {
        key: "in_progress",
        label_fr: "En cours",
        label_en: "In progress",
        portal_fr: "En cours de traitement",
        portal_en: "Being processed",
        category: Category::InProgress,
        is_initial: false,
    },
    DefaultStatus {
        key: "in_review",
        label_fr: "En revue",
        label_en: "In review",
        portal_fr: "En cours de traitement",
        portal_en: "Being processed",
        category: Category::InProgress,
        is_initial: false,
    },
    DefaultStatus {
        key: "done",
        label_fr: "Terminé",
        label_en: "Done",
        portal_fr: "Résolue",
        portal_en: "Resolved",
        category: Category::Done,
        is_initial: false,
    },
    DefaultStatus {
        key: "cancelled",
        label_fr: "Annulé",
        label_en: "Cancelled",
        portal_fr: "Fermée",
        portal_en: "Closed",
        category: Category::Done,
        is_initial: false,
    },
];

pub const STANDARD_TRANSITIONS: [DefaultTransition; 5] = [
    DefaultTransition {
        name_fr: "Démarrer",
        name_en: "Start",
        from: Some("todo"),
        to: "in_progress",
    },
    DefaultTransition {
        name_fr: "Passer en revue",
        name_en: "Send to review",
        from: Some("in_progress"),
        to: "in_review",
    },
    DefaultTransition {
        name_fr: "Refuser",
        name_en: "Reject",
        from: Some("in_review"),
        to: "in_progress",
    },
    DefaultTransition {
        name_fr: "Valider",
        name_en: "Validate",
        from: Some("in_review"),
        to: "done",
    },
    DefaultTransition {
        name_fr: "Annuler",
        name_en: "Cancel",
        from: None,
        to: "cancelled",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const TODO: u128 = 1;
    const DOING: u128 = 2;
    const REVIEW: u128 = 3;
    const DONE: u128 = 4;
    const CANCELLED: u128 = 5;

    fn status(id: u128, is_initial: bool) -> StatusNode {
        StatusNode { id, is_initial }
    }

    fn edge(id: u128, from: Option<u128>, to: u128) -> TransitionEdge {
        TransitionEdge { id, from, to }
    }

    fn statuses() -> Vec<StatusNode> {
        vec![
            status(TODO, true),
            status(DOING, false),
            status(REVIEW, false),
            status(DONE, false),
            status(CANCELLED, false),
        ]
    }

    /// The standard workflow, by id.
    fn standard() -> Vec<TransitionEdge> {
        vec![
            edge(10, Some(TODO), DOING),
            edge(11, Some(DOING), REVIEW),
            edge(12, Some(REVIEW), DOING),
            edge(13, Some(REVIEW), DONE),
            edge(14, None, CANCELLED),
        ]
    }

    #[test]
    fn categories_round_trip() {
        for category in Category::ALL {
            assert_eq!(Category::parse(category.as_str()), Some(category));
        }
        assert_eq!(Category::parse("progress"), None);
    }

    #[test]
    fn the_standard_workflow_is_ready() {
        assert_eq!(problems(&statuses(), &standard()), []);
        assert_eq!(reachable(&statuses(), &standard()).len(), 5);
    }

    #[test]
    fn a_workflow_needs_statuses_and_an_initial_one() {
        assert_eq!(problems(&[], &[]), [Problem::NoStatus]);
        let none_initial: Vec<StatusNode> = statuses().into_iter().map(|s| status(s.id, false)).collect();
        assert_eq!(problems(&none_initial, &standard()), [Problem::NoInitial]);
        // A single initial status is a workflow already.
        assert_eq!(problems(&[status(TODO, true)], &[]), []);
    }

    #[test]
    fn every_status_is_reachable_from_the_initial_one() {
        // Nothing leads to Done once Validate is gone.
        let without_validate: Vec<TransitionEdge> = standard().into_iter().filter(|t| t.id != 13).collect();
        assert_eq!(
            problems(&statuses(), &without_validate),
            [Problem::Unreachable(DONE)]
        );
        // A status leading only to itself and the others is still unreached.
        let island = vec![edge(10, Some(TODO), DOING), edge(20, Some(REVIEW), DONE)];
        assert_eq!(
            problems(&statuses(), &island),
            [
                Problem::Unreachable(REVIEW),
                Problem::Unreachable(DONE),
                Problem::Unreachable(CANCELLED)
            ]
        );
    }

    #[test]
    fn a_transition_from_all_statuses_reaches_its_target() {
        let only_any = vec![edge(14, None, CANCELLED)];
        let reached = reachable(&statuses(), &only_any);
        assert!(reached.contains(&TODO) && reached.contains(&CANCELLED));
        assert!(!reached.contains(&DONE));
        // Without an initial status, nothing is reached at all.
        assert!(reachable(&[status(TODO, false), status(CANCELLED, false)], &only_any).is_empty());
    }

    #[test]
    fn a_transition_is_not_repeated() {
        let mut twice = standard();
        twice.push(edge(15, Some(TODO), DOING));
        twice.push(edge(16, None, CANCELLED));
        assert_eq!(
            problems(&statuses(), &twice),
            [Problem::Duplicate(15), Problem::Duplicate(16)]
        );
        // The same target from another origin is another transition.
        let mut other = standard();
        other.push(edge(17, Some(DOING), CANCELLED));
        assert_eq!(problems(&statuses(), &other), []);
    }

    #[test]
    fn the_transitions_offered_from_a_status() {
        assert_eq!(available(&standard(), TODO), [10, 14]);
        assert_eq!(available(&standard(), REVIEW), [12, 13, 14]);
        // Not to itself, even from any status.
        assert_eq!(available(&standard(), CANCELLED), Vec::<u128>::new());
        assert_eq!(available(&standard(), DONE), [14]);
    }

    #[test]
    fn a_workflow_followed_by_types_or_the_last_one_stays() {
        assert_eq!(removal_refusal(2, 3), Some(RemovalRefusal::InUse));
        assert_eq!(removal_refusal(1, 0), Some(RemovalRefusal::Last));
        assert_eq!(removal_refusal(2, 0), None);
    }

    #[test]
    fn problems_have_stable_codes() {
        assert_eq!(Problem::NoStatus.code(), "no_status");
        assert_eq!(Problem::NoInitial.code(), "no_initial");
        assert_eq!(Problem::Unreachable(1).code(), "unreachable");
        assert_eq!(Problem::Duplicate(1).code(), "duplicate_transition");
    }

    #[test]
    fn the_standard_workflow_by_key_is_ready() {
        let id = |key: &str| {
            STANDARD_STATUSES
                .iter()
                .position(|s| s.key == key)
                .map_or(99, |i| i as u128)
        };
        let nodes: Vec<StatusNode> = STANDARD_STATUSES
            .iter()
            .map(|s| status(id(s.key), s.is_initial))
            .collect();
        let edges: Vec<TransitionEdge> = STANDARD_TRANSITIONS
            .iter()
            .enumerate()
            .map(|(i, t)| edge(100 + i as u128, t.from.map(id), id(t.to)))
            .collect();
        assert_eq!(problems(&nodes, &edges), []);
        assert_eq!(STANDARD_STATUSES.iter().filter(|s| s.is_initial).count(), 1);
        for s in STANDARD_STATUSES {
            assert!(crate::issue_types::is_key(s.key), "{}", s.key);
        }
    }
}
