//! Runs a query of the language (`domain::query`, L06-US01) over the
//! tickets of the current tenant.
//!
//! The tree is compiled to SQL with `QueryBuilder`: every value the member
//! wrote travels as a bind parameter, never in the SQL text, so a query
//! cannot inject anything. Row-level security keeps other tenants out, and
//! [`Context::visible_projects`] the projects the member does not see
//! (confidential ones included).
//!
//! - Stable identifiers: projects and nodes by key, types and statuses by
//!   key (whatever configuration), people by email or `currentUser()`.
//!   Custom fields by key: the published definitions of that key decide how
//!   a value is read; list values (which have no key) by id or by one of
//!   their labels, in any language.
//! - Dates and times compare as local times in the member's time zone:
//!   `created > -7d`, `due < endOfWeek()` (weeks start on Monday).
//! - Two-valued logic: a condition on an empty field is false, never
//!   unknown, so `NOT assignee = currentUser()` keeps unassigned tickets
//!   while `assignee != currentUser()` does not.
//! - Pages follow a cursor over the sort keys (empty values last), then the
//!   ticket id; a cursor only fits the order it came from.
//! - Text (`text ~`, `summary ~`, `description ~`): full text, typos
//!   corrected ([`crate::full_text`]). Without `ORDER BY`, a query searching
//!   text lists the best matches first.

use std::collections::HashMap;
use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::fields::{Kind, ListValue, Options};
use domain::query::{
    Clause, CompareOp, Condition, Expr, Field, Function, OrderKey, Period, Query, QueryError, QueryErrorKind,
    SystemField, TimeUnit, Value,
};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use sqlx::{Postgres, QueryBuilder, Row as _};
use uuid::Uuid;

use crate::full_text::{self, Audience, Scope, Texts};
use crate::issues::Issue;

/// Who searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context<'a> {
    pub account_id: Uuid,
    /// IANA name (`Europe/Paris`): the member's, else the tenant's.
    pub time_zone: &'a str,
    /// The projects the member sees; `None`: all of them (tenant admins).
    pub visible_projects: Option<&'a [Uuid]>,
    /// Members, or the portal: which comments the text search reads.
    pub audience: Audience,
}

/// One page of tickets.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPage {
    pub items: Vec<Issue>,
    /// Where the next page starts; `None` on the last one.
    pub next_cursor: Option<String>,
    /// How many tickets match, when asked for.
    pub total: Option<i64>,
}

/// Why a search did not run.
#[derive(Debug)]
pub enum SearchError {
    /// The query names something the tenant does not have, or that waits
    /// for a later work package, or gives a field a value of the wrong kind.
    Query(QueryError),
    /// The cursor is not one this query handed out.
    Cursor,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for SearchError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

impl From<QueryError> for SearchError {
    fn from(err: QueryError) -> Self {
        Self::Query(err)
    }
}

/// The time zone of `account` in the current tenant: theirs, else the
/// tenant's.
pub async fn time_zone_of(conn: &mut PgConnection, account: Uuid) -> Result<String, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT COALESCE((SELECT a.time_zone FROM account a WHERE a.id = $1),
                        (SELECT t.time_zone FROM tenant t WHERE t.id = app_current_tenant()),
                        'UTC') AS "time_zone!"
        "#,
        account
    )
    .fetch_one(conn)
    .await
}

/// A project of a page of results, with the configuration it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOfPage {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub config_id: Option<Uuid>,
}

