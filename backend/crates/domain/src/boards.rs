//! Boards (specification §6): Kanban (L06-US05) and Scrum (L09-US01).
//!
//! A board belongs to a node of the hierarchy and shows the tickets of a
//! filter (a query of the language) in columns, each mapped to one or more
//! statuses by their stable key. A status sits in one column at most;
//! tickets whose status no column takes are left out of the board.
//!
//! - A column may cap its work in progress: reached at the limit, exceeded
//!   beyond it.
//! - Swimlanes group the cards by assignee or by epic, the cards without
//!   either last.
//! - Quick filters are clauses of the language the members toggle on the
//!   board, joined with `AND` to its filter.
//! - Dropping a card in a column runs the one transition that leads from
//!   its status to a status of that column ([`pick_transition`]).
//! - Cards follow the board's manual rank (`crate::rank`), the unranked ones
//!   last in the filter's order (`ORDER BY`; priority then last change
//!   without one).
//! - A Scrum board also plans its tickets in sprints (`crate::sprints`).

use std::collections::BTreeMap;

use crate::query::{Expr, Field, OrderKey, Query, Span, SystemField};
use crate::validation::{FieldError, Violations};
use crate::workflows::Category;

/// Longest board name, in characters.
pub const NAME_MAX_CHARS: usize = 100;
/// Most columns on a board.
pub const MAX_COLUMNS: usize = 12;
/// Longest column name, in characters.
pub const COLUMN_NAME_MAX_CHARS: usize = 40;
/// Most statuses mapped to one column.
pub const MAX_STATUSES_PER_COLUMN: usize = 30;
/// Highest work-in-progress limit.
pub const MAX_WIP_LIMIT: u32 = 999;
/// Most quick filters on a board.
pub const MAX_QUICK_FILTERS: usize = 10;
/// Longest quick filter label, in characters.
pub const QUICK_LABEL_MAX_CHARS: usize = 40;
/// Most cards a board shows: beyond, it tells the filter to narrow.
pub const MAX_CARDS: usize = 1_000;

/// What a board is for: the flow of its tickets (Kanban), or planning them
/// in sprints (Scrum). Set at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Kanban,
    Scrum,
}

impl Kind {
    pub const ALL: [Self; 2] = [Self::Kanban, Self::Scrum];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kanban => "kanban",
            Self::Scrum => "scrum",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

/// How the cards of a board are grouped in rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Swimlanes {
    #[default]
    None,
    Assignee,
    Epic,
}

impl Swimlanes {
    pub const ALL: [Self; 3] = [Self::None, Self::Assignee, Self::Epic];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Assignee => "assignee",
            Self::Epic => "epic",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// What the cards of a board are estimated in (specification §6): points
/// or hours. Workload always stays in hours, through assignments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Estimation {
    #[default]
    Points,
    Hours,
}

impl Estimation {
    pub const ALL: [Self; 2] = [Self::Points, Self::Hours];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Points => "points",
            Self::Hours => "hours",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }

    /// The key of the system field holding the estimate (`fields::SYSTEM_FIELDS`):
    /// story points, or the original estimate in hours (L09-US03). Workload
    /// never reads it: it stays in hours, through assignments.
    pub const fn field_key(self) -> &'static str {
        match self {
            Self::Points => "story_points",
            Self::Hours => "original_estimate",
        }
    }
}

/// The estimate of a list of cards, in the board's estimation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EstimateTotals {
    /// The sum of the estimates given.
    pub estimate: f64,
    /// The cards without one: counted apart, and flagged.
    pub unestimated: usize,
}

/// The totals of `estimates`, one per card (`None`: not estimated).
pub fn estimate_totals(estimates: impl IntoIterator<Item = Option<f64>>) -> EstimateTotals {
    estimates
        .into_iter()
        .fold(EstimateTotals::default(), |totals, estimate| match estimate {
            Some(estimate) => EstimateTotals {
                estimate: totals.estimate + estimate,
                ..totals
            },
            None => EstimateTotals {
                unestimated: totals.unestimated + 1,
                ..totals
            },
        })
}

/// A column, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnFields {
    pub name: String,
    /// Keys of the statuses it shows.
    pub statuses: Vec<String>,
    pub wip_limit: Option<u32>,
}

