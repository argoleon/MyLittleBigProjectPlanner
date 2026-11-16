//! What each field of the language accepts: operators, values, sorting.
//!
//! Custom fields are checked against their kind when the query runs (the
//! configurations know it); here they take any operator and any value but
//! the hierarchy and sprint functions.

use super::{
    Clause, CompareOp, Condition, Expr, Field, Function, OrderKey, Query, QueryError, QueryErrorKind,
    SystemField, Value,
};
use crate::issues::Priority;

/// Checks every condition and sort key of `query`; the first problem, with
/// its span.
pub fn check(query: &Query) -> Result<(), QueryError> {
    if let Some(filter) = &query.filter {
        check_expr(filter)?;
    }
    query.order_by.iter().try_for_each(check_order)
}

fn check_expr(expr: &Expr) -> Result<(), QueryError> {
    match expr {
        Expr::And(left, right) | Expr::Or(left, right) => {
            check_expr(left)?;
            check_expr(right)
        }
        Expr::Not(inner) => check_expr(inner),
        Expr::Clause(clause) => check_clause(clause),
    }
}

/// The values a field takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Values {
    /// Stable identifiers or names: words and strings.
    Identifiers,
    /// One of the five priorities.
    Priorities,
    /// Email addresses, or `currentUser()`.
    People,
    /// Dates (`2026-11-20`, `2026-11-20 09:30`), durations from now, and
    /// `now()`, `startOf…()`, `endOf…()`.
    Instants,
    /// Project keys, or `descendantsOf(…)` in a list.
    Projects,
    /// Sprint names or ids, or `openSprints()`.
    Sprints,
    /// Version names (`"2.1"`, `2.1`).
    Versions,
    /// Text only (`~`).
    Text,
    /// Anything but the hierarchy and sprint functions: the kind of a
    /// custom field decides when the query runs.
    Any,
}

/// What a field takes.
struct Rule {
    compare: &'static [CompareOp],
    list: bool,
    contains: bool,
    empty: bool,
    values: Values,
    sortable: bool,
}

const EQUALITY: &[CompareOp] = &[CompareOp::Eq, CompareOp::NotEq];
const ALL: &[CompareOp] = &CompareOp::ALL;
const NONE: &[CompareOp] = &[];

const fn rule(field: SystemField) -> Rule {
    let identifiers = Rule {
        compare: EQUALITY,
        list: true,
        contains: false,
        empty: false,
        values: Values::Identifiers,
        sortable: true,
    };
    let instants = Rule {
        compare: ALL,
        list: false,
        contains: false,
        empty: false,
        values: Values::Instants,
        sortable: true,
    };
    let text = Rule {
        compare: NONE,
        list: false,
        contains: true,
        empty: false,
        values: Values::Text,
        sortable: false,
    };
    match field {
        SystemField::Key | SystemField::Type | SystemField::Status => identifiers,
        SystemField::Parent => Rule {
            empty: true,
            ..identifiers
        },
        SystemField::Project => Rule {
            values: Values::Projects,
            ..identifiers
        },
        SystemField::Priority => Rule {
            compare: ALL,
            values: Values::Priorities,
            ..identifiers
        },
        SystemField::Assignee | SystemField::Reporter => Rule {
            empty: true,
            values: Values::People,
            ..identifiers
        },
        SystemField::Sprint => Rule {
            empty: true,
            values: Values::Sprints,
            ..identifiers
        },
        SystemField::FixVersion => Rule {
            empty: true,
            values: Values::Versions,
            ..identifiers
        },
        SystemField::Summary => Rule {
            sortable: true,
            ..text
        },
        SystemField::Description => Rule { empty: true, ..text },
        SystemField::Text => text,
        SystemField::Created | SystemField::Updated => instants,
        SystemField::Due | SystemField::Start => Rule {
            empty: true,
            ..instants
        },
    }
}

const CUSTOM: Rule = Rule {
    compare: ALL,
    list: true,
    contains: true,
    empty: true,
    values: Values::Any,
    sortable: true,
};

