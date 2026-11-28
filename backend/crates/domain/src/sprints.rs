//! Sprints and the backlog of a Scrum board (specification §6, L09-US01).
//!
//! - A sprint belongs to one Scrum board: `future` until started, `active`,
//!   then `closed` (L09-US02). Future and active sprints are *open*.
//! - A ticket is in at most one open sprint of the tenant. A board shows the
//!   open sprints holding tickets of its filter: its own, which its
//!   planners change, and those of other boards, read-only there.
//! - The backlog is the rest of the board's tickets, minus the finished
//!   ones (status category `done`) and the sub-tasks, which follow their
//!   parent into a sprint and only show as a count on its row ([`plan`]).
//! - Every list follows the board's rank, the unranked tickets last in the
//!   order of the filter (`crate::rank::sort_by_rank`).
//! - A board has one active sprint at most. Starting a sprint needs its
//!   dates and snapshots its committed scope ([`check_start`],
//!   [`snapshot`]); closing it snapshots the completed scope and sends the
//!   unfinished tickets to the backlog, a future sprint of the board, or a
//!   new one ([`check_close`], [`close`]) (L09-US02).

use std::collections::BTreeMap;

use crate::dates::Date;
use crate::validation::{FieldError, Violations};

/// Longest sprint name, in characters.
pub const NAME_MAX_CHARS: usize = 100;
/// Longest sprint goal, in characters.
pub const GOAL_MAX_CHARS: usize = 1_000;

/// Where a sprint stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Future,
    Active,
    Closed,
}

impl State {
    pub const ALL: [Self; 3] = [Self::Future, Self::Active, Self::Closed];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Future => "future",
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// Future or active: tickets may join or leave it.
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Closed)
    }
}

/// What a planner writes of a sprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SprintFields {
    pub name: String,
    pub goal: Option<String>,
    pub start: Option<Date>,
    pub end: Option<Date>,
}

/// Checks `fields`: a name of 1 to 100 characters, a goal of 1,000 at most,
/// and both dates or none, the end not before the start.
pub fn validate(fields: &SprintFields) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let name = fields.name.trim().chars().count();
    if name == 0 {
        v.add("name", "required");
    } else if name > NAME_MAX_CHARS {
        v.add("name", "too_long");
    }
    if fields
        .goal
        .as_ref()
        .is_some_and(|g| g.trim().chars().count() > GOAL_MAX_CHARS)
    {
        v.add("goal", "too_long");
    }
    match (fields.start, fields.end) {
        (Some(start), Some(end)) if end < start => v.add("endDate", "before_start"),
        (Some(_), None) => v.add("endDate", "required"),
        (None, Some(_)) => v.add("startDate", "required"),
        _ => {}
    }
    v.into_result()
}

/// The name a new sprint gets when none is given: "PLAT Sprint 4", from
/// the key of the board's node and the sprint's number on the board.
pub fn default_name(prefix: &str, sequence: i32) -> String {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        format!("Sprint {sequence}")
    } else {
        format!("{prefix} Sprint {sequence}")
    }
}

/// A ticket of a board's filter, as the backlog sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item<Id> {
    pub id: Id,
    /// Its parent, for a sub-task.
    pub parent: Option<Id>,
    pub subtask: bool,
    /// Its status is of the `done` category.
    pub done: bool,
    /// In the board's estimation.
    pub estimate: Option<f64>,
    /// The open sprint it is in.
    pub sprint: Option<Id>,
}

/// The count and estimate of a list of tickets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Totals {
    pub count: usize,
    /// The sum of the estimates given.
    pub estimate: f64,
    /// The part of `estimate` already done.
    pub done: f64,
    /// Tickets without an estimate.
    pub unestimated: usize,
}

impl Totals {
    fn add<Id>(&mut self, item: &Item<Id>) {
        self.count += 1;
        match item.estimate {
            Some(estimate) => {
                self.estimate += estimate;
                if item.done {
                    self.done += estimate;
                }
            }
            None => self.unestimated += 1,
        }
    }
}

/// The tickets of a board, as the backlog lists them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan<Id: Ord> {
    /// The tickets of each open sprint, in order.
    pub sprints: BTreeMap<Id, Vec<Id>>,
    pub sprint_totals: BTreeMap<Id, Totals>,
    /// The tickets in no open sprint and not finished, in order.
    pub backlog: Vec<Id>,
    pub backlog_totals: Totals,
    /// The sub-tasks of each listed ticket that the filter holds.
    pub subtasks: BTreeMap<Id, usize>,
}

