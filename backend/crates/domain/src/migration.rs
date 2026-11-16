//! Migration of tickets when a configuration change is published
//! (specification §5, L04-US08): every status and type the draft removes
//! that tickets use is mapped to one the draft keeps, then a background
//! job moves the tickets and publishes the draft once all are moved.
//!
//! The wizard suggests a target for each removed item ([`suggest_status`],
//! [`suggest_type`]); [`validate_mapping`] refuses a mapping that leaves an
//! item out or names a target that is not offered; [`tickets_to_move`]
//! counts each ticket once. The job moves them batch by batch, skipping
//! those already moved ([`moves`]): a retried job does not move a ticket
//! twice.

use std::collections::{BTreeMap, BTreeSet};

use crate::workflows::Category;

/// A status the tickets of a removed status may move to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusOption<'a> {
    pub id: u128,
    pub key: &'a str,
    pub category: Category,
}

/// The status suggested for the tickets of a removed status: the one with
/// the same identifier, else the first of the same category, else the
/// first one.
pub fn suggest_status(key: &str, category: Category, options: &[StatusOption<'_>]) -> Option<u128> {
    options
        .iter()
        .find(|o| o.key == key)
        .or_else(|| options.iter().find(|o| o.category == category))
        .or_else(|| options.first())
        .map(|o| o.id)
}

/// A type the tickets of a removed type may move to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeOption<'a> {
    pub id: u128,
    pub key: &'a str,
    pub is_subtask: bool,
}

/// The type suggested for the tickets of a removed type: the one with the
/// same identifier, else the first that is a sub-task when it was one (or
/// not), else the first one.
pub fn suggest_type(key: &str, is_subtask: bool, options: &[TypeOption<'_>]) -> Option<u128> {
    options
        .iter()
        .find(|o| o.key == key)
        .or_else(|| options.iter().find(|o| o.is_subtask == is_subtask))
        .or_else(|| options.first())
        .map(|o| o.id)
}

/// A removed item to map, and the targets it may map to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Required {
    pub item: u128,
    pub targets: Vec<u128>,
}

/// What is wrong with a mapping, naming the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MappingError {
    /// A removed item with no target.
    Missing(u128),
    /// A target not offered for this item.
    NotATarget(u128),
    /// An item that is not removed, or mapped twice.
    Unexpected(u128),
}

impl MappingError {
    pub const fn item(self) -> u128 {
        match self {
            Self::Missing(item) | Self::NotATarget(item) | Self::Unexpected(item) => item,
        }
    }

    /// A stable code, for the API and the translations.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Missing(_) => "required",
            Self::NotATarget(_) => "not_a_target",
            Self::Unexpected(_) => "unexpected",
        }
    }
}

/// The mapping `pairs` (removed item, target), checked against what must be
/// mapped: each required item once, to one of its targets, nothing else.
/// Every error at once, by item.
pub fn validate_mapping(
    required: &[Required],
    pairs: &[(u128, u128)],
) -> Result<BTreeMap<u128, u128>, Vec<MappingError>> {
    let mut errors = BTreeSet::new();
    let mut mapping = BTreeMap::new();
    for (item, target) in pairs {
        match required.iter().find(|r| r.item == *item) {
            None => {
                errors.insert(MappingError::Unexpected(*item));
            }
            Some(_) if mapping.contains_key(item) => {
                errors.insert(MappingError::Unexpected(*item));
            }
            Some(r) if !r.targets.contains(target) => {
                errors.insert(MappingError::NotATarget(*item));
            }
            Some(_) => {
                mapping.insert(*item, *target);
            }
        }
    }
    for r in required {
        if !pairs.iter().any(|(item, _)| *item == r.item) {
            errors.insert(MappingError::Missing(r.item));
        }
    }
    if errors.is_empty() {
        Ok(mapping)
    } else {
        Err(errors.into_iter().collect())
    }
}

/// Where a ticket now in `current` (a status or a type) goes: its target if
/// `current` is mapped, none if it already moved or never had to.
pub fn target_of(current: u128, mapping: &BTreeMap<u128, u128>) -> Option<u128> {
    mapping.get(&current).copied()
}

/// The tickets `ids` in batches of `size` (at least one), in their order.
pub fn batches(ids: &[u128], size: usize) -> Vec<&[u128]> {
    ids.chunks(size.max(1)).collect()
}

/// How far a migration of `total` tickets is, in percent; done when there
/// is nothing to move.
pub fn percent(migrated: u64, total: u64) -> u8 {
    if total == 0 {
        return 100;
    }
    let percent = migrated.min(total).saturating_mul(100) / total;
    u8::try_from(percent).unwrap_or(100)
}

/// A ticket as the migration reads it: its id, status and type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub id: u128,
    pub status: u128,
    pub issue_type: u128,
}

