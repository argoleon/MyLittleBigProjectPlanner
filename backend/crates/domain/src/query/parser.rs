//! Recursive descent parser: the syntax of the language, without what each
//! field accepts (`check`).
//!
//! ```text
//! query     := [or] [ORDER BY order (, order)*] END
//! or        := and (OR and)*
//! and       := not (AND not)*
//! not       := NOT not | '(' or ')' | clause
//! clause    := field (cmp value | [NOT] IN operands | IS [NOT] (EMPTY | NULL) | (~ | !~) text)
//! operands  := '(' value (, value)* ')' | function
//! value     := word | string | number | duration | function
//! function  := word '(' [value (, value)*] ')'
//! order     := field [ASC | DESC]
//! ```
//!
//! Keywords are read in any case. A field may be quoted when its key is a
//! keyword (`"order" = 2`).

use super::lexer::{Token, TokenKind, tokenize};
use super::{
    Clause, CompareOp, Condition, Expected, Expr, Field, Function, MAX_CLAUSES, MAX_DEPTH, MAX_LIST_VALUES,
    MAX_ORDER_KEYS, MAX_QUERY_CHARS, OrderKey, Period, Query, QueryError, QueryErrorKind, Span, Value,
};

/// The keywords: never a bare field or value.
pub(super) const KEYWORDS: [&str; 11] = [
    "AND", "OR", "NOT", "IN", "IS", "EMPTY", "NULL", "ORDER", "BY", "ASC", "DESC",
];

/// Whether `word` is a keyword, in any case.
pub(super) fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// The syntax tree of `input`, before checking what each field accepts.
pub fn parse_syntax(input: &str) -> Result<Query, QueryError> {
    let length = input.chars().count();
    if length > MAX_QUERY_CHARS {
        return Err(QueryError::new(
            Span::new(MAX_QUERY_CHARS, length - MAX_QUERY_CHARS),
            QueryErrorKind::TooLong,
        ));
    }
    let mut parser = Parser {
        chars: input.chars().collect(),
        tokens: tokenize(input)?,
        position: 0,
        depth: 0,
        clauses: 0,
    };
    parser.query()
}

struct Parser {
    chars: Vec<char>,
    tokens: Vec<Token>,
    position: usize,
    depth: usize,
    clauses: usize,
}