/// The projects `ids`, in one query.
pub async fn projects_of(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<ProjectOfPage>, sqlx::Error> {
    sqlx::query_as!(
        ProjectOfPage,
        "SELECT id, key, name, config_id FROM node WHERE id = ANY($1)",
        ids
    )
    .fetch_all(conn)
    .await
}

/// The tickets matching `query`, `limit` of them after `cursor`, with the
/// total when `count` is set.
pub async fn search(
    conn: &mut PgConnection,
    query: &Query,
    context: &Context<'_>,
    cursor: Option<&str>,
    limit: usize,
    count: bool,
) -> Result<SearchPage, SearchError> {
    // For the rest of the transaction (L06-US03, measured on 100,000
    // tickets):
    // - no JIT: queries over many tickets look costly to the planner, which
    //   then compiles them, for seven times what they take to run;
    // - each query planned with its values: a prepared statement's generic
    //   plan would rebuild the text query for every row, and guess how many
    //   tickets it matches.
    sqlx::query!(
        r#"SELECT set_config('jit', 'off', true) AS "jit!", set_config('plan_cache_mode', 'force_custom_plan', true) AS "plans!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    let custom = custom_fields(conn, query).await?;
    let (searched, ranked) = searched_texts(query);
    let texts = full_text::prepare(conn, &searched).await?;
    let compiler = Compiler {
        context,
        custom: &custom,
        texts: &texts,
    };
    let sort = compiler.sort_keys(&query.order_by, &ranked)?;
    let order_text = order_fingerprint(query);
    let after = cursor
        .map(|c| decode_cursor(c, &order_text, sort.len()))
        .transpose()?;

    // The page first, ids and sort values only: what a ticket shows (the
    // names of its people) is read for the page alone.
    let mut qb = QueryBuilder::new("SELECT i.id");
    for (index, key) in sort.iter().enumerate() {
        qb.push(", (");
        key.push(&mut qb);
        qb.push(")::text AS s").push(index);
    }
    qb.push(" FROM issue i");
    if sort.iter().any(SortKey::needs_project) {
        qb.push(" JOIN node pn ON pn.id = i.project_id");
    }
    qb.push(" WHERE ");
    compiler.push_filter(&mut qb, query.filter.as_ref())?;
    if let Some(after) = &after {
        qb.push(" AND ");
        push_after(&mut qb, &sort, after);
    }
    qb.push(" ORDER BY ");
    for key in &sort {
        if key.nullable() {
            qb.push("(");
            key.push(&mut qb);
            qb.push(") IS NULL, ");
        }
        qb.push("(");
        key.push(&mut qb);
        qb.push(if key.descending { ") DESC, " } else { ") ASC, " });
    }
    qb.push("i.id ASC LIMIT ")
        .push_bind(i64::try_from(limit + 1).unwrap_or(i64::MAX));
    let rows = qb.build().fetch_all(&mut *conn).await?;

    let mut page = Vec::with_capacity(rows.len().min(limit));
    for row in rows.iter().take(limit) {
        let values = (0..sort.len())
            .map(|index| row.try_get::<Option<String>, _>(format!("s{index}").as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        page.push(CursorDoc {
            order: order_text.clone(),
            values,
            id: row.try_get("id")?,
        });
    }
    let next_cursor = if rows.len() > limit {
        page.last().map(encode_cursor)
    } else {
        None
    };
    let ids: Vec<Uuid> = page.iter().map(|doc| doc.id).collect();
    let mut found: HashMap<Uuid, Issue> = crate::issues::find_many(conn, &ids)
        .await?
        .into_iter()
        .map(|issue| (issue.id, issue))
        .collect();
    let items = ids.iter().filter_map(|id| found.remove(id)).collect();

    let total = if count {
        let mut qb = QueryBuilder::new("SELECT count(*) FROM issue i WHERE ");
        compiler.push_filter(&mut qb, query.filter.as_ref())?;
        Some(qb.build_query_scalar::<i64>().fetch_one(&mut *conn).await?)
    } else {
        None
    };
    Ok(SearchPage {
        items,
        next_cursor,
        total,
    })
}

/// A field of the configurations, as published under one key.
#[derive(Debug, Clone)]
struct CustomDef {
    item: Uuid,
    kind: Kind,
    options: Options,
}

/// The texts `query` searches (`~`, `!~`), and those whose matches it keeps
/// (`~`, or `!~` under a `NOT`, an even number of negations in all): what
/// relevance is measured on.
fn searched_texts(query: &Query) -> (Vec<String>, Vec<String>) {
    fn walk(expr: &Expr, negated: bool, all: &mut Vec<String>, kept: &mut Vec<String>) {
        match expr {
            Expr::And(left, right) | Expr::Or(left, right) => {
                walk(left, negated, all, kept);
                walk(right, negated, all, kept);
            }
            Expr::Not(inner) => walk(inner, !negated, all, kept),
            Expr::Clause(Clause {
                condition: Condition::Contains { negated: not, text },
                ..
            }) => {
                all.push(text.clone());
                // `!~` under a `NOT` keeps its matches, like `~` under none.
                if negated == *not {
                    kept.push(text.clone());
                }
            }
            Expr::Clause(_) => {}
        }
    }
    let (mut all, mut kept) = (Vec::new(), Vec::new());
    if let Some(filter) = &query.filter {
        walk(filter, false, &mut all, &mut kept);
    }
    all.sort();
    all.dedup();
    kept.sort();
    kept.dedup();
    (all, kept)
}

/// Every custom key `query` names.
fn custom_keys(query: &Query) -> Vec<String> {
    fn walk(expr: &Expr, keys: &mut Vec<String>) {
        match expr {
            Expr::And(left, right) | Expr::Or(left, right) => {
                walk(left, keys);
                walk(right, keys);
            }
            Expr::Not(inner) => walk(inner, keys),
            Expr::Clause(Clause {
                field: Field::Custom(key),
                ..
            }) => keys.push(key.clone()),
            Expr::Clause(_) => {}
        }
    }
    let mut keys = Vec::new();
    if let Some(filter) = &query.filter {
        walk(filter, &mut keys);
    }
    for key in &query.order_by {
        if let Field::Custom(name) = &key.field {
            keys.push(name.clone());
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// The published definitions of the custom fields `query` names, by key.
async fn custom_fields(
    conn: &mut PgConnection,
    query: &Query,
) -> Result<HashMap<String, Vec<CustomDef>>, sqlx::Error> {
    let keys = custom_keys(query);
    let mut defs: HashMap<String, Vec<CustomDef>> = HashMap::new();
    if keys.is_empty() {
        return Ok(defs);
    }
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (f.item_id) f.item_id, f.key, f.kind, f.options
        FROM field_def f
        JOIN project_config_version v ON v.id = f.version_id AND v.state = 'published'
        WHERE f.key = ANY($1)
        ORDER BY f.item_id
        "#,
        &keys
    )
    .fetch_all(conn)
    .await?;
    for row in rows {
        if let Some(kind) = Kind::parse(&row.kind) {
            defs.entry(row.key).or_default().push(CustomDef {
                item: row.item_id,
                kind,
                options: crate::fields::options_of(row.options),
            });
        }
    }
    Ok(defs)
}

/// A sort expression of the query, with the SQL type its cursor value
/// takes back.
#[derive(Debug, Clone)]
struct SortKey {
    expr: SortExpr,
    descending: bool,
}

#[derive(Debug, Clone)]
enum SortExpr {
    /// Fixed SQL over the ticket `i`, of SQL type `.1`.
    Sql(&'static str, &'static str),
    /// A custom field read under several item ids: the first one set.
    Custom { items: Vec<Uuid>, kind: CustomSort },
    /// How well the ticket matches the searched texts.
    Relevance {
        texts: Vec<Vec<domain::text_search::Term>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomSort {
    Number,
    Date,
    DateTime,
    Text,
}

/// Sort expressions over columns that are never empty: they sort without
/// the "empty last" prefix, which lets PostgreSQL use plain comparisons.
const NEVER_EMPTY: [&str; 6] = [
    PROJECT_KEY_SQL,
    "i.key_number",
    PRIORITY_RANK_SQL,
    "lower(i.summary)",
    "i.created_at",
    "i.updated_at",
];

impl SortKey {
    /// Whether the expression can be empty (`NULL`).
    fn nullable(&self) -> bool {
        match &self.expr {
            SortExpr::Sql(sql, _) => !NEVER_EMPTY.contains(sql),
            SortExpr::Custom { .. } => true,
            SortExpr::Relevance { .. } => false,
        }
    }

    /// Whether it reads the ticket's project (`pn`, joined by the page query).
    fn needs_project(&self) -> bool {
        matches!(self.expr, SortExpr::Sql(PROJECT_KEY_SQL, _))
    }

    fn cast(&self) -> &'static str {
        match &self.expr {
            SortExpr::Sql(_, cast) => cast,
            SortExpr::Relevance { .. } => "real",
            SortExpr::Custom { kind, .. } => match kind {
                CustomSort::Number => "numeric",
                CustomSort::Date => "date",
                CustomSort::DateTime => "timestamptz",
                CustomSort::Text => "text",
            },
        }
    }

    fn push(&self, qb: &mut QueryBuilder<Postgres>) {
        match &self.expr {
            SortExpr::Sql(sql, _) => {
                qb.push(*sql);
            }
            SortExpr::Relevance { texts } => {
                let texts: Vec<&[domain::text_search::Term]> = texts.iter().map(Vec::as_slice).collect();
                full_text::push_relevance(qb, &texts);
            }
            SortExpr::Custom { items, kind } => {
                qb.push("COALESCE(");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        qb.push(", ");
                    }
                    let (typeof_, cast) = match kind {
                        CustomSort::Number => ("number", "::numeric"),
                        CustomSort::Date => ("string", "::date"),
                        CustomSort::DateTime => ("string", "::timestamptz"),
                        CustomSort::Text => ("", ""),
                    };
                    if typeof_.is_empty() {
                        qb.push("lower(i.fields ->> ")
                            .push_bind(item.to_string())
                            .push(")");
                    } else {
                        qb.push("(CASE WHEN jsonb_typeof(i.fields -> ")
                            .push_bind(item.to_string())
                            .push(") = '")
                            .push(typeof_)
                            .push("' THEN (i.fields ->> ")
                            .push_bind(item.to_string())
                            .push(")")
                            .push(cast)
                            .push(" END)");
                    }
                }
                qb.push(")");
            }
        }
    }
}

/// The project's key, through the join the page query adds (`pn`).
const PROJECT_KEY_SQL: &str = "pn.key";
const PRIORITY_RANK_SQL: &str = "CASE i.priority WHEN 'lowest' THEN 1 WHEN 'low' THEN 2 WHEN 'medium' THEN 3 \
                                 WHEN 'high' THEN 4 ELSE 5 END";

/// The rank of a priority: `lowest` 1 to `highest` 5.
fn priority_rank(priority: &str) -> i32 {
    match priority {
        "lowest" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        _ => 5,
    }
}

/// The canonical `ORDER BY` of `query`: what a cursor is tied to.
fn order_fingerprint(query: &Query) -> String {
    Query {
        filter: None,
        order_by: query.order_by.clone(),
    }
    .to_string()
}

/// What a cursor holds: the order it came from, the sort values of the last
/// ticket of its page (as text) and that ticket's id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorDoc {
    #[serde(rename = "o")]
    order: String,
    #[serde(rename = "v")]
    values: Vec<Option<String>>,
    id: Uuid,
}

fn encode_cursor(doc: &CursorDoc) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(doc).unwrap_or_default())
}

fn decode_cursor(cursor: &str, order: &str, keys: usize) -> Result<CursorDoc, SearchError> {
    URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorDoc>(&bytes).ok())
        .filter(|doc| doc.order == order && doc.values.len() == keys)
        .ok_or(SearchError::Cursor)
}

/// The rows after `after` in the order of `sort`, empty values last, then
/// by id.
fn push_after(qb: &mut QueryBuilder<Postgres>, sort: &[SortKey], after: &CursorDoc) {
    qb.push("(");
    for position in 0..=sort.len() {
        if position > 0 {
            qb.push(" OR ");
        }
        qb.push("(");
        for (key, value) in sort.iter().zip(&after.values).take(position) {
            // The same value as the cursor's row.
            qb.push("(");
            key.push(qb);
            match value {
                None => qb.push(") IS NULL AND "),
                Some(value) => qb
                    .push(") = ")
                    .push_bind(value.clone())
                    .push("::")
                    .push(key.cast())
                    .push(" AND "),
            };
        }
        match sort.get(position) {
            None => {
                qb.push("i.id > ").push_bind(after.id);
            }
            Some(key) => match &after.values[position] {
                // Nothing sorts after an empty value but other empty ones.
                None => {
                    qb.push("false");
                }
                Some(value) => {
                    qb.push("(");
                    if key.nullable() {
                        qb.push("(");
                        key.push(qb);
                        qb.push(") IS NULL OR ");
                    }
                    qb.push("(");
                    key.push(qb);
                    qb.push(if key.descending { ") < " } else { ") > " })
                        .push_bind(value.clone())
                        .push("::")
                        .push(key.cast())
                        .push(")");
                }
            },
        }
        qb.push(")");
    }
    qb.push(")");
}

struct Compiler<'a> {
    context: &'a Context<'a>,
    custom: &'a HashMap<String, Vec<CustomDef>>,
    texts: &'a Texts,
}

