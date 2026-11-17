//! Help to write queries of the language (L06-US02): suggestions where the
//! cursor stands, and the tree of a query for the visual builder.
//!
//! - `POST /api/v1/search/suggestions`: the parser reads the text before the
//!   cursor in a tolerant way (`domain::query::complete`) and tells what may
//!   come there; this module turns that into fields, operators, keywords,
//!   functions and values (types, statuses, projects and nodes the member
//!   sees, members, list values), with their translated labels and the text
//!   to insert.
//! - `POST /api/v1/search/parse`: the checked query as a tree, `AND` and `OR`
//!   chains flattened, with its canonical text; the builder reads its
//!   criteria from it.

use axum::response::{IntoResponse, Response};
use domain::query::{
    Completion, Condition, Expected, Expr, Field, Function, Period, Query, SystemField, Value, complete,
    is_sortable, operators_of,
};
use infra::query_suggestions::{self as suggest, Suggested};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::extract::{AcceptLanguage, ApiJson};
use crate::nodes::Sight;
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::search::{TAG, query_problem};
use crate::tenant::TenantTx;

/// Most suggestions returned.
pub const MAX_SUGGESTIONS: usize = 30;

/// Where the member is typing.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SuggestionRequest, example = json!({"query": "status = in_pr", "cursor": 14}))]
pub struct SuggestBody {
    /// The query as typed so far.
    #[serde(default)]
    query: String,
    /// The cursor: a character offset from 0; the end of the query when left out.
    #[serde(default)]
    cursor: Option<usize>,
}

/// What a suggestion is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SuggestionKind)]
pub enum SuggestionKind {
    Field,
    Operator,
    Keyword,
    Function,
    Value,
}

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Suggestion)]
pub struct SuggestionView {
    kind: SuggestionKind,
    /// What replaces `from`..`to` once chosen, as the query writes it.
    #[schema(example = "in_progress")]
    text: String,
    /// What it reads as, in the member's language (a status label, a
    /// project or member name); `null` for the language's own words.
    #[schema(required = true, example = "In progress")]
    label: Option<String>,
    /// More to tell it apart: a status category, a node type, a field kind.
    #[schema(required = true)]
    detail: Option<String>,
    /// Where to put the cursor in `text` once inserted, when not at its end
    /// (inside the parentheses of `descendantsOf("")`).
    #[serde(skip_serializing_if = "Option::is_none")]
    caret: Option<usize>,
}

/// The suggestions at the cursor.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Suggestions)]
pub struct SuggestionsView {
    /// The part of the query a suggestion replaces: from this character…
    from: usize,
    /// …to this one (excluded).
    to: usize,
    suggestions: Vec<SuggestionView>,
}

fn word(kind: SuggestionKind, text: impl Into<String>) -> SuggestionView {
    SuggestionView {
        kind,
        text: text.into(),
        label: None,
        detail: None,
        caret: None,
    }
}

/// Whether `candidate` starts like `typed`, any case.
fn starts(candidate: &str, typed: &str) -> bool {
    candidate.to_lowercase().starts_with(&typed.to_lowercase())
}

/// The value `key` as the query writes it: bare when it can be, else quoted.
fn written(key: &str) -> String {
    Value::Text(key.to_owned()).to_string()
}

/// The fields the language offers here, custom ones included.
async fn field_suggestions(
    tx: &mut TenantTx,
    language: &Language,
    completion: &Completion,
) -> Result<Vec<SuggestionView>, Problem> {
    let typed = &completion.partial;
    let mut found: Vec<SuggestionView> = SystemField::ALL
        .into_iter()
        .filter(|f| !f.is_pending() && starts(f.as_str(), typed))
        .map(Field::System)
        .filter(|f| !completion.sorting || is_sortable(f))
        .map(|f| word(SuggestionKind::Field, f.name()))
        .collect();
    for field in suggest::fields(tx, typed, MAX_SUGGESTIONS).await? {
        if SystemField::ALL
            .iter()
            .any(|f| f.as_str().eq_ignore_ascii_case(&field.key))
        {
            continue;
        }
        found.push(SuggestionView {
            label: language.label(&field.labels),
            detail: field.detail,
            ..word(SuggestionKind::Field, field.key)
        });
    }
    Ok(found)
}