fn rule_of(field: &Field) -> Rule {
    match field {
        Field::System(field) => rule(*field),
        Field::Custom(_) => CUSTOM,
    }
}

/// The operators `rule` allows, as written.
fn allowed(rule: &Rule) -> Vec<String> {
    let mut operators: Vec<String> = rule.compare.iter().map(|op| op.as_str().to_owned()).collect();
    if rule.list {
        operators.extend(["IN".to_owned(), "NOT IN".to_owned()]);
    }
    if rule.contains {
        operators.extend(["~".to_owned(), "!~".to_owned()]);
    }
    if rule.empty {
        operators.extend(["IS EMPTY".to_owned(), "IS NOT EMPTY".to_owned()]);
    }
    operators
}

fn check_clause(clause: &Clause) -> Result<(), QueryError> {
    let rule = rule_of(&clause.field);
    let field = clause.field.name().to_owned();
    let refuse = |kind| Err(QueryError::new(clause.span, kind));
    let allowed_here = match &clause.condition {
        Condition::Compare(op, _) => rule.compare.contains(op),
        Condition::In { .. } => rule.list,
        Condition::Contains { .. } => rule.contains,
        Condition::Empty { .. } => rule.empty,
    };
    if !allowed_here {
        return refuse(QueryErrorKind::OperatorNotAllowed {
            field,
            operator: clause.condition.operator().to_owned(),
            allowed: allowed(&rule),
        });
    }
    let in_list = matches!(clause.condition, Condition::In { .. });
    let values: &[Value] = match &clause.condition {
        Condition::Compare(_, value) => std::slice::from_ref(value),
        Condition::In { values, .. } => values,
        Condition::Contains { .. } | Condition::Empty { .. } => &[],
    };
    for value in values {
        if let Err(kind) = check_value(rule.values, value, in_list, &field) {
            return refuse(kind);
        }
    }
    Ok(())
}

/// Whether a field taking `values` takes `value`.
fn check_value(values: Values, value: &Value, in_list: bool, field: &str) -> Result<(), QueryErrorKind> {
    let not_allowed = || QueryErrorKind::ValueNotAllowed {
        field: field.to_owned(),
        value: value.to_string(),
    };
    match (values, value) {
        (Values::Priorities, Value::Text(text)) => {
            return Priority::parse(&text.to_ascii_lowercase())
                .map(|_| ())
                .ok_or_else(|| QueryErrorKind::UnknownPriority(text.clone()));
        }
        (Values::Instants, Value::Text(text)) if !super::is_date_literal(text) => {
            return Err(QueryErrorKind::InvalidDate(text.clone()));
        }
        _ => {}
    }
    let hierarchy_or_sprints =
        |function: &Function| matches!(function, Function::DescendantsOf(_) | Function::OpenSprints);
    let ok = match value {
        Value::Function(function) => match values {
            Values::Instants => function.is_instant(),
            Values::People => *function == Function::CurrentUser,
            Values::Projects => in_list && matches!(function, Function::DescendantsOf(_)),
            Values::Sprints => in_list && *function == Function::OpenSprints,
            Values::Any => !hierarchy_or_sprints(function),
            Values::Identifiers | Values::Priorities | Values::Versions | Values::Text => false,
        },
        Value::Text(_) => !matches!(values, Values::Text),
        Value::Number(_) => matches!(values, Values::Sprints | Values::Versions | Values::Any),
        Value::Relative(_) => matches!(values, Values::Instants | Values::Any),
    };
    if ok { Ok(()) } else { Err(not_allowed()) }
}

