//! The query language (specification §6, L06-US01): a JQL-like language
//! that search, filters, boards, the Gantt chart, dashboards, the API and
//! MCP share.
//!
//! ```text
//! assignee = currentUser() AND status IN (todo, in_progress)
//!     AND created > -7d ORDER BY priority DESC, due ASC
//! ```
//!
//! [`parse`] turns a query into a typed [`Query`], or a [`QueryError`] with
//! the character position where it went wrong and what was expected there.
//! Fields and values are stable identifiers (`in_progress`, `PLAT`), never
//! translated labels. Nothing here runs a query: `infra::search` compiles
//! the tree to parameterized SQL, so no value is ever spliced into SQL.
//!
//! The tree names the entity it filters through [`Field`]: tickets today;
//! other entity types (objects, after the MVP) add their own fields.
//!
//! [`Query`] prints back to its canonical text (`Display`): keywords in
//! capitals, functions in camelCase, values quoted when needed, and
//! `parse_syntax(q.to_string())` gives `q` back.

mod check;
mod lexer;
mod parser;
mod print;

pub use check::check;
pub use parser::parse_syntax;

/// Longest query, in characters.
pub const MAX_QUERY_CHARS: usize = 10_000;
/// Deepest nesting of parentheses and `NOT`.
pub const MAX_DEPTH: usize = 32;
/// Most conditions in one query.
pub const MAX_CLAUSES: usize = 200;
/// Most values in one `IN (…)` list.
pub const MAX_LIST_VALUES: usize = 500;
/// Most sort keys after `ORDER BY`.
pub const MAX_ORDER_KEYS: usize = 5;

/// A parsed and checked query.
pub fn parse(input: &str) -> Result<Query, QueryError> {
    let query = parse_syntax(input)?;
    check(&query)?;
    Ok(query)
}

/// Where something is in the query text: a character offset (not a byte
/// offset) from 0, and a length in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub len: usize,
}

impl Span {
    pub const fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    /// From the start of `self` to the end of `other`.
    pub const fn to(self, other: Self) -> Self {
        Self {
            start: self.start,
            len: (other.start + other.len).saturating_sub(self.start),
        }
    }
}

/// A query: an optional filter, then how to sort. An empty query selects
/// everything the member may see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub filter: Option<Expr>,
    pub order_by: Vec<OrderKey>,
}

/// A condition, or conditions joined by `AND`, `OR` and `NOT`. `NOT` binds
/// tighter than `AND`, which binds tighter than `OR`; both are left
/// associative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Clause(Clause),
}

/// One condition on one field: `status IN (todo, in_progress)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub field: Field,
    pub condition: Condition,
    /// The whole condition in the text, for the errors found once parsed.
    pub span: Span,
}

/// What a condition asks of its field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// `=`, `!=`, `<`, `<=`, `>`, `>=`.
    Compare(CompareOp, Value),
    /// `IN (a, b)`, `NOT IN (a, b)`, or `IN function()`.
    In { negated: bool, values: Vec<Value> },
    /// `~ "text"` (holds the text), `!~ "text"`.
    Contains { negated: bool, text: String },
    /// `IS EMPTY`, `IS NOT EMPTY` (`NULL` is read as `EMPTY`).
    Empty { negated: bool },
}

impl Condition {
    /// The operator, as written in the canonical text.
    pub const fn operator(&self) -> &'static str {
        match self {
            Self::Compare(op, _) => op.as_str(),
            Self::In { negated: false, .. } => "IN",
            Self::In { negated: true, .. } => "NOT IN",
            Self::Contains { negated: false, .. } => "~",
            Self::Contains { negated: true, .. } => "!~",
            Self::Empty { negated: false } => "IS EMPTY",
            Self::Empty { negated: true } => "IS NOT EMPTY",
        }
    }
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