/// The operators and keywords that may come, those of the field when one is written.
fn token_suggestions(completion: &Completion) -> Vec<SuggestionView> {
    let typed = &completion.partial;
    let mut found = Vec::new();
    let comparing = completion.expected.contains(&Expected::Token("IN"));
    if comparing && let Some(field) = &completion.field {
        for operator in operators_of(field) {
            let kind = if operator.chars().any(char::is_alphabetic) {
                SuggestionKind::Keyword
            } else {
                SuggestionKind::Operator
            };
            if starts(&operator, typed) {
                found.push(word(kind, operator));
            }
        }
        return found;
    }
    for expected in &completion.expected {
        let Expected::Token(token) = expected else {
            continue;
        };
        if matches!(*token, "(" | ")" | ",") || !starts(token, typed) {
            continue;
        }
        let kind = if token.chars().any(char::is_alphabetic) {
            SuggestionKind::Keyword
        } else {
            SuggestionKind::Operator
        };
        found.push(word(kind, *token));
    }
    found
}

fn function(text: &str, caret: Option<usize>) -> SuggestionView {
    SuggestionView {
        caret,
        ..word(SuggestionKind::Function, text)
    }
}

/// The functions a value of `field` may be.
fn function_suggestions(completion: &Completion, field: &Field) -> Vec<SuggestionView> {
    let typed = &completion.partial;
    let functions: Vec<SuggestionView> = match field {
        Field::System(SystemField::Assignee | SystemField::Reporter) => vec![function("currentUser()", None)],
        Field::System(
            SystemField::Created | SystemField::Updated | SystemField::Due | SystemField::Start,
        ) => {
            let mut all = vec![function("now()", None)];
            for period in Period::ALL {
                all.push(function(&Function::StartOf(period).to_string(), None));
                all.push(function(&Function::EndOf(period).to_string(), None));
            }
            all
        }
        Field::System(SystemField::Project)
            if completion.in_list || completion.expected.contains(&Expected::Token("(")) =>
        {
            vec![function("descendantsOf(\"\")", Some("descendantsOf(\"".len()))]
        }
        _ => Vec::new(),
    };
    functions.into_iter().filter(|f| starts(&f.text, typed)).collect()
}

fn values_of(language: &Language, found: Vec<Suggested>) -> Vec<SuggestionView> {
    found
        .into_iter()
        .map(|value| SuggestionView {
            label: value
                .labels
                .get("")
                .cloned()
                .or_else(|| language.label(&value.labels)),
            detail: value.detail,
            ..word(SuggestionKind::Value, written(&value.key))
        })
        .collect()
}

/// The values of `field` matching what is typed.
async fn value_suggestions(
    tx: &mut TenantTx,
    language: &Language,
    completion: &Completion,
    field: &Field,
) -> Result<Vec<SuggestionView>, Problem> {
    let typed = completion.partial.as_str();
    let limit = MAX_SUGGESTIONS;
    let in_descendants = completion.function.as_deref() == Some("descendantsof");
    let found = match field {
        Field::System(SystemField::Project) => {
            let sight = Sight::of(tx).await?;
            let visible = sight.visible();
            suggest::nodes(tx, visible.as_deref(), in_descendants, typed, limit).await?
        }
        Field::System(SystemField::Type) => suggest::issue_types(tx, typed, limit).await?,
        Field::System(SystemField::Status) => suggest::statuses(tx, typed, limit).await?,
        Field::System(SystemField::Assignee | SystemField::Reporter) => {
            suggest::members(tx, typed, limit).await?
        }
        Field::System(SystemField::Priority) => domain::issues::Priority::ALL
            .into_iter()
            .filter(|p| starts(p.as_str(), typed))
            .map(|p| Suggested {
                key: p.as_str().to_owned(),
                labels: Default::default(),
                detail: None,
            })
            .collect(),
        Field::Custom(key) => suggest::list_values(tx, key, typed, limit).await?,
        Field::System(_) => Vec::new(),
    };
    let mut views = values_of(language, found);
    if in_descendants {
        // Inside the quotes already: the key alone, quoted.
        for view in &mut views {
            view.text = format!("\"{}\"", view.text.trim_matches('"'));
        }
    } else {
        views.extend(function_suggestions(completion, field));
    }
    Ok(views)
}

