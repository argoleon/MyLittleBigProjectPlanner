//! Completion of a query being typed (L06-US02): what may come where the
//! member's cursor stands.
//!
//! The text before the cursor is parsed in a tolerant way: it ends with a
//! cursor token instead of the end of the query, so the parser stops there
//! and tells what it expected, with its context (the field of the condition
//! being written, an `IN` list, the arguments of a function, `ORDER BY`).
//! The word or string being typed is left out: the chosen suggestion
//! replaces it.

use super::lexer::{is_word_char, tokenize};
use super::parser::expectations;
use super::{Expected, Field, MAX_QUERY_CHARS, QueryErrorKind, Span};

/// What may come at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// What a suggestion replaces: the word or string being typed (empty
    /// when the cursor follows a space or a symbol).
    pub replace: Span,
    /// The text typed so far there, without an opening quote.
    pub partial: String,
    /// The member is typing inside a quoted string.
    pub quoted: bool,
    /// What the language accepts there; empty when the text before the
    /// cursor is already wrong.
    pub expected: Vec<Expected>,
    /// The field of the condition being written, for operators and values.
    pub field: Option<Field>,
    /// Inside the values of an `IN`.
    pub in_list: bool,
    /// Inside the arguments of this function (lowercase name).
    pub function: Option<String>,
    /// After `ORDER BY`: fields to sort by.
    pub sorting: bool,
}

/// What may come at character `cursor` of `input`.
pub fn complete(input: &str, cursor: usize) -> Completion {
    let chars: Vec<char> = input.chars().collect();
    let cursor = cursor.min(chars.len()).min(MAX_QUERY_CHARS);
    let prefix: String = chars[..cursor].iter().collect();
    // The string being typed, else the word around the cursor.
    let (start, end, quoted) = match tokenize(&prefix) {
        Err(error) if error.kind == QueryErrorKind::UnterminatedString => (error.span.start, cursor, true),
        _ => {
            let mut start = cursor;
            while start > 0 && is_word_char(chars[start - 1]) {
                start -= 1;
            }
            let mut end = cursor;
            while end < chars.len() && is_word_char(chars[end]) {
                end += 1;
            }
            (start, end, false)
        }
    };
    let partial: String = chars[start + usize::from(quoted)..cursor].iter().collect();
    let mut completion = Completion {
        replace: Span::new(start, end - start),
        partial,
        quoted,
        expected: Vec::new(),
        field: None,
        in_list: false,
        function: None,
        sorting: false,
    };
    let before: Vec<char> = chars[..start].to_vec();
    let Ok(tokens) = tokenize(&before.iter().collect::<String>()) else {
        return completion;
    };
    let at_start = tokens.len() == 1;
    let Some((mut expected, context)) = expectations(before, tokens) else {
        return completion;
    };
    if at_start {
        expected.push(Expected::Token("ORDER BY"));
    }
    completion.expected = expected;
    completion.field = context.field;
    completion.in_list = context.list;
    completion.function = context.function;
    completion.sorting = context.sorting;
    completion
}

#[cfg(test)]
mod tests {
    use super::super::SystemField;
    use super::*;

    fn at_end(input: &str) -> Completion {
        complete(input, input.chars().count())
    }

    fn tokens(expected: &[Expected]) -> Vec<&'static str> {
        expected.iter().map(Expected::as_str).collect()
    }

    #[test]
    fn a_field_is_expected_first_the_word_typed_replaced() {
        let completion = at_end("stat");
        assert_eq!(completion.replace, Span::new(0, 4));
        assert_eq!(completion.partial, "stat");
        assert_eq!(tokens(&completion.expected), ["a field", "(", "NOT", "ORDER BY"]);
        assert_eq!(
            tokens(&at_end("status = todo AND ").expected),
            ["a field", "(", "NOT"]
        );
        assert_eq!(at_end("").replace, Span::new(0, 0));
    }

    #[test]
    fn after_a_field_its_operators_then_its_values() {
        let operator = at_end("status ");
        assert_eq!(operator.field, Some(Field::System(SystemField::Status)));
        assert!(tokens(&operator.expected).contains(&"IN"));
        let value = at_end("status = in_pr");
        assert_eq!(
            (value.field.clone(), value.partial.as_str(), value.replace),
            (Some(Field::System(SystemField::Status)), "in_pr", Span::new(9, 5))
        );
        assert_eq!(tokens(&value.expected), ["a value"]);
        let listed = at_end("status IN (todo, ");
        assert!(listed.in_list && listed.field == Some(Field::System(SystemField::Status)));
        assert_eq!(tokens(&listed.expected), ["a value"]);
    }

    #[test]
    fn inside_a_string_or_a_function_or_after_order_by() {
        let quoted = at_end("project IN descendantsOf(\"PORTF-");
        assert!(quoted.quoted);
        assert_eq!(
            (quoted.partial.as_str(), quoted.function.as_deref()),
            ("PORTF-", Some("descendantsof"))
        );
        assert_eq!(quoted.replace, Span::new(25, 7));
        let sorting = at_end("status = todo ORDER BY pri");
        assert!(sorting.sorting);
        assert_eq!(tokens(&sorting.expected), ["a field"]);
        assert_eq!(
            tokens(&at_end("status = todo ORDER BY priority ").expected),
            [",", "ASC", "DESC", "the end of the query"]
        );
        assert_eq!(
            tokens(&at_end("status = todo ").expected),
            ["AND", "OR", "ORDER BY", "the end of the query"]
        );
    }

    #[test]
    fn the_cursor_may_stand_inside_a_word() {
        let inside = complete("status = in_progress AND x", 12);
        assert_eq!(
            (inside.replace, inside.partial.as_str()),
            (Span::new(9, 11), "in_")
        );
        assert_eq!(inside.field, Some(Field::System(SystemField::Status)));
    }

    #[test]
    fn nothing_is_expected_after_a_mistake() {
        assert!(at_end("status todo ").expected.is_empty());
        assert!(at_end("a = b; ").expected.is_empty());
        assert!(
            complete("status = ", 999).expected == vec![Expected::Value],
            "the cursor is kept in the text"
        );
    }
}