/// Splits `items`, already in the board's order, between the open sprints
/// and the backlog: sub-tasks are only counted on their parent, finished
/// tickets in no sprint are left out.
pub fn plan<Id: Copy + Ord>(items: &[Item<Id>]) -> Plan<Id> {
    let mut plan = Plan {
        sprints: BTreeMap::new(),
        sprint_totals: BTreeMap::new(),
        backlog: Vec::new(),
        backlog_totals: Totals::default(),
        subtasks: BTreeMap::new(),
    };
    for item in items {
        if item.subtask
            && let Some(parent) = item.parent
        {
            *plan.subtasks.entry(parent).or_default() += 1;
            continue;
        }
        if let Some(sprint) = item.sprint {
            plan.sprints.entry(sprint).or_default().push(item.id);
            plan.sprint_totals.entry(sprint).or_default().add(item);
        } else if !item.done {
            plan.backlog.push(item.id);
            plan.backlog_totals.add(item);
        }
    }
    plan
}

/// Where a planner moves a ticket in the backlog view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<Id> {
    Backlog,
    Sprint(Id),
}

/// The sprint a move involves, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SprintRef<Id> {
    pub id: Id,
    pub board: Id,
    pub state: State,
}

/// Why a ticket cannot move there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRefusal {
    /// A sub-task follows its parent.
    Subtask,
    /// The ticket is in, or would go to, an open sprint of another board:
    /// read-only here.
    Foreign,
    /// The sprint is closed.
    Closed,
}

/// Whether a ticket may move from the open sprint it is in (`from`) to
/// `target`, on `board`.
pub fn check_move<Id: Copy + Eq>(
    board: Id,
    subtask: bool,
    from: Option<SprintRef<Id>>,
    target: Target<SprintRef<Id>>,
) -> Result<(), MoveRefusal> {
    if subtask {
        return Err(MoveRefusal::Subtask);
    }
    if from.is_some_and(|s| s.board != board) {
        return Err(MoveRefusal::Foreign);
    }
    if let Target::Sprint(sprint) = target {
        if sprint.board != board {
            return Err(MoveRefusal::Foreign);
        }
        if !sprint.state.is_open() {
            return Err(MoveRefusal::Closed);
        }
    }
    Ok(())
}

/// Why a sprint cannot start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefusal {
    /// It is already active, or closed.
    NotFuture,
    /// Another sprint of the board is active.
    ActiveExists,
}

/// Whether a sprint in `state` may start, the board having an active sprint
/// or not.
pub fn check_start(state: State, board_has_active: bool) -> Result<(), StartRefusal> {
    if state != State::Future {
        return Err(StartRefusal::NotFuture);
    }
    if board_has_active {
        return Err(StartRefusal::ActiveExists);
    }
    Ok(())
}

/// Checks the fields a sprint starts with: those of [`validate`], the two
/// dates required.
pub fn validate_start(fields: &SprintFields) -> Result<(), Vec<FieldError>> {
    let mut errors = validate(fields).err().unwrap_or_default();
    for (field, date) in [("startDate", fields.start), ("endDate", fields.end)] {
        if date.is_none() && !errors.iter().any(|e| e.field == field) {
            errors.push(FieldError::new(field, "required"));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Where the unfinished tickets of a closing sprint go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTarget<Id> {
    Backlog,
    /// A future sprint of the board.
    Sprint(SprintRef<Id>),
    /// A sprint created for them, after the board's others.
    NewSprint,
}

/// Why a sprint cannot close so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRefusal {
    /// It is not active.
    NotActive,
    /// The sprint chosen is not a future sprint of the board.
    BadTarget,
}

/// Whether the sprint in `state` of `board` may close, its unfinished
/// tickets going to `target`.
pub fn check_close<Id: Copy + Eq>(
    board: Id,
    state: State,
    target: CloseTarget<Id>,
) -> Result<(), CloseRefusal> {
    if state != State::Active {
        return Err(CloseRefusal::NotActive);
    }
    match target {
        CloseTarget::Sprint(sprint) if sprint.board != board || sprint.state != State::Future => {
            Err(CloseRefusal::BadTarget)
        }
        _ => Ok(()),
    }
}

/// A ticket in a sprint, as a start or a close sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stay<Id> {
    pub issue: Id,
    /// Its status is of the `done` category.
    pub done: bool,
    /// In the board's estimation.
    pub estimate: Option<f64>,
}

/// The count and estimate of a sprint's scope at a moment.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Snapshot {
    pub count: usize,
    /// The sum of the estimates given.
    pub estimate: f64,
}