impl CompareOp {
    pub const ALL: [Self; 6] = [Self::Eq, Self::NotEq, Self::Lt, Self::LtEq, Self::Gt, Self::GtEq];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::NotEq => "!=",
            Self::Lt => "<",
            Self::LtEq => "<=",
            Self::Gt => ">",
            Self::GtEq => ">=",
        }
    }

    /// `<`, `<=`, `>`, `>=`: they need an ordered field.
    pub const fn is_ordering(self) -> bool {
        !matches!(self, Self::Eq | Self::NotEq)
    }
}

/// A value compared with a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A word (`in_progress`, `PLAT-12`) or a quoted string: the same value.
    Text(String),
    /// A number as written (`3`, `-2.5`).
    Number(String),
    /// A duration from now: `-7d` (7 days ago), `2w` (in 2 weeks).
    Relative(Relative),
    Function(Function),
}

/// A duration from now, signed: negative in the past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relative {
    pub amount: i64,
    pub unit: TimeUnit,
}

/// Unit of a relative duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    /// `y`
    Years,
    /// `M` (capital: `m` is minutes)
    Months,
    /// `w`
    Weeks,
    /// `d`
    Days,
    /// `h`
    Hours,
    /// `m`
    Minutes,
}

impl TimeUnit {
    pub const ALL: [Self; 6] = [
        Self::Years,
        Self::Months,
        Self::Weeks,
        Self::Days,
        Self::Hours,
        Self::Minutes,
    ];

    pub const fn letter(self) -> char {
        match self {
            Self::Years => 'y',
            Self::Months => 'M',
            Self::Weeks => 'w',
            Self::Days => 'd',
            Self::Hours => 'h',
            Self::Minutes => 'm',
        }
    }

    pub fn from_letter(letter: char) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.letter() == letter)
    }
}

/// A function of the language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Function {
    /// The member running the query.
    CurrentUser,
    /// This instant.
    Now,
    /// Bounds of the current day, week (Monday to Sunday), month or year,
    /// in the member's time zone.
    StartOf(Period),
    EndOf(Period),
    /// The node of key `key` and every node below it.
    DescendantsOf(String),
    /// The sprints under way (Scrum, L09).
    OpenSprints,
}

/// A calendar period of `startOf…()` and `endOf…()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}

impl Period {
    pub const ALL: [Self; 4] = [Self::Day, Self::Week, Self::Month, Self::Year];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "Day",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::Year => "Year",
        }
    }
}

impl Function {
    /// Its name in the canonical text.
    pub fn name(&self) -> String {
        match self {
            Self::CurrentUser => "currentUser".to_owned(),
            Self::Now => "now".to_owned(),
            Self::StartOf(period) => format!("startOf{}", period.as_str()),
            Self::EndOf(period) => format!("endOf{}", period.as_str()),
            Self::DescendantsOf(_) => "descendantsOf".to_owned(),
            Self::OpenSprints => "openSprints".to_owned(),
        }
    }

    /// Whether it stands for an instant (`now()`, `endOfWeek()`…).
    pub const fn is_instant(&self) -> bool {
        matches!(self, Self::Now | Self::StartOf(_) | Self::EndOf(_))
    }
}

/// A field of a ticket: one of the language's own, or a field of the
/// project configurations, by key (`points`, `remaining`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    System(SystemField),
    Custom(String),
}

impl Field {
    /// Its name in the canonical text.
    pub fn name(&self) -> &str {
        match self {
            Self::System(field) => field.as_str(),
            Self::Custom(key) => key,
        }
    }

    /// The field named `name`, any case. A custom field key is a lowercase
    /// identifier (`remaining_work`): `None` for anything else.
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if let Some(field) = SystemField::ALL
            .into_iter()
            .find(|f| f.as_str().eq_ignore_ascii_case(&lower))
        {
            return Some(Self::System(field));
        }
        crate::issue_types::is_key(&lower).then_some(Self::Custom(lower))
    }
}

/// The fields every ticket has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemField {
    Key,
    Project,
    Type,
    Status,
    Priority,
    Assignee,
    Reporter,
    Summary,
    Description,
    /// Summary and description together (and comments, with full text).
    Text,
    Created,
    Updated,
    Due,
    Start,
    Parent,
    /// Scrum (L09).
    Sprint,
    /// Versions (L09).
    FixVersion,
}