fn check_order(key: &OrderKey) -> Result<(), QueryError> {
    if rule_of(&key.field).sortable {
        Ok(())
    } else {
        Err(QueryError::new(
            key.span,
            QueryErrorKind::NotSortable(key.field.name().to_owned()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Span, parse};
    use super::*;

    fn refusal(query: &str) -> QueryErrorKind {
        parse(query).unwrap_err().kind
    }

    #[test]
    fn the_queries_of_the_specification_pass() {
        for query in [
            "assignee = currentUser() AND status IN (todo, in_progress) ORDER BY priority DESC",
            "project IN descendantsOf(\"PORTF-HR\")",
            "created > -7d AND due < endOfWeek()",
            "priority = high",
            "points > 3",
            "status IN (todo, in_progress)",
            "assignee IS EMPTY",
            "text ~ \"outage\"",
            "sprint IN openSprints()",
            "fixVersion = \"2.1\"",
            "fixVersion = 2.1",
            "ORDER BY priority DESC, due ASC",
            "due <= \"2026-11-20\" AND updated >= \"2026-11-20 09:30\"",
            "priority > Medium AND reporter IN (\"lucas@acme.example\", currentUser())",
            "parent = PLAT-12 OR parent IS EMPTY",
            "labels ~ urgent AND points IS NOT EMPTY AND points IN (1, 2, 3)",
            "description IS EMPTY ORDER BY summary, key DESC",
        ] {
            assert!(parse(query).is_ok(), "{query}: {:?}", parse(query));
        }
    }

    #[test]
    fn a_field_refuses_the_operators_it_has_no_meaning_for() {
        assert_eq!(
            refusal("text = \"outage\""),
            QueryErrorKind::OperatorNotAllowed {
                field: "text".to_owned(),
                operator: "=".to_owned(),
                allowed: vec!["~".to_owned(), "!~".to_owned()],
            }
        );
        assert_eq!(
            refusal("status > todo"),
            QueryErrorKind::OperatorNotAllowed {
                field: "status".to_owned(),
                operator: ">".to_owned(),
                allowed: vec!["=", "!=", "IN", "NOT IN"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            }
        );
        for query in [
            "status IS EMPTY",
            "created IS EMPTY",
            "summary = x",
            "key ~ PLAT",
            "due IN (\"2026-11-20\")",
        ] {
            assert!(
                matches!(refusal(query), QueryErrorKind::OperatorNotAllowed { .. }),
                "{query}"
            );
        }
    }

    #[test]
    fn a_field_refuses_values_of_another_kind() {
        assert_eq!(
            refusal("due = currentUser()"),
            QueryErrorKind::ValueNotAllowed {
                field: "due".to_owned(),
                value: "currentUser()".to_owned()
            }
        );
        assert_eq!(
            refusal("due < tomorrow"),
            QueryErrorKind::InvalidDate("tomorrow".to_owned())
        );
        assert_eq!(
            refusal("due < \"2026-02-30\""),
            QueryErrorKind::InvalidDate("2026-02-30".to_owned())
        );
        assert_eq!(
            refusal("priority = urgent"),
            QueryErrorKind::UnknownPriority("urgent".to_owned())
        );
        for query in [
            "assignee = -7d",
            "status = now()",
            "project = descendantsOf(\"PORTF-HR\")",
            "status IN descendantsOf(\"PORTF-HR\")",
            "points IN descendantsOf(\"PORTF-HR\")",
            "sprint = openSprints()",
            "key = 12",
            "created > 3",
        ] {
            assert!(
                matches!(refusal(query), QueryErrorKind::ValueNotAllowed { .. }),
                "{query}: {:?}",
                parse(query)
            );
        }
    }

    #[test]
    fn errors_point_at_the_condition_or_sort_key() {
        let error = parse("status = todo AND text = x").unwrap_err();
        assert_eq!(error.span, Span::new(18, 8));
        let error = parse("status = todo ORDER BY text").unwrap_err();
        assert_eq!(
            (error.span, error.kind),
            (Span::new(23, 4), QueryErrorKind::NotSortable("text".to_owned()))
        );
        assert!(parse("ORDER BY description").is_err());
        assert!(parse("ORDER BY points DESC, summary").is_ok());
    }

    #[test]
    fn sprints_and_versions_wait_for_their_work_package() {
        assert!(SystemField::Sprint.is_pending() && SystemField::FixVersion.is_pending());
        assert!(SystemField::ALL.iter().filter(|f| f.is_pending()).count() == 2);
    }
}