/// `POST /api/v1/search/suggestions`.
#[utoipa::path(
    post,
    path = "/search/suggestions",
    tag = TAG,
    operation_id = "suggestQuery",
    summary = "Suggest what may come at the cursor of a query",
    description = "Reads the query up to `cursor` in a tolerant way and suggests what the language \
                   accepts there: fields (custom ones included), the operators of the field being \
                   written, keywords, functions, and values with their labels in the member's \
                   language (types, statuses, priorities, members, the projects and nodes the member \
                   sees, list values). A suggestion replaces the characters `from`..`to` (the word or \
                   string being typed). Nothing is suggested after a mistake.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    request_body = SuggestBody,
    responses(
        (status = 200, description = "The suggestions", body = SuggestionsView),
        (status = 400, description = "`invalid_json`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn suggestions(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<SuggestBody>,
) -> Result<Response, Problem> {
    let cursor = body.cursor.unwrap_or_else(|| body.query.chars().count());
    let completion = complete(&body.query, cursor);
    let language = Language::of(&tx, &accept);
    let mut found = Vec::new();
    for expected in &completion.expected {
        match expected {
            Expected::Field => found.extend(field_suggestions(&mut tx, &language, &completion).await?),
            Expected::Value => {
                if let Some(field) = completion.field.clone() {
                    found.extend(value_suggestions(&mut tx, &language, &completion, &field).await?);
                }
            }
            Expected::Token(_) | Expected::Text | Expected::End => {}
        }
    }
    if let Some(field) = &completion.field
        && completion.expected == [Expected::Token("(")]
    {
        found.extend(function_suggestions(&completion, field));
    }
    found.extend(token_suggestions(&completion));
    let mut unique: Vec<SuggestionView> = Vec::new();
    for suggestion in found {
        if !unique
            .iter()
            .any(|s| s.text == suggestion.text && s.kind == suggestion.kind)
        {
            unique.push(suggestion);
        }
    }
    unique.truncate(MAX_SUGGESTIONS);
    tx.commit().await?;
    let mut response = ApiJson(SuggestionsView {
        from: completion.replace.start,
        to: completion.replace.start + completion.replace.len,
        suggestions: unique,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// A query to read.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ParseRequest, example = json!({"query": "project = PLAT AND status IN (in_progress)"}))]
pub struct ParseBody {
    #[serde(default)]
    query: String,
}

/// A value of a condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = QueryValue)]
pub struct ValueView {
    /// `text`, `number`, `relative` or `function`.
    #[schema(example = "text")]
    kind: &'static str,
    /// The text or number as is; a duration (`-7d`) or a call
    /// (`endOfWeek()`) as the query writes it.
    #[schema(example = "in_progress")]
    value: String,
}

/// A node of the tree of a query: `and` and `or` hold their operands
/// (chains flattened), `not` one, `clause` a condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = QueryNode)]
pub struct NodeView {
    /// `and`, `or`, `not` or `clause`.
    #[schema(example = "clause")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(no_recursion, required = false)]
    operands: Vec<NodeView>,
    /// For a clause: the field, as the query writes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    /// For a clause: `=`, `IN`, `IS EMPTY`…
    #[serde(skip_serializing_if = "Option::is_none")]
    operator: Option<String>,
    /// For a clause: its values (none for `IS EMPTY`); the text of `~`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(required = false)]
    values: Vec<ValueView>,
}

/// A sort key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = QuerySortKey)]
pub struct OrderView {
    field: String,
    descending: bool,
}

/// A query read.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ParsedQuery)]
pub struct ParsedView {
    /// The canonical text of the query.
    #[schema(example = "project = PLAT AND status IN (in_progress)")]
    query: String,
    /// `null` for a query without filter.
    #[schema(required = true)]
    filter: Option<NodeView>,
    order_by: Vec<OrderView>,
}

fn value_view(value: &Value) -> ValueView {
    let (kind, value) = match value {
        Value::Text(text) => ("text", text.clone()),
        Value::Number(number) => ("number", number.clone()),
        Value::Relative(relative) => ("relative", relative.to_string()),
        Value::Function(function) => ("function", function.to_string()),
    };
    ValueView { kind, value }
}

fn node_view(expr: &Expr) -> NodeView {
    let empty = NodeView {
        kind: "clause",
        operands: Vec::new(),
        field: None,
        operator: None,
        values: Vec::new(),
    };
    match expr {
        Expr::And(..) | Expr::Or(..) => {
            let and = matches!(expr, Expr::And(..));
            let mut operands = Vec::new();
            let mut stack = vec![expr];
            while let Some(next) = stack.pop() {
                match next {
                    Expr::And(left, right) if and => {
                        stack.push(right);
                        stack.push(left);
                    }
                    Expr::Or(left, right) if !and => {
                        stack.push(right);
                        stack.push(left);
                    }
                    other => operands.push(node_view(other)),
                }
            }
            NodeView {
                kind: if and { "and" } else { "or" },
                operands,
                ..empty
            }
        }
        Expr::Not(inner) => NodeView {
            kind: "not",
            operands: vec![node_view(inner)],
            ..empty
        },
        Expr::Clause(clause) => NodeView {
            field: Some(clause.field.name().to_owned()),
            operator: Some(clause.condition.operator().to_owned()),
            values: match &clause.condition {
                Condition::Compare(_, value) => vec![value_view(value)],
                Condition::In { values, .. } => values.iter().map(value_view).collect(),
                Condition::Contains { text, .. } => vec![value_view(&Value::Text(text.clone()))],
                Condition::Empty { .. } => Vec::new(),
            },
            ..empty
        },
    }
}

fn parsed_view(query: &Query) -> ParsedView {
    ParsedView {
        query: query.to_string(),
        filter: query.filter.as_ref().map(node_view),
        order_by: query
            .order_by
            .iter()
            .map(|key| OrderView {
                field: key.field.name().to_owned(),
                descending: key.descending,
            })
            .collect(),
    }
}

/// `POST /api/v1/search/parse`.
#[utoipa::path(
    post,
    path = "/search/parse",
    tag = TAG,
    operation_id = "parseQuery",
    summary = "Read a query of the language as a tree",
    description = "Parses and checks the query, then returns it as a tree (`and` and `or` chains \
                   flattened) with its canonical text: the visual builder reads its criteria from \
                   it. Nothing is run.",
    request_body = ParseBody,
    responses(
        (status = 200, description = "The query read", body = ParsedView),
        (status = 400, description = "`query.invalid` (see `query`), `invalid_json`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn parse(tx: TenantTx, ApiJson(body): ApiJson<ParseBody>) -> Result<Response, Problem> {
    drop(tx);
    let query = domain::query::parse(&body.query).map_err(|e| query_problem(&e))?;
    Ok(ApiJson(parsed_view(&query)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(query: &str) -> serde_json::Value {
        serde_json::to_value(parsed_view(&domain::query::parse(query).unwrap())).unwrap()
    }

    #[test]
    fn a_query_reads_as_a_flat_tree_with_its_canonical_text() {
        assert_eq!(
            parsed(
                "project = plat and status in (todo, in_progress) and due < endOfWeek() order by priority desc"
            ),
            serde_json::json!({
                "query": "project = plat AND status IN (todo, in_progress) AND due < endOfWeek() ORDER BY priority DESC",
                "filter": {"kind": "and", "operands": [
                    {"kind": "clause", "field": "project", "operator": "=", "values": [{"kind": "text", "value": "plat"}]},
                    {"kind": "clause", "field": "status", "operator": "IN", "values": [
                        {"kind": "text", "value": "todo"}, {"kind": "text", "value": "in_progress"}]},
                    {"kind": "clause", "field": "due", "operator": "<", "values": [{"kind": "function", "value": "endOfWeek()"}]},
                ]},
                "orderBy": [{"field": "priority", "descending": true}],
            })
        );
        assert_eq!(
            parsed("NOT (a IS EMPTY OR b ~ x OR created > -7d)")["filter"],
            serde_json::json!({"kind": "not", "operands": [{"kind": "or", "operands": [
                {"kind": "clause", "field": "a", "operator": "IS EMPTY"},
                {"kind": "clause", "field": "b", "operator": "~", "values": [{"kind": "text", "value": "x"}]},
                {"kind": "clause", "field": "created", "operator": ">", "values": [{"kind": "relative", "value": "-7d"}]},
            ]}]})
        );
        assert_eq!(parsed("")["filter"], serde_json::Value::Null);
    }

    #[test]
    fn operators_follow_the_field_and_keywords_what_is_typed() {
        let status = complete("status ", 7);
        let operators: Vec<String> = token_suggestions(&status).into_iter().map(|s| s.text).collect();
        assert_eq!(operators, ["=", "!=", "IN", "NOT IN"]);
        let after = complete("status = todo o", 15);
        let keywords: Vec<(SuggestionKind, String)> = token_suggestions(&after)
            .into_iter()
            .map(|s| (s.kind, s.text))
            .collect();
        assert_eq!(
            keywords,
            [
                (SuggestionKind::Keyword, "OR".to_owned()),
                (SuggestionKind::Keyword, "ORDER BY".to_owned())
            ]
        );
        let due = complete("due < end", 9);
        let functions: Vec<String> = function_suggestions(&due, &Field::System(SystemField::Due))
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(
            functions,
            ["endOfDay()", "endOfWeek()", "endOfMonth()", "endOfYear()"]
        );
        assert_eq!(written("in_progress"), "in_progress");
        assert_eq!(written("lucas bernard"), "\"lucas bernard\"");
    }
}