impl SystemField {
    pub const ALL: [Self; 17] = [
        Self::Key,
        Self::Project,
        Self::Type,
        Self::Status,
        Self::Priority,
        Self::Assignee,
        Self::Reporter,
        Self::Summary,
        Self::Description,
        Self::Text,
        Self::Created,
        Self::Updated,
        Self::Due,
        Self::Start,
        Self::Parent,
        Self::Sprint,
        Self::FixVersion,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Project => "project",
            Self::Type => "type",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Assignee => "assignee",
            Self::Reporter => "reporter",
            Self::Summary => "summary",
            Self::Description => "description",
            Self::Text => "text",
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Due => "due",
            Self::Start => "start",
            Self::Parent => "parent",
            Self::Sprint => "sprint",
            Self::FixVersion => "fixVersion",
        }
    }

    /// Fields that come with a later work package: parsed and checked now,
    /// refused when run (`QueryErrorKind::Unsupported`) until then.
    pub const fn is_pending(self) -> bool {
        matches!(self, Self::Sprint | Self::FixVersion)
    }
}

/// One sort key: `priority DESC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderKey {
    pub field: Field,
    pub descending: bool,
    pub span: Span,
}

/// Why a query was refused, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError {
    pub span: Span,
    pub kind: QueryErrorKind,
}

impl QueryError {
    pub const fn new(span: Span, kind: QueryErrorKind) -> Self {
        Self { span, kind }
    }

    /// A stable code for the API (`query.<code>` in Problem Details).
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// What was expected at the position, for syntax errors.
    pub fn expected(&self) -> &[Expected] {
        match &self.kind {
            QueryErrorKind::Unexpected { expected, .. } => expected,
            _ => &[],
        }
    }
}

/// The kinds of refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryErrorKind {
    /// Something else came (`found`: the text, `None` at the end).
    Unexpected {
        found: Option<String>,
        expected: Vec<Expected>,
    },
    /// A quote opened and never closed.
    UnterminatedString,
    /// A character the language does not use.
    InvalidCharacter(char),
    /// A number or duration that cannot be read (`12x`, `99999999999999999999d`).
    InvalidNumber(String),
    UnknownField(String),
    UnknownFunction(String),
    /// A function called with the wrong arguments: how many it takes.
    WrongArguments {
        function: String,
        expected: usize,
    },
    /// The field does not take this operator; those it takes.
    OperatorNotAllowed {
        field: String,
        operator: String,
        allowed: Vec<String>,
    },
    /// The field does not take this value (`due = currentUser()`).
    ValueNotAllowed {
        field: String,
        value: String,
    },
    /// Not a date: `YYYY-MM-DD`, optionally followed by ` HH:MM`.
    InvalidDate(String),
    UnknownPriority(String),
    /// The field cannot sort (`text`).
    NotSortable(String),
    /// Comes with a later work package (`sprint`, `fixVersion`, L09).
    Unsupported(String),
    TooLong,
    TooDeep,
    TooManyClauses,
    TooManyValues,
    TooManyOrderKeys,
}

impl QueryErrorKind {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unexpected { .. } => "syntax",
            Self::UnterminatedString => "unterminated_string",
            Self::InvalidCharacter(_) => "invalid_character",
            Self::InvalidNumber(_) => "invalid_number",
            Self::UnknownField(_) => "unknown_field",
            Self::UnknownFunction(_) => "unknown_function",
            Self::WrongArguments { .. } => "wrong_arguments",
            Self::OperatorNotAllowed { .. } => "operator_not_allowed",
            Self::ValueNotAllowed { .. } => "value_not_allowed",
            Self::InvalidDate(_) => "invalid_date",
            Self::UnknownPriority(_) => "unknown_priority",
            Self::NotSortable(_) => "not_sortable",
            Self::Unsupported(_) => "unsupported",
            Self::TooLong => "too_long",
            Self::TooDeep => "too_deep",
            Self::TooManyClauses => "too_many_clauses",
            Self::TooManyValues => "too_many_values",
            Self::TooManyOrderKeys => "too_many_order_keys",
        }
    }
}