/// The values of a condition (`IN` lists and single comparisons alike).
fn values_of(condition: &Condition) -> &[Value] {
    match condition {
        Condition::Compare(_, value) => std::slice::from_ref(value),
        Condition::In { values, .. } => values,
        Condition::Contains { .. } | Condition::Empty { .. } => &[],
    }
}

/// Whether a condition keeps the tickets matching its values (`=`, `IN`)
/// or those not matching them (`!=`, `NOT IN`).
const fn negated(condition: &Condition) -> bool {
    matches!(
        condition,
        Condition::Compare(CompareOp::NotEq, _)
            | Condition::In { negated: true, .. }
            | Condition::Contains { negated: true, .. }
            | Condition::Empty { negated: true }
    )
}

fn texts(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| match value {
            Value::Text(text) => Some(text.clone()),
            Value::Number(number) => Some(number.clone()),
            _ => None,
        })
        .collect()
}

fn not_allowed(clause: &Clause, kind: QueryErrorKind) -> QueryError {
    QueryError::new(clause.span, kind)
}

fn value_not_allowed(clause: &Clause, value: &Value) -> QueryError {
    not_allowed(
        clause,
        QueryErrorKind::ValueNotAllowed {
            field: clause.field.name().to_owned(),
            value: value.to_string(),
        },
    )
}

