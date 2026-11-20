//! Bulk actions on a search result (L06-US08): several tickets changed at
//! once, each on its own, the others changed even when some are refused.
//!
//! - Up to [`SYNC_MAX`] tickets, the request changes them; beyond (all the
//!   results of a query), the worker does, one job per ticket, and the
//!   member is notified when all are done. Never more than [`MAX`].
//! - Modify sets system fields, and the custom fields every selected
//!   ticket's type has in the same configuration ([`shared`]).
//! - Change status offers the statuses some transition reaches; each
//!   ticket runs the first transition its workflow offers to that status
//!   ([`pick_transition`]).
//! - Move maps types and statuses once for the batch; a sub-task whose
//!   parent moves too follows it ([`follows_parent`]).

use std::collections::BTreeSet;

/// Most tickets changed within the request.
pub const SYNC_MAX: usize = 100;
/// Most tickets of one bulk action.
pub const MAX: usize = 10_000;
/// How long a bulk action and its report are kept, in days.
pub const LIFETIME_DAYS: i64 = 7;

/// What a bulk action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Modify,
    Transition,
    Move,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Modify, Self::Transition, Self::Move];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Modify => "modify",
            Self::Transition => "transition",
            Self::Move => "move",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

/// What became of a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Changed as asked.
    Changed,
    /// Already as asked: nothing to change.
    Unchanged,
    /// Left as it was: see the reason.
    Refused,
    /// Moved with its parent, which was in the selection too.
    WithParent,
}

impl Outcome {
    pub const ALL: [Self; 4] = [Self::Changed, Self::Unchanged, Self::Refused, Self::WithParent];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Unchanged => "unchanged",
            Self::Refused => "refused",
            Self::WithParent => "with_parent",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == raw)
    }
}

/// Why a ticket was left as it was: stable codes the screens translate.
pub mod reason {
    /// Gone, or not visible to the member any more.
    pub const NOT_FOUND: &str = "not_found";
    /// The member may not edit it (or create in the target project).
    pub const PERMISSION_DENIED: &str = "permission.denied";
    /// Its project is archived.
    pub const NODE_ARCHIVED: &str = "node.archived";
    /// A value refused: the field errors say which.
    pub const VALIDATION_FAILED: &str = "validation_failed";
    /// No transition of its workflow leads to the status.
    pub const NO_TRANSITION: &str = "transition.none";
    /// The transitions leading there are not the member's to run.
    pub const RESTRICTED: &str = "transition.restricted";
    /// A sub-task moves with its parent only.
    pub const SUBTASK_ALONE: &str = "issue.subtask_moves";
    /// Already in the target project.
    pub const SAME_PROJECT: &str = "same_project";
    /// A type or status the plan maps nowhere.
    pub const NOT_MAPPED: &str = "move.not_mapped";
}

/// How many tickets a selection holds, as an action takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// None: nothing to do.
    Empty,
    /// Within the request.
    Now,
    /// By the worker.
    Background,
    /// More than [`MAX`].
    TooMany,
}

/// How `count` selected tickets are changed.
pub const fn run_of(count: usize) -> Run {
    if count == 0 {
        Run::Empty
    } else if count <= SYNC_MAX {
        Run::Now
    } else if count <= MAX {
        Run::Background
    } else {
        Run::TooMany
    }
}

/// The custom fields every selected ticket has: the intersection of each
/// ticket's fields (an empty selection has none).
pub fn shared(each: &[BTreeSet<u128>]) -> BTreeSet<u128> {
    let mut sets = each.iter();
    let Some(first) = sets.next() else {
        return BTreeSet::new();
    };
    sets.fold(first.clone(), |all, set| all.intersection(set).copied().collect())
}

/// A transition leaving a ticket's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaving {
    /// Key of the status it leads to.
    pub to_status: String,
    /// The member meets its condition.
    pub allowed: bool,
}

/// What changing a ticket's status to `target` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Already in that status.
    AlreadyThere,
    NoTransition,
    Restricted,
    /// The first allowed transition leading there, by workflow order.
    Run(usize),
}

/// The transition to run on a ticket in the status `current`, among
/// those `leaving` it, to reach the status `target` (keys).
pub fn pick_transition(current: &str, target: &str, leaving: &[Leaving]) -> Pick {
    if current == target {
        return Pick::AlreadyThere;
    }
    let mut leading = leaving
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_status == target)
        .peekable();
    if leading.peek().is_none() {
        return Pick::NoTransition;
    }
    leading
        .find(|(_, l)| l.allowed)
        .map_or(Pick::Restricted, |(i, _)| Pick::Run(i))
}

/// Whether a ticket whose parent is `parent` moves with it: its parent is
/// selected too (and moves its sub-tasks).
pub fn follows_parent(parent: Option<u128>, selected: &BTreeSet<u128>) -> bool {
    parent.is_some_and(|p| selected.contains(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_selections_run_now_large_ones_in_the_background() {
        assert_eq!(run_of(0), Run::Empty);
        assert_eq!(run_of(1), Run::Now);
        assert_eq!(run_of(SYNC_MAX), Run::Now);
        assert_eq!(run_of(SYNC_MAX + 1), Run::Background);
        assert_eq!(run_of(MAX), Run::Background);
        assert_eq!(run_of(MAX + 1), Run::TooMany);
    }

    #[test]
    fn shared_fields_are_those_every_ticket_has() {
        let a = BTreeSet::from([1, 2, 3]);
        let b = BTreeSet::from([2, 3, 4]);
        let c = BTreeSet::from([3, 2]);
        assert_eq!(shared(&[a.clone(), b, c]), BTreeSet::from([2, 3]));
        assert_eq!(shared(std::slice::from_ref(&a)), a);
        assert_eq!(shared(&[a, BTreeSet::new()]), BTreeSet::new());
        assert_eq!(shared(&[]), BTreeSet::new());
    }

    fn leaving(to: &str, allowed: bool) -> Leaving {
        Leaving {
            to_status: to.to_owned(),
            allowed,
        }
    }

    #[test]
    fn the_first_allowed_transition_to_the_status_runs() {
        let options = [
            leaving("done", false),
            leaving("in_review", true),
            leaving("done", true),
        ];
        assert_eq!(pick_transition("in_progress", "done", &options), Pick::Run(2));
        assert_eq!(
            pick_transition("in_progress", "in_review", &options),
            Pick::Run(1)
        );
        assert_eq!(
            pick_transition("in_progress", "todo", &options),
            Pick::NoTransition
        );
        assert_eq!(pick_transition("done", "done", &options), Pick::AlreadyThere);
        assert_eq!(
            pick_transition("in_progress", "done", &[leaving("done", false)]),
            Pick::Restricted
        );
    }

    #[test]
    fn a_subtask_follows_its_parent_when_both_are_selected() {
        let selected = BTreeSet::from([1, 2]);
        assert!(follows_parent(Some(1), &selected));
        assert!(!follows_parent(Some(9), &selected));
        assert!(!follows_parent(None, &selected));
    }

    #[test]
    fn identifiers_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        for outcome in Outcome::ALL {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
        }
        assert_eq!(Kind::parse("delete"), None);
    }
}