/// What a syntax error expected, in the words of the language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// A field name.
    Field,
    /// A value: a word, a quoted string, a number, a duration or a function.
    Value,
    /// A quoted string or a word (after `~`).
    Text,
    /// A keyword or a symbol, as written: `AND`, `)`, `ORDER BY`, `=`.
    Token(&'static str),
    /// The end of the query.
    End,
}

impl Expected {
    /// How it reads in an error message and in the API (`"a field"`, `"AND"`).
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Field => "a field",
            Self::Value => "a value",
            Self::Text => "a text",
            Self::Token(token) => token,
            Self::End => "the end of the query",
        }
    }
}

/// Whether `date` is a date (`YYYY-MM-DD`) or a date and time
/// (`YYYY-MM-DD HH:MM`) that exists.
pub fn is_date_literal(date: &str) -> bool {
    let (day, time) = match date.split_once(' ') {
        Some((day, time)) => (day, Some(time)),
        None => (date, None),
    };
    let digits = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_digit());
    let parts: Vec<&str> = day.split('-').collect();
    let [year, month, day_of_month] = parts.as_slice() else {
        return false;
    };
    if !(digits(year, 4) && digits(month, 2) && digits(day_of_month, 2)) {
        return false;
    }
    let (year, month, day_of_month): (u32, u32, u32) =
        match (year.parse(), month.parse(), day_of_month.parse()) {
            (Ok(y), Ok(m), Ok(d)) => (y, m, d),
            _ => return false,
        };
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if !(1..=days).contains(&day_of_month) {
        return false;
    }
    match time {
        None => true,
        Some(time) => match time.split_once(':') {
            Some((hours, minutes)) if digits(hours, 2) && digits(minutes, 2) => {
                hours.parse::<u32>().is_ok_and(|h| h < 24) && minutes.parse::<u32>().is_ok_and(|m| m < 60)
            }
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_checked_down_to_leap_years_and_minutes() {
        for good in [
            "2026-11-20",
            "2024-02-29",
            "2000-02-29",
            "2026-11-20 09:30",
            "2026-12-31 23:59",
        ] {
            assert!(is_date_literal(good), "{good}");
        }
        for bad in [
            "2026-11-31",
            "2026-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-1-01",
            "26-11-20",
            "2026-11-20 24:00",
            "2026-11-20 9:30",
            "2026-11-20T09:30",
            "",
            "today",
        ] {
            assert!(!is_date_literal(bad), "{bad}");
        }
    }

    #[test]
    fn fields_are_read_in_any_case_and_custom_keys_are_identifiers() {
        assert_eq!(Field::parse("Status"), Some(Field::System(SystemField::Status)));
        assert_eq!(
            Field::parse("FIXVERSION"),
            Some(Field::System(SystemField::FixVersion))
        );
        assert_eq!(
            Field::parse("Remaining_Work"),
            Some(Field::Custom("remaining_work".to_owned()))
        );
        assert_eq!(Field::parse("points"), Some(Field::Custom("points".to_owned())));
        for bad in ["2points", "my-field", "", "é"] {
            assert_eq!(Field::parse(bad), None, "{bad}");
        }
        for field in SystemField::ALL {
            assert_eq!(Field::parse(field.as_str()), Some(Field::System(field)));
        }
    }

    #[test]
    fn units_and_functions_have_one_spelling() {
        for unit in TimeUnit::ALL {
            assert_eq!(TimeUnit::from_letter(unit.letter()), Some(unit));
        }
        assert_eq!(TimeUnit::from_letter('s'), None);
        assert_eq!(Function::EndOf(Period::Week).name(), "endOfWeek");
        assert!(Function::StartOf(Period::Day).is_instant() && !Function::CurrentUser.is_instant());
    }

    #[test]
    fn spans_stretch_from_one_to_another() {
        assert_eq!(Span::new(3, 2).to(Span::new(10, 4)), Span::new(3, 11));
    }
}
