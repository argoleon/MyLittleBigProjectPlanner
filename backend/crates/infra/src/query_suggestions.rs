//! Values the query editor and builder suggest (L06-US02): stable keys and
//! their labels, matching what is typed, within the tenant (row-level
//! security) and, for nodes, the ones the member sees.
//!
//! A word matches a value whose key starts with it, or one of whose labels
//! (any language) or names holds it, in any case.

use std::collections::BTreeMap;

use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::invitations::like_pattern;

type Labels = BTreeMap<String, String>;

/// A suggested value: its key, as the query writes it, and what it reads as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggested {
    pub key: String,
    /// A name (projects, people) or translated labels (types, statuses, fields).
    pub labels: Labels,
    /// More to tell it apart: the node type, the status category, the field kind.
    pub detail: Option<String>,
}

/// `text%`, wildcards taken literally, for keys.
fn starts_with(text: &str) -> String {
    let contains = like_pattern(text);
    contains.strip_prefix('%').unwrap_or(&contains).to_owned()
}

fn limit_of(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

fn named(key: String, name: String, detail: Option<String>) -> Suggested {
    Suggested {
        key,
        labels: [(String::new(), name)].into(),
        detail,
    }
}

/// Nodes the member sees (`visible`, `None`: all), projects only unless
/// `any_type`, active ones, by key.
pub async fn nodes(
    conn: &mut PgConnection,
    visible: Option<&[Uuid]>,
    any_type: bool,
    text: &str,
    limit: usize,
) -> Result<Vec<Suggested>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT n.key, n.name, n.node_type
        FROM node n
        WHERE n.archived_at IS NULL
          AND ($1 OR n.node_type = 'project')
          AND ($2::uuid[] IS NULL OR n.id = ANY($2))
          AND (lower(n.key) LIKE $3 OR lower(n.name) LIKE $4)
        ORDER BY n.key
        LIMIT $5
        "#,
        any_type,
        visible,
        starts_with(text),
        like_pattern(text),
        limit_of(limit)
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| named(row.key, row.name, Some(row.node_type)))
        .collect())
}

/// Issue types of the published configurations, one per key.
pub async fn issue_types(
    conn: &mut PgConnection,
    text: &str,
    limit: usize,
) -> Result<Vec<Suggested>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (t.key) t.key, t.label AS "label: Json<Labels>"
        FROM issue_type t
        JOIN project_config_version v ON v.id = t.version_id AND v.state = 'published'
        WHERE t.key LIKE $1
           OR EXISTS (SELECT 1 FROM jsonb_each_text(t.label) l WHERE lower(l.value) LIKE $2)
        ORDER BY t.key, t.updated_at DESC
        LIMIT $3
        "#,
        starts_with(text),
        like_pattern(text),
        limit_of(limit)
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Suggested {
            key: row.key,
            labels: row.label.0,
            detail: None,
        })
        .collect())
}

/// Statuses of the published workflows, one per key, with their category.
pub async fn statuses(
    conn: &mut PgConnection,
    text: &str,
    limit: usize,
) -> Result<Vec<Suggested>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (s.key) s.key, s.label AS "label: Json<Labels>", s.category
        FROM workflow_status s
        JOIN project_config_version v ON v.id = s.version_id AND v.state = 'published'
        WHERE s.key LIKE $1
           OR EXISTS (SELECT 1 FROM jsonb_each_text(s.label) l WHERE lower(l.value) LIKE $2)
        ORDER BY s.key, s.updated_at DESC
        LIMIT $3
        "#,
        starts_with(text),
        like_pattern(text),
        limit_of(limit)
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Suggested {
            key: row.key,
            labels: row.label.0,
            detail: Some(row.category),
        })
        .collect())
}

/// Active members of the tenant, by email, with their name.
pub async fn members(
    conn: &mut PgConnection,
    text: &str,
    limit: usize,
) -> Result<Vec<Suggested>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT a.email, a.display_name
        FROM membership m
        JOIN account a ON a.id = m.account_id
        WHERE m.status = 'active' AND m.account_type = 'internal'
          AND (lower(a.email) LIKE $1 OR lower(a.display_name) LIKE $1)
        ORDER BY lower(a.display_name), a.email
        LIMIT $2
        "#,
        like_pattern(text),
        limit_of(limit)
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| named(row.email, row.display_name, None))
        .collect())
}

/// Custom fields of the published configurations, one per key, with their kind.
pub async fn fields(
    conn: &mut PgConnection,
    text: &str,
    limit: usize,
) -> Result<Vec<Suggested>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (f.key) f.key, f.label AS "label: Json<Labels>", f.kind
        FROM field_def f
        JOIN project_config_version v ON v.id = f.version_id AND v.state = 'published'
        WHERE f.key LIKE $1
           OR EXISTS (SELECT 1 FROM jsonb_each_text(f.label) l WHERE lower(l.value) LIKE $2)
        ORDER BY f.key, f.updated_at DESC
        LIMIT $3
        "#,
        starts_with(text),
        like_pattern(text),
        limit_of(limit)
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Suggested {
            key: row.key,
            labels: row.label.0,
            detail: Some(row.kind),
        })
        .collect())
}

/// The list values of the custom field `key` (any level), by label; the
/// query names them by one of their labels.
pub async fn list_values(
    conn: &mut PgConnection,
    key: &str,
    text: &str,
    limit: usize,
) -> Result<Vec<Suggested>, sqlx::Error> {
    let rows = sqlx::query_scalar!(
        r#"
        SELECT f.options
        FROM field_def f
        JOIN project_config_version v ON v.id = f.version_id AND v.state = 'published'
        WHERE f.key = $1 AND f.kind IN ('single_select', 'multi_select', 'cascading_select')
        "#,
        key
    )
    .fetch_all(conn)
    .await?;
    fn walk(values: &[domain::fields::ListValue], text: &str, found: &mut Vec<Suggested>) {
        for value in values {
            let matches = value
                .labels
                .values()
                .any(|label| label.to_lowercase().contains(text));
            if matches
                && value.active
                && let Some(first) = value.labels.values().next()
                && !found.iter().any(|s| s.labels == value.labels)
            {
                found.push(Suggested {
                    key: first.clone(),
                    labels: value.labels.clone(),
                    detail: None,
                });
            }
            walk(&value.children, text, found);
        }
    }
    let text = text.to_lowercase();
    let mut found = Vec::new();
    for options in rows {
        let options = crate::fields::options_of(options);
        walk(options.values.as_deref().unwrap_or_default(), &text, &mut found);
    }
    found.truncate(limit);
    Ok(found)
}