/// What the migration does to a ticket: its status and its type, each
/// `(from, to)` when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub id: u128,
    pub status: Option<(u128, u128)>,
    pub issue_type: Option<(u128, u128)>,
}

/// The moves of `tickets` under the mappings of statuses and types, in
/// their order. A ticket no longer in a mapped status or type (moved by an
/// earlier run, or never concerned) is left out: running a batch again
/// moves no ticket twice.
pub fn moves(tickets: &[Ticket], statuses: &BTreeMap<u128, u128>, types: &BTreeMap<u128, u128>) -> Vec<Move> {
    tickets
        .iter()
        .filter_map(|t| {
            let status = target_of(t.status, statuses).map(|to| (t.status, to));
            let issue_type = target_of(t.issue_type, types).map(|to| (t.issue_type, to));
            (status.is_some() || issue_type.is_some()).then_some(Move {
                id: t.id,
                status,
                issue_type,
            })
        })
        .collect()
}

/// The tickets in each `(status, type, count)` of `counts` that a mapping
/// of `statuses` and `types` moves: each ticket once, even when both its
/// status and its type are removed.
pub fn tickets_to_move(
    counts: &[(u128, u128, u64)],
    statuses: &BTreeSet<u128>,
    types: &BTreeSet<u128>,
) -> u64 {
    counts
        .iter()
        .filter(|(status, issue_type, _)| statuses.contains(status) || types.contains(issue_type))
        .map(|(_, _, count)| count)
        .sum()
}

/// Where a migration stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Queued: the job has not started.
    Pending,
    /// Moving tickets.
    Running,
    /// Every ticket moved, the draft published.
    Done,
    /// Stopped for good (the draft changed meanwhile…); nothing published.
    Failed,
}

