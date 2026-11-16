//! The canonical text of a query: what the visual builder shows and saved
//! filters store. `parse_syntax(&query.to_string())` gives `query` back
//! (spans aside).

use std::fmt::{self, Display, Formatter, Write};

use super::lexer::{is_number, is_word_char};
use super::parser::is_keyword;
use super::{Clause, Condition, Expr, Field, Function, OrderKey, Query, Relative, Span, Value};

impl Query {
    /// The same query, every span reset: two queries of the same meaning
    /// compare equal whatever their spacing.
    pub fn without_spans(mut self) -> Self {
        fn reset(expr: &mut Expr) {
            match expr {
                Expr::And(left, right) | Expr::Or(left, right) => {
                    reset(left);
                    reset(right);
                }
                Expr::Not(inner) => reset(inner),
                Expr::Clause(clause) => clause.span = Span::default(),
            }
        }
        if let Some(filter) = &mut self.filter {
            reset(filter);
        }
        for key in &mut self.order_by {
            key.span = Span::default();
        }
        self
    }
}

/// `text` as a quoted string: `"…"`, with `\` before `"` and `\`.
fn quoted(f: &mut Formatter<'_>, text: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            f.write_char('\\')?;
        }
        f.write_char(c)?;
    }
    f.write_char('"')
}

/// Whether `text` reads back as the same word when left bare: word
/// characters only, not starting like a number or a duration, no keyword.
fn is_bare(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| is_word_char(first) && !first.is_ascii_digit() && first != '-')
        && text.chars().all(is_word_char)
        && !is_keyword(text)
        && !is_number(text)
}

fn word_or_quoted(f: &mut Formatter<'_>, text: &str) -> fmt::Result {
    if is_bare(text) {
        f.write_str(text)
    } else {
        quoted(f, text)
    }
}

impl Display for Field {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        word_or_quoted(f, self.name())
    }
}

impl Display for Relative {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.unit.letter())
    }
}

impl Display for Function {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())?;
        f.write_char('(')?;
        if let Self::DescendantsOf(key) = self {
            quoted(f, key)?;
        }
        f.write_char(')')
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => word_or_quoted(f, text),
            Self::Number(number) => f.write_str(number),
            Self::Relative(relative) => relative.fmt(f),
            Self::Function(function) => function.fmt(f),
        }
    }
}

impl Display for Clause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.field, self.condition.operator())?;
        match &self.condition {
            Condition::Compare(_, value) => write!(f, " {value}"),
            Condition::In { values, .. } => match values.as_slice() {
                [Value::Function(function)] => write!(f, " {function}"),
                _ => {
                    f.write_str(" (")?;
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            f.write_str(", ")?;
                        }
                        value.fmt(f)?;
                    }
                    f.write_char(')')
                }
            },
            Condition::Contains { text, .. } => {
                f.write_char(' ')?;
                quoted(f, text)
            }
            Condition::Empty { .. } => Ok(()),
        }
    }
}

/// How tightly an expression binds: `OR` loosest, a condition tightest.
const fn binding(expr: &Expr) -> u8 {
    match expr {
        Expr::Or(..) => 1,
        Expr::And(..) => 2,
        Expr::Not(_) => 3,
        Expr::Clause(_) => 4,
    }
}

/// `expr`, in parentheses when it binds looser than `at_least`.
fn operand(f: &mut Formatter<'_>, expr: &Expr, at_least: u8) -> fmt::Result {
    if binding(expr) < at_least {
        write!(f, "({expr})")
    } else {
        expr.fmt(f)
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Left associative: a right operand of the same level keeps its
        // parentheses (`a AND (b AND c)` is another tree than `a AND b AND c`).
        match self {
            Self::Or(left, right) => {
                operand(f, left, 1)?;
                f.write_str(" OR ")?;
                operand(f, right, 2)
            }
            Self::And(left, right) => {
                operand(f, left, 2)?;
                f.write_str(" AND ")?;
                operand(f, right, 3)
            }
            Self::Not(inner) => {
                f.write_str("NOT ")?;
                operand(f, inner, 3)
            }
            Self::Clause(clause) => clause.fmt(f),
        }
    }
}

impl Display for OrderKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            self.field,
            if self.descending { "DESC" } else { "ASC" }
        )
    }
}