/// A quick filter, as written: a clause of the language without `ORDER BY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickFilterFields {
    pub label: String,
    pub query: String,
}

/// What a member writes of a board, besides its filter (checked by the
/// language, with its position, like a search).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardFields {
    pub name: String,
    pub columns: Vec<ColumnFields>,
    pub quick_filters: Vec<QuickFilterFields>,
}

fn check_length(v: &mut Violations, field: String, value: &str, max: usize) {
    let length = value.trim().chars().count();
    if length == 0 {
        v.add(field, "required");
    } else if length > max {
        v.add(field, "too_long");
    }
}

/// Checks `fields`: names, columns (a status in one column at most), work
/// in progress limits, and quick filters (each a query of the language
/// without `ORDER BY`).
pub fn validate(fields: &BoardFields) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    check_length(&mut v, "name".to_owned(), &fields.name, NAME_MAX_CHARS);
    if fields.columns.is_empty() {
        v.add("columns", "required");
    } else if fields.columns.len() > MAX_COLUMNS {
        v.add("columns", "too_many");
    }
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, column) in fields.columns.iter().enumerate() {
        check_length(
            &mut v,
            format!("columns.{i}.name"),
            &column.name,
            COLUMN_NAME_MAX_CHARS,
        );
        if column.statuses.len() > MAX_STATUSES_PER_COLUMN {
            v.add(format!("columns.{i}.statuses"), "too_many");
        }
        for status in &column.statuses {
            if !crate::issue_types::is_key(status) {
                v.add(format!("columns.{i}.statuses"), "format");
            } else if seen.insert(status, i).is_some_and(|other| other != i) {
                v.add(format!("columns.{i}.statuses"), "duplicate");
            }
        }
        if column
            .wip_limit
            .is_some_and(|limit| !(1..=MAX_WIP_LIMIT).contains(&limit))
        {
            v.add(format!("columns.{i}.wipLimit"), "out_of_range");
        }
    }
    if fields.quick_filters.len() > MAX_QUICK_FILTERS {
        v.add("quickFilters", "too_many");
    }
    for (i, quick) in fields.quick_filters.iter().enumerate() {
        check_length(
            &mut v,
            format!("quickFilters.{i}.label"),
            &quick.label,
            QUICK_LABEL_MAX_CHARS,
        );
        match crate::query::parse(&quick.query) {
            Err(_) => v.add(format!("quickFilters.{i}.query"), "invalid"),
            Ok(query) if !query.order_by.is_empty() => {
                v.add(format!("quickFilters.{i}.query"), "order_by_not_allowed");
            }
            Ok(query) if query.filter.is_none() => v.add(format!("quickFilters.{i}.query"), "required"),
            Ok(_) => {}
        }
    }
    v.into_result()
}

/// The column showing the status `key`, if any.
pub fn column_of(columns: &[ColumnFields], key: &str) -> Option<usize> {
    columns.iter().position(|c| c.statuses.iter().any(|s| s == key))
}

/// Where a column stands against its work in progress limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wip {
    /// No limit.
    Unlimited,
    Under,
    /// As many cards as the limit.
    Reached,
    /// More cards than the limit: highlighted.
    Exceeded,
}

impl Wip {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unlimited => "unlimited",
            Self::Under => "under",
            Self::Reached => "reached",
            Self::Exceeded => "exceeded",
        }
    }
}

/// How `count` cards stand against `limit`.
pub fn wip(count: usize, limit: Option<u32>) -> Wip {
    let Some(limit) = limit else {
        return Wip::Unlimited;
    };
    match count.cmp(&(limit as usize)) {
        std::cmp::Ordering::Less => Wip::Under,
        std::cmp::Ordering::Equal => Wip::Reached,
        std::cmp::Ordering::Greater => Wip::Exceeded,
    }
}

/// The query a board runs: its filter, `AND` the quick filters toggled on;
/// its `ORDER BY`, else priority (highest first) then last change.
pub fn effective_query(filter: &Query, quick: &[Query]) -> Query {
    let conditions = filter
        .filter
        .iter()
        .chain(quick.iter().filter_map(|q| q.filter.as_ref()))
        .cloned();
    let joined = conditions.reduce(|all, next| Expr::And(Box::new(all), Box::new(next)));
    let order_by = if filter.order_by.is_empty() {
        vec![
            OrderKey {
                field: Field::System(SystemField::Priority),
                descending: true,
                span: Span::default(),
            },
            OrderKey {
                field: Field::System(SystemField::Updated),
                descending: true,
                span: Span::default(),
            },
        ]
    } else {
        filter.order_by.clone()
    };
    Query {
        filter: joined,
        order_by,
    }
}