/// The snapshot of `stays`: what a sprint commits to when it starts.
pub fn snapshot<Id>(stays: &[Stay<Id>]) -> Snapshot {
    Snapshot {
        count: stays.len(),
        estimate: stays.iter().filter_map(|s| s.estimate).sum(),
    }
}

/// What closing a sprint does with its tickets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Closing<Id> {
    /// The finished ones: they stay in the closed sprint.
    pub completed: Vec<Id>,
    /// The others: they go to the target.
    pub unfinished: Vec<Id>,
    /// The completed scope.
    pub snapshot: Snapshot,
}

/// Splits the tickets of a closing sprint between the completed and the
/// unfinished ones, by the category of their status.
pub fn close<Id: Copy>(stays: &[Stay<Id>]) -> Closing<Id> {
    let (done, rest): (Vec<&Stay<Id>>, Vec<&Stay<Id>>) = stays.iter().partition(|s| s.done);
    Closing {
        completed: done.iter().map(|s| s.issue).collect(),
        unfinished: rest.iter().map(|s| s.issue).collect(),
        snapshot: Snapshot {
            count: done.len(),
            estimate: done.iter().filter_map(|s| s.estimate).sum(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Option<Date> {
        Date::parse(text)
    }

    fn fields(name: &str, start: Option<&str>, end: Option<&str>) -> SprintFields {
        SprintFields {
            name: name.to_owned(),
            goal: None,
            start: start.and_then(date),
            end: end.and_then(date),
        }
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
    fn a_sprint_needs_a_name_and_both_dates_or_none() {
        assert!(validate(&fields("Sprint 25", None, None)).is_ok());
        assert!(validate(&fields("Sprint 25", Some("2026-11-16"), Some("2026-11-27"))).is_ok());
        assert!(validate(&fields("Sprint 25", Some("2026-11-16"), Some("2026-11-16"))).is_ok());
        assert_eq!(
            codes(validate(&fields(" ", None, None))),
            vec![("name".to_owned(), "required")]
        );
        assert_eq!(
            codes(validate(&fields(&"x".repeat(101), None, None))),
            vec![("name".to_owned(), "too_long")]
        );
        assert_eq!(
            codes(validate(&fields("S", Some("2026-11-27"), Some("2026-11-16")))),
            vec![("endDate".to_owned(), "before_start")]
        );
        assert_eq!(
            codes(validate(&fields("S", Some("2026-11-27"), None))),
            vec![("endDate".to_owned(), "required")]
        );
        assert_eq!(
            codes(validate(&fields("S", None, Some("2026-11-27")))),
            vec![("startDate".to_owned(), "required")]
        );
        let long_goal = SprintFields {
            goal: Some("g".repeat(1_001)),
            ..fields("S", None, None)
        };
        assert_eq!(codes(validate(&long_goal)), vec![("goal".to_owned(), "too_long")]);
    }

    #[test]
    fn default_names_follow_the_node_key() {
        assert_eq!(default_name("PLAT", 4), "PLAT Sprint 4");
        assert_eq!(default_name(" ", 1), "Sprint 1");
    }

    #[test]
    fn states_are_open_until_closed() {
        assert!(State::Future.is_open() && State::Active.is_open() && !State::Closed.is_open());
        for state in State::ALL {
            assert_eq!(State::parse(state.as_str()), Some(state));
        }
    }

    fn item(id: u32, sprint: Option<u32>, done: bool, estimate: Option<f64>) -> Item<u32> {
        Item {
            id,
            parent: None,
            subtask: false,
            done,
            estimate,
            sprint,
        }
    }

    #[test]
    fn plan_splits_sprints_and_backlog_and_counts_sub_tasks() {
        let items = vec![
            item(1, Some(100), false, Some(8.0)),
            item(2, None, false, Some(5.0)),
            item(3, Some(100), true, Some(3.0)),
            item(4, None, true, Some(2.0)),
            item(5, None, false, None),
            Item {
                parent: Some(2),
                subtask: true,
                ..item(6, None, false, Some(1.0))
            },
            Item {
                parent: Some(2),
                subtask: true,
                ..item(7, Some(100), false, None)
            },
            item(8, Some(200), false, Some(1.5)),
        ];
        let plan = plan(&items);
        assert_eq!(plan.sprints[&100], vec![1, 3]);
        assert_eq!(plan.sprints[&200], vec![8]);
        assert_eq!(
            plan.backlog,
            vec![2, 5],
            "done ticket 4 and sub-tasks are left out"
        );
        assert_eq!(plan.subtasks.get(&2), Some(&2));
        assert_eq!(
            plan.sprint_totals[&100],
            Totals {
                count: 2,
                estimate: 11.0,
                done: 3.0,
                unestimated: 0
            }
        );
        assert_eq!(
            plan.backlog_totals,
            Totals {
                count: 2,
                estimate: 5.0,
                done: 0.0,
                unestimated: 1
            }
        );
    }

    #[test]
    fn moves_stay_on_the_board_and_in_open_sprints() {
        let own = |state| SprintRef {
            id: 10,
            board: 1,
            state,
        };
        let foreign = SprintRef {
            id: 20,
            board: 2,
            state: State::Future,
        };
        assert_eq!(
            check_move(1, false, None, Target::Sprint(own(State::Future))),
            Ok(())
        );
        assert_eq!(
            check_move(1, false, None, Target::Sprint(own(State::Active))),
            Ok(())
        );
        assert_eq!(
            check_move(1, false, Some(own(State::Future)), Target::Backlog),
            Ok(())
        );
        assert_eq!(
            check_move(1, false, None, Target::Sprint(own(State::Closed))),
            Err(MoveRefusal::Closed)
        );
        assert_eq!(
            check_move(1, false, None, Target::Sprint(foreign)),
            Err(MoveRefusal::Foreign)
        );
        assert_eq!(
            check_move(1, false, Some(foreign), Target::Backlog),
            Err(MoveRefusal::Foreign)
        );
        assert_eq!(
            check_move(1, true, None, Target::Backlog),
            Err(MoveRefusal::Subtask)
        );
    }

    #[test]
    fn a_sprint_starts_once_and_alone_on_its_board() {
        assert_eq!(check_start(State::Future, false), Ok(()));
        assert_eq!(check_start(State::Future, true), Err(StartRefusal::ActiveExists));
        assert_eq!(check_start(State::Active, false), Err(StartRefusal::NotFuture));
        assert_eq!(check_start(State::Closed, true), Err(StartRefusal::NotFuture));
    }

    #[test]
    fn a_sprint_starts_with_its_dates() {
        assert!(validate_start(&fields("S", Some("2026-11-30"), Some("2026-12-11"))).is_ok());
        assert_eq!(
            codes(validate_start(&fields("S", None, None))),
            vec![
                ("startDate".to_owned(), "required"),
                ("endDate".to_owned(), "required")
            ]
        );
        assert_eq!(
            codes(validate_start(&fields("S", Some("2026-11-30"), None))),
            vec![("endDate".to_owned(), "required")]
        );
        assert_eq!(
            codes(validate_start(&fields(
                "",
                Some("2026-12-11"),
                Some("2026-11-30")
            ))),
            vec![
                ("name".to_owned(), "required"),
                ("endDate".to_owned(), "before_start")
            ]
        );
    }

    #[test]
    fn a_sprint_closes_when_active_towards_a_future_sprint_of_its_board() {
        let sprint = |board, state| SprintRef { id: 7, board, state };
        assert_eq!(check_close(1, State::Active, CloseTarget::Backlog), Ok(()));
        assert_eq!(check_close(1, State::Active, CloseTarget::NewSprint), Ok(()));
        assert_eq!(
            check_close(1, State::Active, CloseTarget::Sprint(sprint(1, State::Future))),
            Ok(())
        );
        assert_eq!(
            check_close(1, State::Future, CloseTarget::Backlog),
            Err(CloseRefusal::NotActive)
        );
        assert_eq!(
            check_close(1, State::Closed, CloseTarget::Backlog),
            Err(CloseRefusal::NotActive)
        );
        for target in [
            sprint(2, State::Future),
            sprint(1, State::Active),
            sprint(1, State::Closed),
        ] {
            assert_eq!(
                check_close(1, State::Active, CloseTarget::Sprint(target)),
                Err(CloseRefusal::BadTarget)
            );
        }
    }

    #[test]
    fn start_and_close_snapshot_the_committed_and_completed_scope() {
        let stay = |issue, done, estimate| Stay {
            issue,
            done,
            estimate,
        };
        let stays = vec![
            stay(1, true, Some(8.0)),
            stay(2, false, Some(5.0)),
            stay(3, true, None),
            stay(4, false, None),
        ];
        assert_eq!(
            snapshot(&stays),
            Snapshot {
                count: 4,
                estimate: 13.0
            }
        );
        assert_eq!(snapshot::<u32>(&[]), Snapshot::default());
        let closing = close(&stays);
        assert_eq!(closing.completed, vec![1, 3]);
        assert_eq!(closing.unfinished, vec![2, 4]);
        assert_eq!(
            closing.snapshot,
            Snapshot {
                count: 2,
                estimate: 8.0
            }
        );
        assert_eq!(close::<u32>(&[]), Closing::default());
    }
}