const COMPARISONS: [Expected; 11] = [
    Expected::Token("="),
    Expected::Token("!="),
    Expected::Token("<"),
    Expected::Token("<="),
    Expected::Token(">"),
    Expected::Token(">="),
    Expected::Token("~"),
    Expected::Token("!~"),
    Expected::Token("IN"),
    Expected::Token("NOT IN"),
    Expected::Token("IS"),
];

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    /// The token after the next one.
    fn peek_second(&self) -> &TokenKind {
        self.tokens
            .get(self.position + 1)
            .map_or(&TokenKind::End, |token| &token.kind)
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.position].clone();
        if token.kind != TokenKind::End {
            self.position += 1;
        }
        token
    }

    /// The span of the last token taken.
    fn last_span(&self) -> Span {
        self.tokens[self.position.saturating_sub(1)].span
    }

    fn at_keyword(&self, keyword: &str) -> bool {
        matches!(self.peek_kind(), TokenKind::Word(word) if word.eq_ignore_ascii_case(keyword))
    }

    /// Takes `keyword` if it comes next.
    fn eat_keyword(&mut self, keyword: &str) -> bool {
        let found = self.at_keyword(keyword);
        if found {
            self.bump();
        }
        found
    }

    /// The next token, refused: what was found instead of `expected`.
    fn unexpected(&self, expected: Vec<Expected>) -> QueryError {
        let token = self.peek();
        let found = (token.kind != TokenKind::End).then(|| {
            self.chars[token.span.start..token.span.start + token.span.len]
                .iter()
                .collect()
        });
        QueryError::new(token.span, QueryErrorKind::Unexpected { found, expected })
    }

    fn expect_keyword(&mut self, keyword: &'static str) -> Result<Span, QueryError> {
        if self.at_keyword(keyword) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(vec![Expected::Token(keyword)]))
        }
    }

    fn expect_kind(&mut self, kind: &TokenKind, expected: Vec<Expected>) -> Result<Span, QueryError> {
        if self.peek_kind() == kind {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(expected))
        }
    }

    /// One level deeper (parentheses, `NOT`, function arguments).
    fn enter(&mut self) -> Result<(), QueryError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(QueryError::new(self.peek().span, QueryErrorKind::TooDeep));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn query(&mut self) -> Result<Query, QueryError> {
        let filter = if matches!(self.peek_kind(), TokenKind::End) || self.at_keyword("ORDER") {
            None
        } else {
            Some(self.or()?)
        };
        let mut order_by = Vec::new();
        if self.eat_keyword("ORDER") {
            self.expect_keyword("BY")?;
            loop {
                if order_by.len() == MAX_ORDER_KEYS {
                    return Err(QueryError::new(
                        self.peek().span,
                        QueryErrorKind::TooManyOrderKeys,
                    ));
                }
                order_by.push(self.order_key()?);
                if !matches!(self.peek_kind(), TokenKind::Comma) {
                    break;
                }
                self.bump();
            }
            if !matches!(self.peek_kind(), TokenKind::End) {
                return Err(self.unexpected(vec![
                    Expected::Token(","),
                    Expected::Token("ASC"),
                    Expected::Token("DESC"),
                    Expected::End,
                ]));
            }
        } else if !matches!(self.peek_kind(), TokenKind::End) {
            let mut expected = vec![Expected::Token("AND"), Expected::Token("OR")];
            if filter.is_none() {
                expected = vec![Expected::Field, Expected::Token("("), Expected::Token("NOT")];
            }
            expected.extend([Expected::Token("ORDER BY"), Expected::End]);
            return Err(self.unexpected(expected));
        }
        Ok(Query { filter, order_by })
    }

    fn or(&mut self) -> Result<Expr, QueryError> {
        let mut left = self.and()?;
        while self.eat_keyword("OR") {
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, QueryError> {
        let mut left = self.not()?;
        while self.eat_keyword("AND") {
            let right = self.not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, QueryError> {
        if self.at_keyword("NOT") {
            self.enter()?;
            self.bump();
            let negated = self.not()?;
            self.leave();
            return Ok(Expr::Not(Box::new(negated)));
        }
        if matches!(self.peek_kind(), TokenKind::LParen) {
            self.enter()?;
            self.bump();
            let inner = self.or()?;
            self.expect_kind(
                &TokenKind::RParen,
                vec![
                    Expected::Token("AND"),
                    Expected::Token("OR"),
                    Expected::Token(")"),
                ],
            )?;
            self.leave();
            return Ok(inner);
        }
        self.clause().map(Expr::Clause)
    }

    /// A field: a bare word that is not a keyword, or a quoted name.
    fn field(&mut self, expected: Vec<Expected>) -> Result<(Field, Span), QueryError> {
        let name = match self.peek_kind() {
            TokenKind::Word(word) if !is_keyword(word) => word.clone(),
            TokenKind::Str(name) => name.clone(),
            _ => return Err(self.unexpected(expected)),
        };
        let span = self.bump().span;
        Field::parse(&name)
            .map(|field| (field, span))
            .ok_or_else(|| QueryError::new(span, QueryErrorKind::UnknownField(name)))
    }

    fn clause(&mut self) -> Result<Clause, QueryError> {
        let (field, start) = self.field(vec![
            Expected::Field,
            Expected::Token("("),
            Expected::Token("NOT"),
        ])?;
        self.clauses += 1;
        if self.clauses > MAX_CLAUSES {
            return Err(QueryError::new(start, QueryErrorKind::TooManyClauses));
        }
        let condition = match self.peek_kind().clone() {
            TokenKind::Op(op @ ("~" | "!~")) => {
                self.bump();
                let text = match self.peek_kind() {
                    TokenKind::Str(text) | TokenKind::Number(text) => text.clone(),
                    TokenKind::Word(word) if !is_keyword(word) => word.clone(),
                    _ => return Err(self.unexpected(vec![Expected::Text])),
                };
                self.bump();
                Condition::Contains {
                    negated: op == "!~",
                    text,
                }
            }
            TokenKind::Op(op) => {
                self.bump();
                let compare = CompareOp::ALL
                    .into_iter()
                    .find(|c| c.as_str() == op)
                    .unwrap_or(CompareOp::Eq);
                Condition::Compare(compare, self.value()?)
            }
            TokenKind::Word(word) if word.eq_ignore_ascii_case("IN") => {
                self.bump();
                self.in_operands(false)?
            }
            TokenKind::Word(word) if word.eq_ignore_ascii_case("NOT") => {
                self.bump();
                self.expect_keyword("IN")?;
                self.in_operands(true)?
            }
            TokenKind::Word(word) if word.eq_ignore_ascii_case("IS") => {
                self.bump();
                let negated = self.eat_keyword("NOT");
                if !(self.eat_keyword("EMPTY") || self.eat_keyword("NULL")) {
                    let mut expected = vec![Expected::Token("EMPTY")];
                    if !negated {
                        expected.push(Expected::Token("NOT"));
                    }
                    return Err(self.unexpected(expected));
                }
                Condition::Empty { negated }
            }
            _ => return Err(self.unexpected(COMPARISONS.to_vec())),
        };
        Ok(Clause {
            field,
            condition,
            span: start.to(self.last_span()),
        })
    }

    /// After `IN`: a list in parentheses, or a function (`descendantsOf(…)`).
    fn in_operands(&mut self, negated: bool) -> Result<Condition, QueryError> {
        let values = if matches!(self.peek_kind(), TokenKind::LParen) {
            self.bump();
            let mut values = vec![self.value()?];
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                if values.len() == MAX_LIST_VALUES {
                    return Err(QueryError::new(self.peek().span, QueryErrorKind::TooManyValues));
                }
                values.push(self.value()?);
            }
            self.expect_kind(
                &TokenKind::RParen,
                vec![Expected::Token(","), Expected::Token(")")],
            )?;
            values
        } else if matches!(self.peek_kind(), TokenKind::Word(_))
            && matches!(self.peek_second(), TokenKind::LParen)
        {
            vec![self.function()?]
        } else {
            return Err(self.unexpected(vec![Expected::Token("(")]));
        };
        Ok(Condition::In { negated, values })
    }

    fn value(&mut self) -> Result<Value, QueryError> {
        let value = match self.peek_kind().clone() {
            TokenKind::Word(_) if matches!(self.peek_second(), TokenKind::LParen) => return self.function(),
            TokenKind::Word(word) if !is_keyword(&word) => Value::Text(word),
            TokenKind::Str(text) => Value::Text(text),
            TokenKind::Number(number) => Value::Number(number),
            TokenKind::Relative(relative) => Value::Relative(relative),
            _ => return Err(self.unexpected(vec![Expected::Value])),
        };
        self.bump();
        Ok(value)
    }

    /// A function call, its name first.
    fn function(&mut self) -> Result<Value, QueryError> {
        let name_token = self.bump();
        let TokenKind::Word(name) = name_token.kind else {
            return Err(QueryError::new(
                name_token.span,
                QueryErrorKind::UnknownFunction(String::new()),
            ));
        };
        let lower = name.to_ascii_lowercase();
        let period = |prefix: &str| {
            lower.strip_prefix(prefix).and_then(|rest| {
                Period::ALL
                    .into_iter()
                    .find(|p| p.as_str().eq_ignore_ascii_case(rest))
            })
        };
        let known: Option<(Function, usize)> = match lower.as_str() {
            "currentuser" => Some((Function::CurrentUser, 0)),
            "now" => Some((Function::Now, 0)),
            "descendantsof" => Some((Function::DescendantsOf(String::new()), 1)),
            "opensprints" => Some((Function::OpenSprints, 0)),
            _ => period("startof")
                .map(|p| (Function::StartOf(p), 0))
                .or_else(|| period("endof").map(|p| (Function::EndOf(p), 0))),
        };
        let Some((function, arity)) = known else {
            return Err(QueryError::new(
                name_token.span,
                QueryErrorKind::UnknownFunction(name),
            ));
        };
        self.enter()?;
        self.bump(); // (
        let mut arguments = Vec::new();
        if !matches!(self.peek_kind(), TokenKind::RParen) {
            arguments.push(self.value()?);
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                arguments.push(self.value()?);
            }
        }
        let end = self.expect_kind(
            &TokenKind::RParen,
            vec![Expected::Token(","), Expected::Token(")")],
        )?;
        self.leave();
        let wrong = || {
            QueryError::new(
                name_token.span.to(end),
                QueryErrorKind::WrongArguments {
                    function: function.name(),
                    expected: arity,
                },
            )
        };
        if arguments.len() != arity {
            return Err(wrong());
        }
        Ok(Value::Function(match function {
            Function::DescendantsOf(_) => match arguments.pop() {
                Some(Value::Text(key)) => Function::DescendantsOf(key),
                _ => return Err(wrong()),
            },
            other => other,
        }))
    }

    fn order_key(&mut self) -> Result<OrderKey, QueryError> {
        let (field, start) = self.field(vec![Expected::Field])?;
        let descending = if self.eat_keyword("DESC") {
            true
        } else {
            self.eat_keyword("ASC");
            false
        };
        Ok(OrderKey {
            field,
            descending,
            span: start.to(self.last_span()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Relative, SystemField, TimeUnit};
    use super::*;

    fn system(field: SystemField) -> Field {
        Field::System(field)
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_owned())
    }

    /// The tree without spans, to compare shapes.
    fn shape(query: &str) -> Query {
        parse_syntax(query).unwrap().without_spans()
    }

    fn clause(field: Field, condition: Condition) -> Expr {
        Expr::Clause(Clause {
            field,
            condition,
            span: Span::default(),
        })
    }

    fn error(query: &str) -> QueryError {
        parse_syntax(query).unwrap_err()
    }

    #[test]
    fn reads_the_example_of_the_acceptance_criteria() {
        let query =
            shape("assignee = currentUser() AND status IN (todo, in_progress) ORDER BY priority DESC");
        assert_eq!(
            query,
            Query {
                filter: Some(Expr::And(
                    Box::new(clause(
                        system(SystemField::Assignee),
                        Condition::Compare(CompareOp::Eq, Value::Function(Function::CurrentUser))
                    )),
                    Box::new(clause(
                        system(SystemField::Status),
                        Condition::In {
                            negated: false,
                            values: vec![text("todo"), text("in_progress")]
                        }
                    )),
                )),
                order_by: vec![OrderKey {
                    field: system(SystemField::Priority),
                    descending: true,
                    span: Span::default()
                }],
            }
        );
    }

    #[test]
    fn not_binds_tighter_than_and_which_binds_tighter_than_or() {
        let a = || clause(Field::Custom("a".to_owned()), Condition::Empty { negated: false });
        let b = || clause(Field::Custom("b".to_owned()), Condition::Empty { negated: false });
        let c = || clause(Field::Custom("c".to_owned()), Condition::Empty { negated: false });
        assert_eq!(
            shape("a IS EMPTY OR NOT b IS EMPTY AND c IS EMPTY").filter,
            Some(Expr::Or(
                Box::new(a()),
                Box::new(Expr::And(Box::new(Expr::Not(Box::new(b()))), Box::new(c())))
            ))
        );
        assert_eq!(
            shape("(a IS EMPTY OR b IS EMPTY) AND c IS EMPTY").filter,
            Some(Expr::And(
                Box::new(Expr::Or(Box::new(a()), Box::new(b()))),
                Box::new(c())
            ))
        );
        assert_eq!(
            shape("a is empty and b is empty and c is empty").filter,
            Some(Expr::And(
                Box::new(Expr::And(Box::new(a()), Box::new(b()))),
                Box::new(c())
            )),
            "left associative, keywords in any case"
        );
    }

    #[test]
    fn reads_every_operator_value_and_function() {
        let condition = |query: &str| match shape(query).filter {
            Some(Expr::Clause(clause)) => clause.condition,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            condition("created > -7d"),
            Condition::Compare(
                CompareOp::Gt,
                Value::Relative(Relative {
                    amount: -7,
                    unit: TimeUnit::Days
                })
            )
        );
        assert_eq!(
            condition("due < endOfWeek()"),
            Condition::Compare(CompareOp::Lt, Value::Function(Function::EndOf(Period::Week)))
        );
        assert_eq!(
            condition("due >= STARTOFMONTH()"),
            Condition::Compare(CompareOp::GtEq, Value::Function(Function::StartOf(Period::Month)))
        );
        assert_eq!(
            condition("updated <= now()"),
            Condition::Compare(CompareOp::LtEq, Value::Function(Function::Now))
        );
        assert_eq!(
            condition("points != 2.5"),
            Condition::Compare(CompareOp::NotEq, Value::Number("2.5".to_owned()))
        );
        assert_eq!(
            condition("project IN descendantsOf(\"PORTF-HR\")"),
            Condition::In {
                negated: false,
                values: vec![Value::Function(Function::DescendantsOf("PORTF-HR".to_owned()))]
            }
        );
        assert_eq!(
            condition("sprint in openSprints()"),
            Condition::In {
                negated: false,
                values: vec![Value::Function(Function::OpenSprints)]
            }
        );
        assert_eq!(
            condition("status NOT IN (done)"),
            Condition::In {
                negated: true,
                values: vec![text("done")]
            }
        );
        assert_eq!(
            condition("text ~ \"outage\""),
            Condition::Contains {
                negated: false,
                text: "outage".to_owned()
            }
        );
        assert_eq!(
            condition("summary !~ 404"),
            Condition::Contains {
                negated: true,
                text: "404".to_owned()
            }
        );
        assert_eq!(condition("assignee IS NULL"), Condition::Empty { negated: false });
        assert_eq!(
            condition("assignee is not empty"),
            Condition::Empty { negated: true }
        );
        assert_eq!(
            condition("\"order\" = 2"),
            Condition::Compare(CompareOp::Eq, Value::Number("2".to_owned()))
        );
    }

    #[test]
    fn an_empty_query_or_a_sort_alone_is_fine() {
        assert_eq!(shape(""), Query::default());
        assert_eq!(shape("   "), Query::default());
        let sorted = shape("ORDER BY due, created DESC");
        assert_eq!(sorted.filter, None);
        assert_eq!(
            sorted
                .order_by
                .iter()
                .map(|k| (k.field.name(), k.descending))
                .collect::<Vec<_>>(),
            [("due", false), ("created", true)]
        );
    }

    #[test]
    fn a_sql_fragment_in_a_string_stays_a_value() {
        assert_eq!(
            shape("summary ~ \"x'; DROP TABLE issue; --\"").filter,
            Some(clause(
                system(SystemField::Summary),
                Condition::Contains {
                    negated: false,
                    text: "x'; DROP TABLE issue; --".to_owned()
                }
            ))
        );
    }

    #[test]
    fn spans_cover_each_condition_and_sort_key() {
        let query = parse_syntax("status = todo AND assignee IS NOT EMPTY ORDER BY due DESC").unwrap();
        let Some(Expr::And(left, right)) = query.filter else {
            panic!("an AND");
        };
        let span = |expr: &Expr| match expr {
            Expr::Clause(clause) => clause.span,
            other => panic!("{other:?}"),
        };
        assert_eq!((span(&left), span(&right)), (Span::new(0, 13), Span::new(18, 21)));
        assert_eq!(query.order_by[0].span, Span::new(49, 8));
    }

    #[test]
    fn syntax_errors_say_where_and_what_was_expected() {
        let unexpected = |found: Option<&str>, expected: Vec<Expected>| QueryErrorKind::Unexpected {
            found: found.map(str::to_owned),
            expected,
        };
        let e = error("status = ");
        assert_eq!(
            (e.span, e.kind),
            (Span::new(9, 0), unexpected(None, vec![Expected::Value]))
        );
        let e = error("status todo");
        assert_eq!(e.span, Span::new(7, 4));
        assert_eq!(e.kind, unexpected(Some("todo"), COMPARISONS.to_vec()));
        let e = error("(status = todo");
        assert_eq!(
            e.kind,
            unexpected(
                None,
                vec![
                    Expected::Token("AND"),
                    Expected::Token("OR"),
                    Expected::Token(")")
                ]
            )
        );
        let e = error("status = todo priority = high");
        assert_eq!(e.span, Span::new(14, 8));
        assert_eq!(
            e.expected(),
            [
                Expected::Token("AND"),
                Expected::Token("OR"),
                Expected::Token("ORDER BY"),
                Expected::End
            ]
        );
        assert_eq!(error("status IN todo").expected(), [Expected::Token("(")]);
        assert_eq!(
            error("status IN (todo,)").expected(),
            [Expected::Value],
            "a value after a comma"
        );
        assert_eq!(error("ORDER status").expected(), [Expected::Token("BY")]);
        assert_eq!(error("ORDER BY").expected(), [Expected::Field]);
        assert_eq!(
            error("a IS b").expected(),
            [Expected::Token("EMPTY"), Expected::Token("NOT")]
        );
        assert_eq!(
            error("AND x = 1").expected(),
            [Expected::Field, Expected::Token("("), Expected::Token("NOT")]
        );
        assert_eq!(error("x = AND").expected(), [Expected::Value]);
        assert_eq!(error("x NOT y").expected(), [Expected::Token("IN")]);
    }

    #[test]
    fn unknown_names_and_wrong_arguments_are_refused() {
        // `statut` may be a custom field (the configurations tell); `my-field` is no field name.
        let e = error("statut = todo AND my-field = 1");
        assert_eq!(
            (e.span, e.kind),
            (
                Span::new(18, 8),
                QueryErrorKind::UnknownField("my-field".to_owned())
            )
        );
        let e = error("x = yesterday()");
        assert_eq!(
            (e.span, e.kind),
            (
                Span::new(4, 9),
                QueryErrorKind::UnknownFunction("yesterday".to_owned())
            )
        );
        let e = error("x = currentUser(1)");
        assert_eq!(
            (e.span, e.kind),
            (
                Span::new(4, 14),
                QueryErrorKind::WrongArguments {
                    function: "currentUser".to_owned(),
                    expected: 0
                }
            )
        );
        assert_eq!(
            error("project IN descendantsOf()").kind,
            QueryErrorKind::WrongArguments {
                function: "descendantsOf".to_owned(),
                expected: 1
            }
        );
        assert_eq!(
            error("project IN descendantsOf(3)").kind,
            QueryErrorKind::WrongArguments {
                function: "descendantsOf".to_owned(),
                expected: 1
            }
        );
    }

    #[test]
    fn limits_keep_queries_small() {
        let long = format!("summary ~ \"{}\"", "x".repeat(MAX_QUERY_CHARS));
        assert_eq!(error(&long).kind, QueryErrorKind::TooLong);
        let deep = format!(
            "{}a IS EMPTY{}",
            "(".repeat(MAX_DEPTH + 1),
            ")".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(error(&deep).kind, QueryErrorKind::TooDeep);
        let nots = format!("{}a IS EMPTY", "NOT ".repeat(MAX_DEPTH + 1));
        assert_eq!(error(&nots).kind, QueryErrorKind::TooDeep);
        let many = vec!["a IS EMPTY"; MAX_CLAUSES + 1].join(" OR ");
        assert_eq!(error(&many).kind, QueryErrorKind::TooManyClauses);
        let list = (0..=MAX_LIST_VALUES)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            error(&format!("a IN ({list})")).kind,
            QueryErrorKind::TooManyValues
        );
        let keys = ["due"; MAX_ORDER_KEYS + 1].join(", ");
        assert_eq!(
            error(&format!("ORDER BY {keys}")).kind,
            QueryErrorKind::TooManyOrderKeys
        );
        // At the limits: fine.
        assert!(parse_syntax(&vec!["a IS EMPTY"; MAX_CLAUSES].join(" OR ")).is_ok());
        let deepest = format!("{}a IS EMPTY{}", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
        assert!(parse_syntax(&deepest).is_ok());
    }
}
