//! Search with the query language (specification §6, §15, L06-US01):
//! `POST /api/v1/search`.
//!
//! The query is parsed and checked (`domain::query`), then run by
//! `infra::search` over the tickets of the projects the member sees, dates
//! in their time zone. A query that cannot run answers `query.invalid`
//! with a `query` member: the reason, the character position and length,
//! and for syntax errors what was expected there.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::response::{IntoResponse, Response};
use domain::issues::Priority;
use domain::query::{QueryError, QueryErrorKind};
use domain::validation::FieldError;
use infra::search::{Context, SearchError};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;

use crate::extract::{AcceptLanguage, ApiJson};
use crate::fields::FieldSelection;
use crate::issues::{
    Content, IssueRefView, PersonView, PriorityView, ProjectView, StatusView, TypeView, issue_ref, person,
    project_ref,
};
use crate::nodes::Sight;
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody, QueryProblem};
use crate::roles::Language;
use crate::tenant::TenantTx;

pub const TAG: &str = "search";

/// Results per page by default.
pub const DEFAULT_LIMIT: u32 = 50;
/// Most results per page.
pub const MAX_LIMIT: u32 = 100;

/// The fields of a result, for `fields`.
const RESULT_FIELDS: [&str; 15] = [
    "id",
    "key",
    "summary",
    "project",
    "type",
    "status",
    "priority",
    "assignee",
    "reporter",
    "startDate",
    "dueDate",
    "parentId",
    "parent",
    "createdAt",
    "updatedAt",
];

/// A search.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = SearchRequest,
    example = json!({
        "query": "assignee = currentUser() AND status IN (todo, in_progress) ORDER BY priority DESC",
        "fields": ["key", "summary", "status", "priority"],
        "limit": 50
    })
)]
pub struct SearchBody {
    /// The query (specification §6), 10,000 characters at most. Empty:
    /// every ticket the member sees, newest first.
    #[serde(default)]
    query: String,
    /// The fields of each result to return (`id` always comes); all by
    /// default.
    #[serde(default)]
    fields: Vec<String>,
    /// `parent`: the ticket each one sits under, in a few words.
    #[serde(default)]
    expand: Vec<String>,
    /// `nextCursor` of the previous page, with the same query.
    #[serde(default)]
    cursor: Option<String>,
    /// 1 to 100, 50 by default.
    #[serde(default)]
    limit: Option<u32>,
}