impl State {
    pub const ALL: [Self; 4] = [Self::Pending, Self::Running, Self::Done, Self::Failed];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == raw)
    }

    /// Whether the job has nothing left to do.
    pub const fn is_over(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODO: u128 = 1;
    const IN_PROGRESS: u128 = 2;
    const IN_ACCEPTANCE: u128 = 3;
    const DONE: u128 = 4;
    const IN_TEST: u128 = 9;

    fn statuses() -> Vec<StatusOption<'static>> {
        vec![
            StatusOption {
                id: TODO,
                key: "todo",
                category: Category::Todo,
            },
            StatusOption {
                id: IN_PROGRESS,
                key: "in_progress",
                category: Category::InProgress,
            },
            StatusOption {
                id: IN_ACCEPTANCE,
                key: "in_acceptance",
                category: Category::InProgress,
            },
            StatusOption {
                id: DONE,
                key: "done",
                category: Category::Done,
            },
        ]
    }

    #[test]
    fn a_status_is_suggested_by_identifier_then_category() {
        // "In test" goes: the first status in progress is suggested.
        assert_eq!(
            suggest_status("in_test", Category::InProgress, &statuses()),
            Some(IN_PROGRESS)
        );
        assert_eq!(
            suggest_status("in_acceptance", Category::Todo, &statuses()),
            Some(IN_ACCEPTANCE)
        );
        assert_eq!(suggest_status("closed", Category::Done, &statuses()), Some(DONE));
        assert_eq!(suggest_status("x", Category::Done, &statuses()[..2]), Some(TODO));
        assert_eq!(suggest_status("x", Category::Done, &[]), None);
    }

    #[test]
    fn a_type_is_suggested_by_identifier_then_sub_task_or_not() {
        let types = [
            TypeOption {
                id: 10,
                key: "story",
                is_subtask: false,
            },
            TypeOption {
                id: 11,
                key: "subtask",
                is_subtask: true,
            },
        ];
        assert_eq!(suggest_type("story", true, &types), Some(10));
        assert_eq!(suggest_type("defect", false, &types), Some(10));
        assert_eq!(suggest_type("chore", true, &types), Some(11));
        assert_eq!(suggest_type("x", true, &types[..1]), Some(10));
    }

    #[test]
    fn a_mapping_names_every_removed_item_once_to_an_offered_target() {
        let required = [
            Required {
                item: IN_TEST,
                targets: vec![TODO, IN_PROGRESS, IN_ACCEPTANCE, DONE],
            },
            Required {
                item: 8,
                targets: vec![TODO, DONE],
            },
        ];
        assert_eq!(
            validate_mapping(&required, &[(IN_TEST, IN_ACCEPTANCE), (8, DONE)]),
            Ok([(IN_TEST, IN_ACCEPTANCE), (8, DONE)].into())
        );
        // One left out: the button stays disabled, the API refuses.
        assert_eq!(
            validate_mapping(&required, &[(IN_TEST, IN_ACCEPTANCE)]),
            Err(vec![MappingError::Missing(8)])
        );
        assert_eq!(
            validate_mapping(
                &required,
                &[(IN_TEST, IN_ACCEPTANCE), (8, IN_PROGRESS), (8, DONE), (77, TODO)]
            ),
            Err(vec![MappingError::NotATarget(8), MappingError::Unexpected(77),])
        );
        assert_eq!(
            validate_mapping(&required, &[(IN_TEST, TODO), (IN_TEST, DONE), (8, DONE)]),
            Err(vec![MappingError::Unexpected(IN_TEST)])
        );
        assert_eq!(MappingError::Missing(8).code(), "required");
        assert_eq!(MappingError::NotATarget(8).item(), 8);
        // Nothing removed: nothing to map.
        assert_eq!(validate_mapping(&[], &[]), Ok(BTreeMap::new()));
    }

    #[test]
    fn a_ticket_moves_once_however_often_the_job_runs() {
        let mapping: BTreeMap<u128, u128> = [(IN_TEST, IN_ACCEPTANCE)].into();
        // Still in "In test": it moves.
        assert_eq!(target_of(IN_TEST, &mapping), Some(IN_ACCEPTANCE));
        // Moved by a previous attempt, or never concerned: left alone.
        assert_eq!(target_of(IN_ACCEPTANCE, &mapping), None);
        assert_eq!(target_of(DONE, &mapping), None);
    }

    #[test]
    fn tickets_move_in_batches_and_progress_counts_them() {
        let ids: Vec<u128> = (1..=7).collect();
        let planned = batches(&ids, 3);
        assert_eq!(planned, [&[1, 2, 3][..], &[4, 5, 6], &[7]]);
        assert_eq!(batches(&ids, 0).len(), 7, "at least one per batch");
        assert!(batches(&[], 3).is_empty());
        assert_eq!(percent(0, 2172), 0);
        assert_eq!(percent(1086, 2172), 50);
        assert_eq!(percent(2172, 2172), 100);
        assert_eq!(percent(0, 0), 100, "nothing to move: done");
    }

    #[test]
    fn a_batch_moves_the_tickets_still_mapped_and_none_twice() {
        const STORY: u128 = 20;
        const DEFECT: u128 = 21;
        const BUG: u128 = 22;
        let statuses: BTreeMap<u128, u128> = [(IN_TEST, IN_ACCEPTANCE)].into();
        let types: BTreeMap<u128, u128> = [(DEFECT, BUG)].into();
        let ticket = |id, status, issue_type| Ticket {
            id,
            status,
            issue_type,
        };
        let batch = [
            ticket(1, IN_TEST, STORY),
            ticket(2, TODO, DEFECT),
            ticket(3, IN_TEST, DEFECT),
            ticket(4, DONE, STORY),
        ];
        let planned = moves(&batch, &statuses, &types);
        assert_eq!(
            planned,
            [
                Move {
                    id: 1,
                    status: Some((IN_TEST, IN_ACCEPTANCE)),
                    issue_type: None
                },
                Move {
                    id: 2,
                    status: None,
                    issue_type: Some((DEFECT, BUG))
                },
                // Both removed: one move, both changes.
                Move {
                    id: 3,
                    status: Some((IN_TEST, IN_ACCEPTANCE)),
                    issue_type: Some((DEFECT, BUG))
                },
            ]
        );
        // The same batch read again once moved: nothing left to do.
        let moved: Vec<Ticket> = batch
            .iter()
            .map(|t| {
                let m = planned.iter().find(|m| m.id == t.id);
                ticket(
                    t.id,
                    m.and_then(|m| m.status).map_or(t.status, |(_, to)| to),
                    m.and_then(|m| m.issue_type).map_or(t.issue_type, |(_, to)| to),
                )
            })
            .collect();
        assert!(moves(&moved, &statuses, &types).is_empty());
        assert!(moves(&batch, &BTreeMap::new(), &BTreeMap::new()).is_empty());
    }

    #[test]
    fn a_ticket_in_a_removed_status_and_type_counts_once() {
        const STORY: u128 = 20;
        const DEFECT: u128 = 21;
        let counts = [
            (IN_TEST, STORY, 5),
            (IN_TEST, DEFECT, 2),
            (TODO, DEFECT, 3),
            (TODO, STORY, 40),
        ];
        let statuses: BTreeSet<u128> = [IN_TEST].into();
        let types: BTreeSet<u128> = [DEFECT].into();
        assert_eq!(tickets_to_move(&counts, &statuses, &types), 10);
        assert_eq!(tickets_to_move(&counts, &statuses, &BTreeSet::new()), 7);
        assert_eq!(tickets_to_move(&counts, &BTreeSet::new(), &BTreeSet::new()), 0);
    }

    #[test]
    fn states_round_trip_and_say_when_over() {
        for state in State::ALL {
            assert_eq!(State::parse(state.as_str()), Some(state));
        }
        assert!(State::Done.is_over() && State::Failed.is_over());
        assert!(!State::Pending.is_over() && !State::Running.is_over());
    }
}