impl Display for Query {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(filter) = &self.filter {
            filter.fmt(f)?;
        }
        if !self.order_by.is_empty() {
            if self.filter.is_some() {
                f.write_char(' ')?;
            }
            f.write_str("ORDER BY ")?;
            for (index, key) in self.order_by.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                key.fmt(f)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::super::{CompareOp, Period, SystemField, TimeUnit, parse_syntax};
    use super::*;

    fn canonical(query: &str) -> String {
        parse_syntax(query).unwrap().to_string()
    }

    #[test]
    fn prints_the_canonical_text() {
        assert_eq!(
            canonical(
                "assignee=currentuser() and status in (todo,\"in progress\") order by priority desc,due"
            ),
            "assignee = currentUser() AND status IN (todo, \"in progress\") ORDER BY priority DESC, due ASC"
        );
        assert_eq!(
            canonical("project in descendantsOf(PORTF-HR) AND NOT (a is null or b !~ 'x\"y')"),
            "project IN descendantsOf(\"PORTF-HR\") AND NOT (a IS EMPTY OR b !~ \"x\\\"y\")"
        );
        assert_eq!(canonical("created > +7d"), "created > 7d");
        assert_eq!(
            canonical("key = \"and\""),
            "key = \"and\"",
            "a keyword stays quoted"
        );
        assert_eq!(
            canonical("x = \"3\""),
            "x = \"3\"",
            "a text that looks like a number too"
        );
        assert_eq!(canonical("\"order\" = 2"), "\"order\" = 2");
        assert_eq!(
            canonical("a IS EMPTY AND (b IS EMPTY AND c IS EMPTY)"),
            "a IS EMPTY AND (b IS EMPTY AND c IS EMPTY)"
        );
        assert_eq!(
            canonical("(a IS EMPTY AND b IS EMPTY) AND c IS EMPTY"),
            "a IS EMPTY AND b IS EMPTY AND c IS EMPTY"
        );
        assert_eq!(canonical(""), "");
        assert_eq!(canonical("order by key"), "ORDER BY key ASC");
    }

    fn field() -> impl Strategy<Value = Field> {
        prop_oneof![
            proptest::sample::select(SystemField::ALL.to_vec()).prop_map(Field::System),
            "[a-z][a-z0-9_]{0,12}".prop_filter_map("not a system field nor a keyword", |key| {
                (!is_keyword(&key) && matches!(Field::parse(&key), Some(Field::Custom(_))))
                    .then_some(Field::Custom(key))
            }),
        ]
    }

    fn text() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-zA-Z_][a-zA-Z0-9_.@-]{0,10}",
            "\\PC{0,12}",
            Just(String::new()),
            Just("AND".to_owned()),
            Just("a \"quoted\" \\ text".to_owned()),
            Just("x'; DROP TABLE issue; --".to_owned()),
        ]
    }

    fn function() -> impl Strategy<Value = Function> {
        let period = proptest::sample::select(Period::ALL.to_vec());
        prop_oneof![
            Just(Function::CurrentUser),
            Just(Function::Now),
            Just(Function::OpenSprints),
            period.clone().prop_map(Function::StartOf),
            period.prop_map(Function::EndOf),
            text().prop_map(Function::DescendantsOf),
        ]
    }

    fn value() -> impl Strategy<Value = Value> {
        prop_oneof![
            text().prop_map(Value::Text),
            "-?[0-9]{1,6}(\\.[0-9]{1,3})?".prop_map(Value::Number),
            (-1000_i64..1000, proptest::sample::select(TimeUnit::ALL.to_vec()))
                .prop_map(|(amount, unit)| Value::Relative(Relative { amount, unit })),
            function().prop_map(Value::Function),
        ]
    }

    fn condition() -> impl Strategy<Value = Condition> {
        prop_oneof![
            (proptest::sample::select(CompareOp::ALL.to_vec()), value())
                .prop_map(|(op, v)| Condition::Compare(op, v)),
            (any::<bool>(), proptest::collection::vec(value(), 1..4))
                .prop_map(|(negated, values)| Condition::In { negated, values }),
            (any::<bool>(), text()).prop_map(|(negated, text)| Condition::Contains { negated, text }),
            any::<bool>().prop_map(|negated| Condition::Empty { negated }),
        ]
    }

    fn expr() -> impl Strategy<Value = Expr> {
        let leaf = (field(), condition()).prop_map(|(field, condition)| {
            Expr::Clause(Clause {
                field,
                condition,
                span: Span::default(),
            })
        });
        leaf.prop_recursive(5, 24, 2, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone()).prop_map(|(l, r)| Expr::And(Box::new(l), Box::new(r))),
                (inner.clone(), inner.clone()).prop_map(|(l, r)| Expr::Or(Box::new(l), Box::new(r))),
                inner.prop_map(|e| Expr::Not(Box::new(e))),
            ]
        })
    }

    fn query() -> impl Strategy<Value = Query> {
        let key = (field(), any::<bool>()).prop_map(|(field, descending)| OrderKey {
            field,
            descending,
            span: Span::default(),
        });
        (proptest::option::of(expr()), proptest::collection::vec(key, 0..3))
            .prop_map(|(filter, order_by)| Query { filter, order_by })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        #[test]
        fn printing_then_parsing_gives_the_same_tree(query in query()) {
            let text = query.to_string();
            let parsed = parse_syntax(&text).map(Query::without_spans);
            prop_assert_eq!(parsed, Ok(query), "{}", text);
        }

        #[test]
        fn any_text_parses_or_fails_with_a_span_inside_it(input in "\\PC{0,60}") {
            let length = input.chars().count();
            if let Err(error) = parse_syntax(&input) {
                prop_assert!(error.span.start + error.span.len <= length, "{:?} in {:?}", error, input);
            }
        }
    }
}