/// A ticket found.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SearchResult)]
pub struct ResultView {
    id: Uuid,
    #[schema(example = "PLAT-142")]
    key: String,
    summary: String,
    project: ProjectView,
    #[serde(rename = "type")]
    issue_type: TypeView,
    status: StatusView,
    priority: PriorityView,
    #[schema(required = true)]
    assignee: Option<PersonView>,
    #[schema(required = true)]
    reporter: Option<PersonView>,
    #[schema(required = true)]
    start_date: Option<Date>,
    #[schema(required = true)]
    due_date: Option<Date>,
    #[schema(required = true)]
    parent_id: Option<Uuid>,
    /// `expand=parent`: the ticket it sits under.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<IssueRefView>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

/// A page of results.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SearchPage)]
pub struct PageView {
    /// The results, with the fields asked for.
    #[schema(value_type = Vec<ResultView>)]
    items: Vec<serde_json::Value>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
    /// How many tickets match: on the first page (no `cursor`) only.
    #[schema(required = true, example = 38)]
    total: Option<i64>,
}

/// An English sentence for `error`, for logs and API users; the interface
/// builds its own from `query`.
fn sentence(error: &QueryError) -> String {
    let at = error.span.start;
    match &error.kind {
        QueryErrorKind::Unexpected { found, expected } => {
            let expected: Vec<&str> = expected.iter().map(domain::query::Expected::as_str).collect();
            format!(
                "Expected {} at position {at}, found {}.",
                expected.join(" or "),
                found
                    .as_deref()
                    .map_or("the end of the query".to_owned(), |f| format!("`{f}`"))
            )
        }
        QueryErrorKind::UnterminatedString => format!("The quote at position {at} is never closed."),
        QueryErrorKind::InvalidCharacter(c) => format!("`{c}` at position {at} is not part of the language."),
        QueryErrorKind::InvalidNumber(n) => format!("`{n}` at position {at} is not a number or a duration."),
        QueryErrorKind::UnknownField(field) => format!("There is no field `{field}`."),
        QueryErrorKind::UnknownFunction(function) => format!("There is no function `{function}()`."),
        QueryErrorKind::WrongArguments { function, expected } => {
            format!("`{function}()` takes {expected} argument(s).")
        }
        QueryErrorKind::OperatorNotAllowed {
            field,
            operator,
            allowed,
        } => {
            format!(
                "`{field}` does not take `{operator}`; it takes {}.",
                allowed.join(", ")
            )
        }
        QueryErrorKind::ValueNotAllowed { field, value } => {
            format!("`{field}` does not take the value `{value}`.")
        }
        QueryErrorKind::InvalidDate(date) => {
            format!("`{date}` is not a date (YYYY-MM-DD or YYYY-MM-DD HH:MM).")
        }
        QueryErrorKind::UnknownPriority(p) => format!("`{p}` is not a priority."),
        QueryErrorKind::NotSortable(field) => format!("Tickets cannot be sorted by `{field}`."),
        QueryErrorKind::Unsupported(field) => format!("`{field}` comes with sprints and versions."),
        QueryErrorKind::TooLong => "The query is too long.".to_owned(),
        QueryErrorKind::TooDeep => "The query nests too deep.".to_owned(),
        QueryErrorKind::TooManyClauses => "The query has too many conditions.".to_owned(),
        QueryErrorKind::TooManyValues => "The list has too many values.".to_owned(),
        QueryErrorKind::TooManyOrderKeys => "The query sorts by too many fields.".to_owned(),
    }
}

/// `query.invalid` for `error`.
pub(crate) fn query_problem(error: &QueryError) -> Problem {
    let (field, allowed) = match &error.kind {
        QueryErrorKind::UnknownField(field)
        | QueryErrorKind::ValueNotAllowed { field, .. }
        | QueryErrorKind::NotSortable(field)
        | QueryErrorKind::Unsupported(field) => (Some(field.clone()), Vec::new()),
        QueryErrorKind::OperatorNotAllowed { field, allowed, .. } => (Some(field.clone()), allowed.clone()),
        _ => (None, Vec::new()),
    };
    let found = match &error.kind {
        QueryErrorKind::Unexpected { found, .. } => found.clone(),
        _ => None,
    };
    Problem::query(
        QueryProblem {
            reason: error.code(),
            position: error.span.start,
            length: error.span.len,
            expected: error
                .expected()
                .iter()
                .map(domain::query::Expected::as_str)
                .collect(),
            found,
            field,
            allowed,
        },
        sentence(error),
    )
}

fn search_problem(error: SearchError) -> Problem {
    match error {
        SearchError::Query(error) => query_problem(&error),
        SearchError::Cursor => Problem::validation(vec![FieldError::new("cursor", "invalid")]),
        SearchError::Database(err) => Problem::internal(&err),
    }
}

/// `POST /api/v1/search`.
#[utoipa::path(
    post,
    path = "/search",
    tag = TAG,
    operation_id = "searchWithQuery",
    summary = "Search tickets with the query language",
    description = "Runs a query of the language over the tickets of the projects the member sees \
                   (specification §6): `assignee = currentUser() AND status IN (todo, in_progress) \
                   ORDER BY priority DESC`. Fields and values are stable identifiers; dates are read \
                   in the member's time zone. Pages follow `nextCursor`; the first page also gives \
                   the `total`. A query that cannot run answers `query.invalid` with a `query` \
                   member telling the reason, the character `position` and `length`, and for syntax \
                   errors what was `expected`.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    request_body = SearchBody,
    responses(
        (status = 200, description = "A page of tickets", body = PageView),
        (status = 400, description = "`query.invalid` (see `query`); `validation_failed`: `limit` \
                                      (`out_of_range`), `cursor` (`invalid`: not from this query), \
                                      `fields.<name>` or `expand.<name>` (`unknown_field`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn search(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<SearchBody>,
) -> Result<Response, Problem> {
    let mut errors = Vec::new();
    let limit = body.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        errors.push(FieldError::new("limit", "out_of_range"));
    }
    for name in &body.fields {
        if !RESULT_FIELDS.contains(&name.as_str()) {
            errors.push(FieldError::new(format!("fields.{name}"), "unknown_field"));
        }
    }
    for name in &body.expand {
        if name != "parent" {
            errors.push(FieldError::new(format!("expand.{name}"), "unknown_field"));
        }
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let selection = if body.fields.is_empty() {
        None
    } else {
        let mut fields = body.fields.clone();
        fields.insert(0, "id".to_owned());
        Some(FieldSelection::parse(
            Some(&fields.join(",")),
            &RESULT_FIELDS,
            &RESULT_FIELDS,
        )?)
    };
    let query = domain::query::parse(&body.query).map_err(|e| query_problem(&e))?;

    let account = tx.account_id();
    let sight = Sight::of(&mut tx).await?;
    let visible = sight.visible();
    let time_zone = infra::search::time_zone_of(&mut tx, account).await?;
    let context = Context {
        account_id: account,
        time_zone: &time_zone,
        visible_projects: visible.as_deref(),
    };
    let page = infra::search::search(
        &mut tx,
        &query,
        &context,
        body.cursor.as_deref(),
        limit as usize,
        body.cursor.is_none(),
    )
    .await
    .map_err(search_problem)?;

    // The projects of the page in one query, each configuration read once.
    let project_ids: Vec<Uuid> = page
        .items
        .iter()
        .map(|i| i.project_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let projects: HashMap<Uuid, infra::search::ProjectOfPage> =
        infra::search::projects_of(&mut tx, &project_ids)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
    let mut contents: BTreeMap<Uuid, Content> = BTreeMap::new();
    for config in projects.values().filter_map(|p| p.config_id) {
        if let std::collections::btree_map::Entry::Vacant(entry) = contents.entry(config) {
            entry.insert(Content::of_config(&mut tx, config).await?);
        }
    }
    let parents: HashMap<Uuid, infra::issues::Issue> = if body.expand.iter().any(|e| e == "parent") {
        let ids: Vec<Uuid> = page.items.iter().filter_map(|i| i.parent_id).collect();
        infra::issues::find_many(&mut tx, &ids)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect()
    } else {
        HashMap::new()
    };
    let language = Language::of(&tx, &accept);
    let mut items = Vec::with_capacity(page.items.len());
    for issue in page.items {
        let Some(project) = projects.get(&issue.project_id) else {
            continue;
        };
        let content = project.config_id.and_then(|c| contents.get(&c));
        let (issue_type, status) = match content {
            Some(content) => (
                language.type_of(content, issue.type_id),
                language.status_view(content, issue.status_id),
            ),
            None => continue,
        };
        let view = ResultView {
            id: issue.id,
            key: issue.key,
            summary: issue.summary,
            project: project_ref(project.id, project.key.clone(), project.name.clone()),
            issue_type,
            status,
            priority: Priority::parse(&issue.priority).unwrap_or_default().into(),
            assignee: person(issue.assignee_id, issue.assignee_name.as_ref()),
            reporter: person(issue.reporter_id, issue.reporter_name.as_ref()),
            start_date: issue.start_date,
            due_date: issue.due_date,
            parent_id: issue.parent_id,
            parent: issue.parent_id.and_then(|p| parents.get(&p).map(issue_ref)),
            created_at: issue.created_at,
            updated_at: issue.updated_at,
        };
        let json = serde_json::to_value(&view).unwrap_or_default();
        items.push(match &selection {
            Some(selection) => selection.apply(json),
            None => json,
        });
    }
    tx.commit().await?;
    let mut response = ApiJson(PageView {
        items,
        next_cursor: page.next_cursor,
        total: page.total,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem_of(query: &str) -> QueryProblem {
        let error = domain::query::parse(query).unwrap_err();
        *query_problem(&error).query.unwrap()
    }

    #[test]
    fn a_syntax_error_tells_the_position_and_what_was_expected() {
        let problem = problem_of("status = ");
        assert_eq!(
            (
                problem.reason,
                problem.position,
                problem.length,
                problem.expected,
                problem.found
            ),
            ("syntax", 9, 0, vec!["a value"], None)
        );
        let error = domain::query::parse("status todo").unwrap_err();
        assert_eq!(
            query_problem(&error).detail.as_deref(),
            Some(
                "Expected = or != or < or <= or > or >= or ~ or !~ or IN or NOT IN or IS at position 7, found `todo`."
            )
        );
    }

    #[test]
    fn errors_about_a_field_name_it() {
        let problem = problem_of("text = outage");
        assert_eq!(
            (problem.reason, problem.field.as_deref(), problem.allowed),
            (
                "operator_not_allowed",
                Some("text"),
                vec!["~".to_owned(), "!~".to_owned()]
            )
        );
        assert_eq!(problem_of("due < tomorrow").reason, "invalid_date");
        assert_eq!(problem_of("x = \"open").reason, "unterminated_string");
        let problem = problem_of("ORDER BY text");
        assert_eq!(
            (problem.reason, problem.position, problem.length),
            ("not_sortable", 9, 4)
        );
    }
}