fn operator_not_allowed(clause: &Clause, allowed: &[&str]) -> QueryError {
    not_allowed(
        clause,
        QueryErrorKind::OperatorNotAllowed {
            field: clause.field.name().to_owned(),
            operator: clause.condition.operator().to_owned(),
            allowed: allowed.iter().map(|op| (*op).to_owned()).collect(),
        },
    )
}

/// The ids of the list values of `options` that `text` names: by id, or by
/// one of their labels in any language, at any level.
fn list_value_ids(options: &Options, text: &str) -> Vec<String> {
    fn walk(values: &[ListValue], text: &str, found: &mut Vec<String>) {
        for value in values {
            let id = Uuid::from_u128(value.id);
            if id.to_string() == text.to_ascii_lowercase()
                || value
                    .labels
                    .values()
                    .any(|label| label.to_lowercase() == text.to_lowercase())
            {
                found.push(id.to_string());
            }
            walk(&value.children, text, found);
        }
    }
    let mut found = Vec::new();
    walk(options.values.as_deref().unwrap_or_default(), text, &mut found);
    found
}

impl Compiler<'_> {
    /// `WHERE` content: the projects the member sees, and the filter.
    fn push_filter(&self, qb: &mut QueryBuilder<Postgres>, filter: Option<&Expr>) -> Result<(), QueryError> {
        match self.context.visible_projects {
            Some(projects) => {
                qb.push("i.project_id = ANY(")
                    .push_bind(projects.to_vec())
                    .push(")");
            }
            None => {
                qb.push("true");
            }
        }
        if let Some(filter) = filter {
            qb.push(" AND ");
            self.push_expr(qb, filter, false)?;
        }
        Ok(())
    }

    /// `expr`; `negated` when a `NOT` is above it.
    fn push_expr(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        expr: &Expr,
        negated: bool,
    ) -> Result<(), QueryError> {
        match expr {
            Expr::And(left, right) | Expr::Or(left, right) => {
                qb.push("(");
                self.push_expr(qb, left, negated)?;
                qb.push(if matches!(expr, Expr::And(..)) {
                    " AND "
                } else {
                    " OR "
                });
                self.push_expr(qb, right, negated)?;
                qb.push(")");
            }
            Expr::Not(inner) => {
                qb.push("NOT ");
                self.push_expr(qb, inner, true)?;
            }
            // Never unknown: an empty field fails the condition. `WHERE`, AND
            // and OR already treat unknown as false; only NOT would turn it
            // into true, so the condition is made false there. Elsewhere it
            // stays bare, for the indexes.
            Expr::Clause(clause) if negated => {
                qb.push("COALESCE((");
                self.push_clause(qb, clause)?;
                qb.push("), false)");
            }
            Expr::Clause(clause) => self.push_clause(qb, clause)?,
        }
        Ok(())
    }

    fn push_clause(&self, qb: &mut QueryBuilder<Postgres>, clause: &Clause) -> Result<(), QueryError> {
        let field = match &clause.field {
            Field::System(field) => *field,
            Field::Custom(key) => return self.push_custom(qb, clause, key),
        };
        if field.is_pending() {
            return Err(not_allowed(
                clause,
                QueryErrorKind::Unsupported(field.as_str().to_owned()),
            ));
        }
        let condition = &clause.condition;
        if negated(condition) {
            qb.push("NOT ");
        }
        qb.push("(");
        match (field, condition) {
            (
                SystemField::Summary | SystemField::Description | SystemField::Text,
                Condition::Contains { text, .. },
            ) => {
                let scope = match field {
                    SystemField::Summary => Scope::Summary,
                    SystemField::Description => Scope::Description,
                    _ => Scope::All,
                };
                full_text::push_matches(qb, scope, self.texts.terms(text), self.context.audience);
            }
            (SystemField::Description, Condition::Empty { .. }) => {
                qb.push("i.description IS NULL");
            }
            (
                SystemField::Assignee
                | SystemField::Reporter
                | SystemField::Parent
                | SystemField::Due
                | SystemField::Start,
                Condition::Empty { .. },
            ) => {
                qb.push(match field {
                    SystemField::Assignee => "i.assignee_id",
                    SystemField::Reporter => "i.reporter_id",
                    SystemField::Parent => "i.parent_id",
                    SystemField::Due => "i.due_date",
                    _ => "i.start_date",
                })
                .push(" IS NULL");
            }
            (
                SystemField::Created | SystemField::Updated | SystemField::Due | SystemField::Start,
                Condition::Compare(op, value),
            ) => {
                qb.push(match field {
                    SystemField::Created => "(i.created_at AT TIME ZONE ",
                    SystemField::Updated => "(i.updated_at AT TIME ZONE ",
                    SystemField::Due => "(i.due_date::timestamp",
                    _ => "(i.start_date::timestamp",
                });
                if matches!(field, SystemField::Created | SystemField::Updated) {
                    qb.push_bind(self.context.time_zone.to_owned());
                }
                qb.push(") ").push(positive(*op)).push(" ");
                self.push_instant(qb, clause, value)?;
            }
            (SystemField::Priority, Condition::Compare(op, Value::Text(priority))) if op.is_ordering() => {
                qb.push(PRIORITY_RANK_SQL)
                    .push(" ")
                    .push(op.as_str())
                    .push(" ")
                    .push_bind(priority_rank(&priority.to_ascii_lowercase()));
            }
            (_, Condition::Compare(..) | Condition::In { .. }) => {
                self.push_identifiers(qb, clause, field)?;
            }
            _ => return Err(operator_not_allowed(clause, &[])),
        }
        qb.push(")");
        Ok(())
    }

    /// `=`, `!=`, `IN`, `NOT IN` on a field of identifiers: whether the
    /// ticket's value is one of those named (the negation is outside).
    fn push_identifiers(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        clause: &Clause,
        field: SystemField,
    ) -> Result<(), QueryError> {
        let values = values_of(&clause.condition);
        let words = texts(values);
        let upper: Vec<String> = words.iter().map(|w| w.to_uppercase()).collect();
        let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        match field {
            SystemField::Key => {
                qb.push("i.key = ANY(")
                    .push_bind(upper.clone())
                    .push(") OR EXISTS (SELECT 1 FROM issue_key_alias a WHERE a.issue_id = i.id AND a.key = ANY(")
                    .push_bind(upper)
                    .push("))");
            }
            SystemField::Project => {
                qb.push("i.project_id IN (SELECT n.id FROM node n WHERE n.key = ANY(")
                    .push_bind(upper)
                    .push("))");
                for value in values {
                    if let Value::Function(Function::DescendantsOf(key)) = value {
                        qb.push(
                            " OR i.project_id IN (SELECT c.descendant_id FROM node_closure c \
                             JOIN node n ON n.id = c.ancestor_id WHERE n.key = ",
                        )
                        .push_bind(key.to_uppercase())
                        .push(")");
                    }
                }
            }
            SystemField::Type => {
                qb.push("i.type_id IN (SELECT t.item_id FROM issue_type t WHERE t.key = ANY(")
                    .push_bind(lower)
                    .push("))");
            }
            SystemField::Status => {
                qb.push("i.status_id IN (SELECT s.item_id FROM workflow_status s WHERE s.key = ANY(")
                    .push_bind(lower)
                    .push("))");
            }
            SystemField::Priority => {
                qb.push("i.priority = ANY(").push_bind(lower).push(")");
            }
            SystemField::Assignee | SystemField::Reporter => {
                let column = if field == SystemField::Assignee {
                    "i.assignee_id"
                } else {
                    "i.reporter_id"
                };
                let me: Vec<Uuid> = values
                    .iter()
                    .filter(|v| matches!(v, Value::Function(Function::CurrentUser)))
                    .map(|_| self.context.account_id)
                    .collect();
                qb.push(column)
                    .push(" = ANY(")
                    .push_bind(me)
                    .push(") OR ")
                    .push(column)
                    .push(" IN (SELECT a.id FROM account a WHERE lower(a.email) = ANY(")
                    .push_bind(lower)
                    .push("))");
            }
            SystemField::Parent => {
                qb.push("i.parent_id IN (SELECT p.id FROM issue p WHERE p.key = ANY(")
                    .push_bind(upper)
                    .push("))");
            }
            _ => return Err(operator_not_allowed(clause, &[])),
        }
        Ok(())
    }

    /// An instant as a local time of the member's time zone.
    fn push_instant(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        clause: &Clause,
        value: &Value,
    ) -> Result<(), QueryError> {
        let now = |qb: &mut QueryBuilder<Postgres>| {
            qb.push("(now() AT TIME ZONE ")
                .push_bind(self.context.time_zone.to_owned())
                .push(")");
        };
        match value {
            Value::Text(date) => {
                qb.push_bind(date.clone()).push("::timestamp");
            }
            Value::Relative(relative) => {
                let unit = match relative.unit {
                    TimeUnit::Years => "years",
                    TimeUnit::Months => "months",
                    TimeUnit::Weeks => "weeks",
                    TimeUnit::Days => "days",
                    TimeUnit::Hours => "hours",
                    TimeUnit::Minutes => "mins",
                };
                qb.push("(");
                now(qb);
                qb.push(" + make_interval(")
                    .push(unit)
                    .push(" => ")
                    .push_bind(i32::try_from(relative.amount).unwrap_or(0))
                    .push("))");
            }
            Value::Function(Function::Now) => now(qb),
            Value::Function(Function::StartOf(period) | Function::EndOf(period)) => {
                let unit = period_unit(*period);
                qb.push("(date_trunc('").push(unit).push("', ");
                now(qb);
                qb.push(")");
                if matches!(value, Value::Function(Function::EndOf(_))) {
                    qb.push(" + interval '1 ")
                        .push(unit)
                        .push("' - interval '1 microsecond'");
                }
                qb.push(")");
            }
            other => return Err(value_not_allowed(clause, other)),
        }
        Ok(())
    }

    /// A condition on a custom field: true when one of its published
    /// definitions holds it.
    fn push_custom(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        clause: &Clause,
        key: &str,
    ) -> Result<(), QueryError> {
        let defs = self
            .custom
            .get(key)
            .filter(|defs| !defs.is_empty())
            .ok_or_else(|| not_allowed(clause, QueryErrorKind::UnknownField(key.to_owned())))?;
        // The definitions this condition makes sense for.
        let mut fitting = Vec::new();
        let mut refusal = None;
        for def in defs {
            match check_custom(clause, def) {
                Ok(()) => fitting.push(def),
                Err(error) => refusal = refusal.or(Some(error)),
            }
        }
        if fitting.is_empty() {
            return Err(refusal.unwrap_or_else(|| operator_not_allowed(clause, &[])));
        }
        if negated(&clause.condition) {
            qb.push("NOT ");
        }
        qb.push("(");
        for (index, def) in fitting.iter().enumerate() {
            if index > 0 {
                qb.push(" OR ");
            }
            qb.push("COALESCE((");
            self.push_custom_def(qb, clause, def)?;
            qb.push("), false)");
        }
        qb.push(")");
        Ok(())
    }

    /// The condition for one definition, without its negation.
    fn push_custom_def(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        clause: &Clause,
        def: &CustomDef,
    ) -> Result<(), QueryError> {
        let item = def.item.to_string();
        let json = |qb: &mut QueryBuilder<Postgres>| {
            qb.push("(i.fields -> ").push_bind(item.clone()).push(")");
        };
        let text = |qb: &mut QueryBuilder<Postgres>| {
            qb.push("(i.fields ->> ").push_bind(item.clone()).push(")");
        };
        let condition = &clause.condition;
        let values = values_of(condition);
        if let Condition::Empty { .. } = condition {
            qb.push("COALESCE(");
            json(qb);
            qb.push(", 'null'::jsonb) IN ('null'::jsonb, '\"\"'::jsonb, '[]'::jsonb)");
            return Ok(());
        }
        match def.kind {
            Kind::Number => {
                qb.push("(CASE WHEN jsonb_typeof(");
                json(qb);
                qb.push(") = 'number' THEN ");
                text(qb);
                qb.push("::numeric END)");
                match condition {
                    Condition::Compare(op, _) => {
                        let number = texts(values).into_iter().next().unwrap_or_default();
                        qb.push(" ")
                            .push(positive(*op))
                            .push(" ")
                            .push_bind(number)
                            .push("::numeric");
                    }
                    _ => {
                        qb.push(" = ANY(").push_bind(texts(values)).push("::numeric[])");
                    }
                }
            }
            Kind::ShortText | Kind::RichText | Kind::Url => match condition {
                Condition::Contains { text: needle, .. } => {
                    text(qb);
                    qb.push(" ILIKE ")
                        .push_bind(crate::invitations::like_pattern(needle));
                }
                _ => {
                    qb.push("lower(");
                    text(qb);
                    qb.push(") = ANY(")
                        .push_bind(texts(values).iter().map(|t| t.to_lowercase()).collect::<Vec<_>>())
                        .push(")");
                }
            },
            Kind::Checkbox => {
                let wanted = texts(values)
                    .first()
                    .is_some_and(|t| t.eq_ignore_ascii_case("true"));
                qb.push("COALESCE(");
                json(qb);
                qb.push(", 'false'::jsonb) = to_jsonb(")
                    .push_bind(wanted)
                    .push(")");
            }
            Kind::Date | Kind::DateTime => {
                let Condition::Compare(op, value) = condition else {
                    return Err(operator_not_allowed(clause, &[]));
                };
                qb.push("((CASE WHEN jsonb_typeof(");
                json(qb);
                qb.push(") = 'string' THEN ");
                text(qb);
                if def.kind == Kind::Date {
                    qb.push("::date::timestamp END)");
                } else {
                    qb.push("::timestamptz END) AT TIME ZONE ")
                        .push_bind(self.context.time_zone.to_owned());
                }
                qb.push(") ").push(positive(*op)).push(" ");
                self.push_instant(qb, clause, value)?;
            }
            Kind::SingleSelect | Kind::MultiSelect | Kind::CascadingSelect => {
                let ids: Vec<String> = texts(values)
                    .iter()
                    .flat_map(|t| list_value_ids(&def.options, t))
                    .collect();
                json(qb);
                qb.push(" ?| ").push_bind(ids).push("::text[]");
            }
            Kind::Labels => match condition {
                Condition::Contains { text: needle, .. } => {
                    qb.push("EXISTS (SELECT 1 FROM jsonb_array_elements_text(CASE WHEN jsonb_typeof(");
                    json(qb);
                    qb.push(") = 'array' THEN ");
                    json(qb);
                    qb.push(" ELSE '[]'::jsonb END) e WHERE e ILIKE ")
                        .push_bind(crate::invitations::like_pattern(needle))
                        .push(")");
                }
                _ => {
                    json(qb);
                    qb.push(" ?| ").push_bind(texts(values)).push("::text[]");
                }
            },
            Kind::User => {
                let me: Vec<String> = values
                    .iter()
                    .filter(|v| matches!(v, Value::Function(Function::CurrentUser)))
                    .map(|_| self.context.account_id.to_string())
                    .collect();
                let emails: Vec<String> = texts(values).iter().map(|t| t.to_lowercase()).collect();
                qb.push("(");
                json(qb);
                qb.push(" ?| ").push_bind(me).push("::text[] OR ");
                json(qb);
                qb.push(" ?| ARRAY(SELECT a.id::text FROM account a WHERE lower(a.email) = ANY(")
                    .push_bind(emails)
                    .push(")))");
            }
            Kind::Group => {
                json(qb);
                qb.push(" ?| ARRAY(SELECT g.id::text FROM \"group\" g WHERE lower(g.name) = ANY(")
                    .push_bind(texts(values).iter().map(|t| t.to_lowercase()).collect::<Vec<_>>())
                    .push("))");
            }
            Kind::IssueLink => {
                json(qb);
                qb.push(" ?| ARRAY(SELECT p.id::text FROM issue p WHERE p.key = ANY(")
                    .push_bind(texts(values).iter().map(|t| t.to_uppercase()).collect::<Vec<_>>())
                    .push("))");
            }
        }
        Ok(())
    }

    /// The SQL sort keys of `order_by`; newest first by default.
    /// The sort keys of `order_by`; without one, the newest tickets first,
    /// after the best matches of the `ranked` texts when there are some.
    fn sort_keys(&self, order_by: &[OrderKey], ranked: &[String]) -> Result<Vec<SortKey>, QueryError> {
        let key = |sql, cast, descending| SortKey {
            expr: SortExpr::Sql(sql, cast),
            descending,
        };
        if order_by.is_empty() {
            let mut keys = Vec::new();
            if !ranked.is_empty() {
                keys.push(SortKey {
                    expr: SortExpr::Relevance {
                        texts: ranked
                            .iter()
                            .map(|text| self.texts.terms(text).to_vec())
                            .collect(),
                    },
                    descending: true,
                });
            }
            keys.push(key("i.created_at", "timestamptz", true));
            return Ok(keys);
        }
        let mut keys = Vec::new();
        for order in order_by {
            let d = order.descending;
            let field = match &order.field {
                Field::System(field) => *field,
                Field::Custom(name) => {
                    keys.push(self.custom_sort(order, name)?);
                    continue;
                }
            };
            if field.is_pending() {
                return Err(QueryError::new(
                    order.span,
                    QueryErrorKind::Unsupported(field.as_str().to_owned()),
                ));
            }
            match field {
                SystemField::Key => {
                    keys.push(key(PROJECT_KEY_SQL, "text", d));
                    keys.push(key("i.key_number", "integer", d));
                }
                SystemField::Project => keys.push(key(PROJECT_KEY_SQL, "text", d)),
                SystemField::Type => keys.push(key(
                    "(SELECT t.key FROM issue_type t WHERE t.item_id = i.type_id ORDER BY t.updated_at DESC, t.id LIMIT 1)",
                    "text",
                    d,
                )),
                SystemField::Status => {
                    keys.push(key(
                        "(SELECT CASE s.category WHEN 'todo' THEN 1 WHEN 'in_progress' THEN 2 ELSE 3 END \
                         FROM workflow_status s WHERE s.item_id = i.status_id ORDER BY s.updated_at DESC, s.id LIMIT 1)",
                        "integer",
                        d,
                    ));
                    keys.push(key(
                        "(SELECT s.key FROM workflow_status s WHERE s.item_id = i.status_id \
                         ORDER BY s.updated_at DESC, s.id LIMIT 1)",
                        "text",
                        d,
                    ));
                }
                SystemField::Priority => keys.push(key(PRIORITY_RANK_SQL, "integer", d)),
                SystemField::Assignee => keys.push(key("lower(app_member_name(i.assignee_id))", "text", d)),
                SystemField::Reporter => keys.push(key("lower(app_member_name(i.reporter_id))", "text", d)),
                SystemField::Summary => keys.push(key("lower(i.summary)", "text", d)),
                SystemField::Created => keys.push(key("i.created_at", "timestamptz", d)),
                SystemField::Updated => keys.push(key("i.updated_at", "timestamptz", d)),
                SystemField::Due => keys.push(key("i.due_date", "date", d)),
                SystemField::Start => keys.push(key("i.start_date", "date", d)),
                SystemField::Parent => keys.push(key("(SELECT p.key FROM issue p WHERE p.id = i.parent_id)", "text", d)),
                SystemField::Description | SystemField::Text | SystemField::Sprint | SystemField::FixVersion => {
                    return Err(QueryError::new(
                        order.span,
                        QueryErrorKind::NotSortable(field.as_str().to_owned()),
                    ));
                }
            }
        }
        Ok(keys)
    }

    fn custom_sort(&self, order: &OrderKey, name: &str) -> Result<SortKey, QueryError> {
        let defs = self
            .custom
            .get(name)
            .filter(|defs| !defs.is_empty())
            .ok_or_else(|| QueryError::new(order.span, QueryErrorKind::UnknownField(name.to_owned())))?;
        let kind_of = |kind| match kind {
            Kind::Number => CustomSort::Number,
            Kind::Date => CustomSort::Date,
            Kind::DateTime => CustomSort::DateTime,
            _ => CustomSort::Text,
        };
        let first = kind_of(defs[0].kind);
        let kind = if defs.iter().all(|def| kind_of(def.kind) == first) {
            first
        } else {
            CustomSort::Text
        };
        Ok(SortKey {
            expr: SortExpr::Custom {
                items: defs.iter().map(|def| def.item).collect(),
                kind,
            },
            descending: order.descending,
        })
    }
}