/// A status a new board's columns are drawn from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRef {
    pub key: String,
    pub category: Category,
}

/// The columns of a new board: one per category holding statuses (To do,
/// In progress, Done), each with the statuses of its category, named in
/// `language` (`fr` or `en`).
pub fn default_columns(statuses: &[StatusRef], language: &str) -> Vec<ColumnFields> {
    Category::ALL
        .into_iter()
        .filter_map(|category| {
            let keys: Vec<String> = statuses
                .iter()
                .filter(|s| s.category == category)
                .map(|s| s.key.clone())
                .collect();
            (!keys.is_empty()).then(|| ColumnFields {
                name: category_name(category, language).to_owned(),
                statuses: keys,
                wip_limit: None,
            })
        })
        .collect()
}

const fn category_name(category: Category, language: &str) -> &'static str {
    let french = matches!(language.as_bytes(), b"fr");
    match (category, french) {
        (Category::Todo, true) => "À faire",
        (Category::Todo, false) => "To do",
        (Category::InProgress, true) => "En cours",
        (Category::InProgress, false) => "In progress",
        (Category::Done, true) => "Terminé",
        (Category::Done, false) => "Done",
    }
}

/// The quick filters of a new board: the member's tickets, and bugs.
pub fn default_quick_filters(language: &str) -> Vec<QuickFilterFields> {
    let mine = if language == "fr" {
        "Mes tickets"
    } else {
        "My tickets"
    };
    vec![
        QuickFilterFields {
            label: mine.to_owned(),
            query: "assignee = currentUser()".to_owned(),
        },
        QuickFilterFields {
            label: "Bugs".to_owned(),
            query: "type = bug".to_owned(),
        },
    ]
}

/// A transition that leaves the status of a dropped card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Key of the status it leads to.
    pub to_status: String,
    /// The member meets its condition.
    pub allowed: bool,
}

/// What dropping a card in a column does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drop {
    /// Its status is already one of the column's: nothing to run.
    AlreadyThere,
    /// No transition leads from its status to the column.
    NoTransition,
    /// Only transitions whose condition the member does not meet.
    Restricted,
    /// This transition (an index of the candidates).
    Run(usize),
    /// Several lead there: the member picks one of these.
    Choose(Vec<usize>),
}

/// What dropping a card whose status is `current` in `column` runs, among
/// the transitions leaving its status.
pub fn pick_transition(current: &str, column: &ColumnFields, candidates: &[Candidate]) -> Drop {
    if column.statuses.iter().any(|s| s == current) {
        return Drop::AlreadyThere;
    }
    let leading: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| column.statuses.contains(&c.to_status))
        .map(|(i, _)| i)
        .collect();
    let allowed: Vec<usize> = leading
        .iter()
        .copied()
        .filter(|i| candidates[*i].allowed)
        .collect();
    match (leading.is_empty(), allowed.as_slice()) {
        (true, _) => Drop::NoTransition,
        (false, []) => Drop::Restricted,
        (false, [one]) => Drop::Run(*one),
        (false, several) => Drop::Choose(several.to_vec()),
    }
}