/// `!=` is the negation of `=`: written outside, the comparison inside.
const fn positive(op: CompareOp) -> &'static str {
    match op {
        CompareOp::NotEq => "=",
        other => other.as_str(),
    }
}

const fn period_unit(period: Period) -> &'static str {
    match period {
        Period::Day => "day",
        Period::Week => "week",
        Period::Month => "month",
        Period::Year => "year",
    }
}

/// Whether the condition fits a custom field of this definition.
fn check_custom(clause: &Clause, def: &CustomDef) -> Result<(), QueryError> {
    let condition = &clause.condition;
    let values = values_of(condition);
    if matches!(condition, Condition::Empty { .. }) {
        return Ok(());
    }
    let allowed: &[&str] = match def.kind {
        Kind::Number => &[
            "=",
            "!=",
            "<",
            "<=",
            ">",
            ">=",
            "IN",
            "NOT IN",
            "IS EMPTY",
            "IS NOT EMPTY",
        ],
        Kind::Date | Kind::DateTime => &["=", "!=", "<", "<=", ">", ">=", "IS EMPTY", "IS NOT EMPTY"],
        Kind::ShortText | Kind::RichText | Kind::Url | Kind::Labels => {
            &["=", "!=", "IN", "NOT IN", "~", "!~", "IS EMPTY", "IS NOT EMPTY"]
        }
        Kind::Checkbox => &["=", "!="],
        Kind::SingleSelect
        | Kind::MultiSelect
        | Kind::CascadingSelect
        | Kind::User
        | Kind::Group
        | Kind::IssueLink => &["=", "!=", "IN", "NOT IN", "IS EMPTY", "IS NOT EMPTY"],
    };
    if !allowed.contains(&condition.operator()) {
        return Err(operator_not_allowed(clause, allowed));
    }
    for value in values {
        let instant = matches!(def.kind, Kind::Date | Kind::DateTime);
        let fits = match value {
            Value::Number(_) => !instant && def.kind != Kind::Checkbox,
            Value::Text(text) if instant => domain::query::is_date_literal(text),
            Value::Text(flag) if def.kind == Kind::Checkbox => {
                flag.eq_ignore_ascii_case("true") || flag.eq_ignore_ascii_case("false")
            }
            Value::Text(_) => def.kind != Kind::Number,
            Value::Relative(_) => instant,
            Value::Function(function) => {
                (instant && function.is_instant())
                    || (def.kind == Kind::User && *function == Function::CurrentUser)
            }
        };
        if !fits {
            return Err(value_not_allowed(clause, value));
        }
    }
    Ok(())
}

/// For tests and logs: the SQL `query` compiles to, without running it.
pub fn sql_of(query: &Query, context: &Context<'_>) -> Result<String, QueryError> {
    let custom = HashMap::new();
    let (searched, ranked) = searched_texts(query);
    let texts = Texts::split(searched.iter().map(String::as_str));
    let compiler = Compiler {
        context,
        custom: &custom,
        texts: &texts,
    };
    let mut qb = QueryBuilder::new("SELECT i.id FROM issue i WHERE ");
    compiler.push_filter(&mut qb, query.filter.as_ref())?;
    let sort = compiler.sort_keys(&query.order_by, &ranked)?;
    let mut sql = qb.into_string();
    let _ = write!(sql, " /* {} sort keys */", sort.len());
    Ok(sql)
}

#[cfg(test)]
mod tests {
    use domain::query::{Span, parse};

    use super::*;

    fn context() -> Context<'static> {
        Context {
            account_id: Uuid::nil(),
            time_zone: "Europe/Paris",
            visible_projects: None,
            audience: Audience::Members,
        }
    }

    fn sql(query: &str) -> String {
        sql_of(&parse(query).unwrap(), &context()).unwrap()
    }

    #[test]
    fn values_travel_as_parameters_never_in_the_sql() {
        for query in [
            "summary ~ \"x'; DROP TABLE issue; --\"",
            "key = \"PLAT-1' OR '1'='1\"",
            "assignee = \"a@b.c'--\"",
            "project IN descendantsOf(\"PORTF'); DELETE FROM node; --\")",
            "status IN (\"todo')--\", done)",
            "due < \"2026-11-20\" AND created > -7d",
        ] {
            let sql = sql(query);
            for fragment in ["DROP", "DELETE", "'1'='1", "2026-11-20", "todo')", "a@b.c"] {
                assert!(!sql.contains(fragment), "{fragment} inlined in {sql}");
            }
            assert!(sql.contains("$1"), "{sql}");
        }
    }

    #[test]
    fn each_condition_compiles_to_its_columns() {
        assert!(sql("assignee = currentUser()").contains("i.assignee_id = ANY($1)"));
        assert!(sql("assignee IS EMPTY").contains("i.assignee_id IS NULL"));
        assert!(
            sql("status != done").contains("NOT (i.status_id IN (SELECT s.item_id FROM workflow_status s")
        );
        assert!(sql("project IN descendantsOf(\"PORTF-HR\")").contains("node_closure c"));
        assert!(sql("created > -7d").contains(
            "(i.created_at AT TIME ZONE $1) > ((now() AT TIME ZONE $2) + make_interval(days => $3))"
        ));
        assert!(sql("due < endOfWeek()").contains(
            "(i.due_date::timestamp) < (date_trunc('week', (now() AT TIME ZONE $1)) + interval '1 week' - interval '1 microsecond')"
        ));
        assert!(sql("priority >= high").contains("WHEN 'high' THEN 4 ELSE 5 END >= $1"));
        assert!(sql("text ~ outage").contains(
            "(i.search_vector @@ (to_tsquery('mlbpp_fr', $1) || to_tsquery('mlbpp_en', $2)) OR i.id IN (SELECT c.issue_id FROM comment c"
        ));
        assert!(sql("summary ~ outage").contains("(i.search_vector @@ (to_tsquery('mlbpp_fr', $1)"));
        assert!(sql("NOT key = PLAT-1").contains("NOT COALESCE(("));
    }

    #[test]
    fn without_order_a_text_search_lists_the_best_matches_first() {
        assert!(sql("text ~ outage").ends_with("/* 2 sort keys */"));
        assert!(sql("status = todo OR summary ~ outage").ends_with("/* 2 sort keys */"));
        // Nothing to rank on: the newest tickets first.
        for query in [
            "status = todo",
            "text !~ outage",
            "NOT text ~ outage",
            "text ~ outage ORDER BY due",
        ] {
            assert!(sql(query).ends_with("/* 1 sort keys */"), "{query}");
        }
    }

    #[test]
    fn the_texts_kept_are_those_under_no_negation() {
        let query =
            parse("text ~ a AND NOT (summary ~ b OR description !~ c) AND text !~ d OR NOT NOT text ~ e")
                .unwrap();
        let (all, kept) = searched_texts(&query);
        assert_eq!(all, ["a", "b", "c", "d", "e"]);
        assert_eq!(kept, ["a", "c", "e"]);
    }

    #[test]
    fn the_projects_in_sight_come_first() {
        let projects = [Uuid::nil()];
        let limited = Context {
            visible_projects: Some(&projects),
            ..context()
        };
        let sql = sql_of(&parse("status = todo").unwrap(), &limited).unwrap();
        assert!(
            sql.starts_with("SELECT i.id FROM issue i WHERE i.project_id = ANY($1) AND "),
            "{sql}"
        );
    }

    #[test]
    fn sprints_and_versions_wait_for_l09() {
        for query in ["sprint IN openSprints()", "fixVersion = \"2.1\""] {
            let error = sql_of(&parse(query).unwrap(), &context()).unwrap_err();
            assert_eq!(error.code(), "unsupported", "{query}");
            assert_eq!(error.span, Span::new(0, query.chars().count()));
        }
    }

    #[test]
    fn a_custom_field_unknown_to_the_configurations_is_refused() {
        let error = sql_of(&parse("severity = high").unwrap(), &context()).unwrap_err();
        assert_eq!(error.kind, QueryErrorKind::UnknownField("severity".to_owned()));
    }

    #[test]
    fn list_values_are_found_by_id_or_label_in_any_language() {
        let options = Options {
            values: Some(vec![ListValue {
                id: 7,
                labels: [
                    ("fr".to_owned(), "Critique".to_owned()),
                    ("en".to_owned(), "Critical".to_owned()),
                ]
                .into(),
                active: true,
                children: vec![ListValue {
                    id: 8,
                    labels: [("en".to_owned(), "Outage".to_owned())].into(),
                    active: true,
                    children: vec![],
                }],
            }]),
            ..Options::default()
        };
        let id = |n: u128| Uuid::from_u128(n).to_string();
        assert_eq!(list_value_ids(&options, "critique"), [id(7)]);
        assert_eq!(list_value_ids(&options, "CRITICAL"), [id(7)]);
        assert_eq!(list_value_ids(&options, "outage"), [id(8)]);
        assert_eq!(list_value_ids(&options, &id(8).to_uppercase()), [id(8)]);
        assert!(list_value_ids(&options, "minor").is_empty());
    }

    #[test]
    fn cursors_only_fit_their_order() {
        let doc = CursorDoc {
            order: "ORDER BY due ASC".to_owned(),
            values: vec![Some("2026-11-20".to_owned())],
            id: Uuid::nil(),
        };
        let cursor = encode_cursor(&doc);
        assert_eq!(decode_cursor(&cursor, "ORDER BY due ASC", 1).ok(), Some(doc));
        assert!(matches!(
            decode_cursor(&cursor, "ORDER BY due DESC", 1),
            Err(SearchError::Cursor)
        ));
        assert!(matches!(
            decode_cursor(&cursor, "ORDER BY due ASC", 2),
            Err(SearchError::Cursor)
        ));
        assert!(matches!(
            decode_cursor("not a cursor", "", 0),
            Err(SearchError::Cursor)
        ));
    }
}