/// The swimlanes of cards, each card given by its lane key (`None`: no
/// assignee, no epic): lanes in key order, the one without a key last;
/// cards keep their order inside a lane.
pub fn lanes<K: Ord + Clone>(keys: &[Option<K>]) -> Vec<(Option<K>, Vec<usize>)> {
    let mut keyed: BTreeMap<K, Vec<usize>> = BTreeMap::new();
    let mut none = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        match key {
            Some(key) => keyed.entry(key.clone()).or_default().push(i),
            None => none.push(i),
        }
    }
    let mut lanes: Vec<(Option<K>, Vec<usize>)> =
        keyed.into_iter().map(|(k, cards)| (Some(k), cards)).collect();
    if !none.is_empty() {
        lanes.push((None, none));
    }
    lanes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, statuses: &[&str], wip_limit: Option<u32>) -> ColumnFields {
        ColumnFields {
            name: name.to_owned(),
            statuses: statuses.iter().map(|s| (*s).to_owned()).collect(),
            wip_limit,
        }
    }

    fn board(columns: Vec<ColumnFields>, quick: Vec<(&str, &str)>) -> BoardFields {
        BoardFields {
            name: "Platform".to_owned(),
            columns,
            quick_filters: quick
                .into_iter()
                .map(|(label, query)| QuickFilterFields {
                    label: label.to_owned(),
                    query: query.to_owned(),
                })
                .collect(),
        }
    }

    fn codes(fields: &BoardFields) -> Vec<(String, &'static str)> {
        validate(fields)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    fn plat() -> Vec<ColumnFields> {
        vec![
            column("To do", &["todo"], None),
            column("In progress", &["in_progress"], Some(4)),
            column("Review", &["review"], Some(3)),
            column("Done", &["done", "cancelled"], None),
        ]
    }

    #[test]
    fn a_complete_board_is_valid() {
        assert_eq!(
            codes(&board(plat(), vec![("Mine", "assignee = currentUser()")])),
            vec![]
        );
    }

    #[test]
    fn names_are_required_and_bounded() {
        let mut fields = board(vec![column(" ", &["todo"], None)], vec![(" ", "type = bug")]);
        fields.name = "  ".to_owned();
        assert_eq!(
            codes(&fields),
            vec![
                ("name".to_owned(), "required"),
                ("columns.0.name".to_owned(), "required"),
                ("quickFilters.0.label".to_owned(), "required"),
            ]
        );
        fields.name = "x".repeat(NAME_MAX_CHARS + 1);
        fields.columns[0].name = "é".repeat(COLUMN_NAME_MAX_CHARS);
        fields.quick_filters[0].label = "y".repeat(QUICK_LABEL_MAX_CHARS + 1);
        assert_eq!(
            codes(&fields),
            vec![
                ("name".to_owned(), "too_long"),
                ("quickFilters.0.label".to_owned(), "too_long"),
            ]
        );
    }

    #[test]
    fn a_board_has_one_to_twelve_columns() {
        assert_eq!(
            codes(&board(vec![], vec![])),
            vec![("columns".to_owned(), "required")]
        );
        let many = (0..=MAX_COLUMNS)
            .map(|i| column(&format!("C{i}"), &[], None))
            .collect();
        assert_eq!(
            codes(&board(many, vec![])),
            vec![("columns".to_owned(), "too_many")]
        );
    }

    #[test]
    fn a_status_sits_in_one_column_at_most() {
        let columns = vec![
            column("A", &["todo", "todo"], None),
            column("B", &["review", "todo"], None),
        ];
        assert_eq!(
            codes(&board(columns, vec![])),
            vec![("columns.1.statuses".to_owned(), "duplicate")]
        );
    }

    #[test]
    fn status_keys_and_limits_are_checked() {
        let columns = vec![
            column("A", &["In Progress"], Some(0)),
            column("B", &[], Some(MAX_WIP_LIMIT + 1)),
            column("C", &[], Some(MAX_WIP_LIMIT)),
        ];
        assert_eq!(
            codes(&board(columns, vec![])),
            vec![
                ("columns.0.statuses".to_owned(), "format"),
                ("columns.0.wipLimit".to_owned(), "out_of_range"),
                ("columns.1.wipLimit".to_owned(), "out_of_range"),
            ]
        );
        let crowded: Vec<String> = (0..=MAX_STATUSES_PER_COLUMN).map(|i| format!("s{i}")).collect();
        let crowded: Vec<&str> = crowded.iter().map(String::as_str).collect();
        assert_eq!(
            codes(&board(vec![column("A", &crowded, None)], vec![])),
            vec![("columns.0.statuses".to_owned(), "too_many")]
        );
    }

    #[test]
    fn quick_filters_are_clauses_of_the_language() {
        let quick = vec![
            ("Broken", "status = "),
            ("Sorted", "type = bug ORDER BY due"),
            ("Empty", "ORDER BY due"),
            ("Bugs", "type = bug"),
        ];
        assert_eq!(
            codes(&board(plat(), quick)),
            vec![
                ("quickFilters.0.query".to_owned(), "invalid"),
                ("quickFilters.1.query".to_owned(), "order_by_not_allowed"),
                ("quickFilters.2.query".to_owned(), "order_by_not_allowed"),
            ]
        );
        assert_eq!(
            codes(&board(plat(), vec![("Nothing", "")])),
            vec![("quickFilters.0.query".to_owned(), "required")]
        );
        let many = (0..=MAX_QUICK_FILTERS).map(|_| ("Bugs", "type = bug")).collect();
        assert_eq!(
            codes(&board(plat(), many)),
            vec![("quickFilters".to_owned(), "too_many")]
        );
    }

    #[test]
    fn each_status_maps_to_its_column() {
        let columns = plat();
        assert_eq!(column_of(&columns, "todo"), Some(0));
        assert_eq!(column_of(&columns, "review"), Some(2));
        assert_eq!(column_of(&columns, "cancelled"), Some(3));
        assert_eq!(column_of(&columns, "blocked"), None);
    }

    #[test]
    fn a_column_over_its_limit_is_exceeded() {
        assert_eq!(wip(4, Some(3)), Wip::Exceeded);
        assert_eq!(wip(4, Some(4)), Wip::Reached);
        assert_eq!(wip(2, Some(3)), Wip::Under);
        assert_eq!(wip(0, Some(1)), Wip::Under);
        assert_eq!(wip(500, None), Wip::Unlimited);
    }

    #[test]
    fn quick_filters_join_the_board_filter() {
        let filter = crate::query::parse("project = PLAT ORDER BY due").unwrap();
        let quick = [
            crate::query::parse("assignee = currentUser()").unwrap(),
            crate::query::parse("type = bug OR type = task").unwrap(),
        ];
        assert_eq!(
            effective_query(&filter, &quick).to_string(),
            "project = PLAT AND assignee = currentUser() AND (type = bug OR type = task) ORDER BY due ASC"
        );
        assert_eq!(
            effective_query(&filter, &[]).to_string(),
            "project = PLAT ORDER BY due ASC"
        );
    }

    #[test]
    fn without_order_by_cards_follow_priority_then_last_change() {
        let filter = crate::query::parse("").unwrap();
        assert_eq!(
            effective_query(&filter, &[]).to_string(),
            "ORDER BY priority DESC, updated DESC"
        );
        let quick = [crate::query::parse("type = bug").unwrap()];
        assert_eq!(
            effective_query(&filter, &quick).to_string(),
            "type = bug ORDER BY priority DESC, updated DESC"
        );
    }

    #[test]
    fn the_effective_query_runs_again_once_printed() {
        let filter = crate::query::parse("project = PLAT OR project = OPS").unwrap();
        let quick = [crate::query::parse("NOT type = bug").unwrap()];
        let query = effective_query(&filter, &quick);
        assert_eq!(
            crate::query::parse(&query.to_string()).unwrap().to_string(),
            query.to_string()
        );
        assert_eq!(
            query.to_string(),
            "(project = PLAT OR project = OPS) AND NOT type = bug ORDER BY priority DESC, updated DESC"
        );
    }

    #[test]
    fn a_new_board_has_a_column_per_category_in_use() {
        let statuses = [
            StatusRef {
                key: "todo".to_owned(),
                category: Category::Todo,
            },
            StatusRef {
                key: "done".to_owned(),
                category: Category::Done,
            },
            StatusRef {
                key: "backlog".to_owned(),
                category: Category::Todo,
            },
        ];
        assert_eq!(
            default_columns(&statuses, "fr"),
            vec![
                column("À faire", &["todo", "backlog"], None),
                column("Terminé", &["done"], None),
            ]
        );
        assert_eq!(
            default_columns(&statuses, "en")
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["To do", "Done"]
        );
        assert_eq!(default_columns(&[], "en"), vec![]);
    }

    #[test]
    fn default_quick_filters_are_valid() {
        for language in ["fr", "en"] {
            let quick = default_quick_filters(language);
            assert_eq!(
                codes(&board(
                    plat(),
                    quick
                        .iter()
                        .map(|q| (q.label.as_str(), q.query.as_str()))
                        .collect()
                )),
                vec![]
            );
        }
        assert_eq!(default_quick_filters("fr")[0].label, "Mes tickets");
        assert_eq!(default_quick_filters("en")[0].label, "My tickets");
    }

    fn candidate(to: &str, allowed: bool) -> Candidate {
        Candidate {
            to_status: to.to_owned(),
            allowed,
        }
    }

    #[test]
    fn a_drop_runs_the_one_transition_leading_to_the_column() {
        let review = column("Review", &["review"], Some(3));
        let candidates = [candidate("done", true), candidate("review", true)];
        assert_eq!(pick_transition("in_progress", &review, &candidates), Drop::Run(1));
    }

    #[test]
    fn a_drop_without_transition_is_refused() {
        let done = column("Done", &["done"], None);
        assert_eq!(
            pick_transition("todo", &done, &[candidate("in_progress", true)]),
            Drop::NoTransition
        );
        assert_eq!(pick_transition("todo", &done, &[]), Drop::NoTransition);
    }

    #[test]
    fn a_drop_on_a_restricted_transition_is_refused() {
        let done = column("Done", &["done"], None);
        assert_eq!(
            pick_transition("review", &done, &[candidate("done", false)]),
            Drop::Restricted
        );
        // An allowed one wins over a restricted one.
        assert_eq!(
            pick_transition(
                "review",
                &done,
                &[candidate("done", false), candidate("done", true)]
            ),
            Drop::Run(1)
        );
    }

    #[test]
    fn several_transitions_leave_the_choice_to_the_member() {
        let done = column("Done", &["done", "cancelled"], None);
        let candidates = [
            candidate("done", true),
            candidate("todo", true),
            candidate("cancelled", true),
            candidate("cancelled", false),
        ];
        assert_eq!(
            pick_transition("review", &done, &candidates),
            Drop::Choose(vec![0, 2])
        );
    }

    #[test]
    fn a_drop_in_its_own_column_does_nothing() {
        let done = column("Done", &["done", "cancelled"], None);
        assert_eq!(
            pick_transition("cancelled", &done, &[candidate("done", true)]),
            Drop::AlreadyThere
        );
    }

    #[test]
    fn swimlanes_follow_their_key_without_one_last() {
        let keys = [
            Some(("nathan", 2)),
            None,
            Some(("camille", 1)),
            Some(("nathan", 2)),
            None,
        ];
        assert_eq!(
            lanes(&keys),
            vec![
                (Some(("camille", 1)), vec![2]),
                (Some(("nathan", 2)), vec![0, 3]),
                (None, vec![1, 4]),
            ]
        );
        let all_keyed = [Some(1), Some(1)];
        assert_eq!(lanes(&all_keyed), vec![(Some(1), vec![0, 1])]);
        assert_eq!(lanes::<u8>(&[]), vec![]);
    }

    #[test]
    fn identifiers_round_trip() {
        for s in Swimlanes::ALL {
            assert_eq!(Swimlanes::parse(s.as_str()), Some(s));
        }
        for e in Estimation::ALL {
            assert_eq!(Estimation::parse(e.as_str()), Some(e));
        }
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::parse("safe"), None);
        assert_eq!(Swimlanes::parse("sprint"), None);
        assert_eq!(Estimation::parse("days"), None);
        assert_eq!(Estimation::Points.field_key(), "story_points");
        assert_eq!(Estimation::Hours.field_key(), "original_estimate");
        for estimation in Estimation::ALL {
            assert!(
                crate::fields::is_system_key(estimation.field_key()),
                "{estimation:?}"
            );
        }
        assert_eq!(
            [Wip::Unlimited, Wip::Under, Wip::Reached, Wip::Exceeded].map(Wip::as_str),
            ["unlimited", "under", "reached", "exceeded"]
        );
    }

    #[test]
    fn totals_sum_the_estimates_given_and_count_the_others() {
        assert_eq!(estimate_totals([]), EstimateTotals::default());
        assert!(estimate_totals([None]).estimate.is_sign_positive());
        // Points: decimals kept; hours: the same sum, read in hours.
        assert_eq!(
            estimate_totals([Some(8.0), Some(0.5), None, Some(3.0), None]),
            EstimateTotals {
                estimate: 11.5,
                unestimated: 2
            }
        );
        assert_eq!(
            estimate_totals([Some(7.5), Some(16.0)]),
            EstimateTotals {
                estimate: 23.5,
                unestimated: 0
            }
        );
    }
}
