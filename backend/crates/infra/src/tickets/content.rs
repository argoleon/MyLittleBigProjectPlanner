//! The published configuration of a project as tickets read it, and the
//! helpers on custom field values shared by the operations on tickets.

use std::collections::{BTreeMap, BTreeSet};

use domain::fields::{Kind, Rules, Value};
use domain::issue_types::TypeNode;
use domain::issues::FieldDef;
use domain::validation::FieldError;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::fields::Field;
use crate::issue_types::IssueType;
use crate::workflows::Workflows;

/// Why a project's configuration could not be read.
#[derive(Debug)]
pub enum ContentError {
    /// The project has no configuration, or it has no published version.
    NotFound,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for ContentError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no published configuration"),
            Self::Database(e) => write!(f, "database: {e}"),
        }
    }
}

/// The published configuration of a project, as tickets read it.
#[derive(Debug)]
pub struct Content {
    pub types: Vec<IssueType>,
    pub workflows: Workflows,
    pub fields: Vec<Field>,
    pub screens: Vec<crate::screens::Screen>,
}

impl Content {
    /// The published content of the configuration of `project`.
    pub async fn of(conn: &mut PgConnection, project: Uuid) -> Result<Self, ContentError> {
        let config = crate::configs::of_project(conn, project)
            .await?
            .ok_or(ContentError::NotFound)?;
        Self::of_config(conn, config).await
    }

    /// The published content of configuration `config`.
    pub async fn of_config(conn: &mut PgConnection, config: Uuid) -> Result<Self, ContentError> {
        let version = crate::configs::find(conn, config)
            .await?
            .ok_or(ContentError::NotFound)?
            .published
            .id;
        Ok(Self {
            types: crate::issue_types::list(conn, version).await?,
            workflows: crate::workflows::load(conn, version).await?,
            fields: crate::fields::list(conn, version).await?,
            screens: crate::screens::list(conn, version).await?,
        })
    }

    pub fn type_nodes(&self) -> Vec<TypeNode> {
        self.types.iter().map(IssueType::node).collect()
    }
}

/// The fields with their rules, borrowing `fields` and `types`.
pub fn field_defs<'a>(fields: &'a [Field], type_ids: &'a [Vec<u128>]) -> Vec<FieldDef<'a>> {
    fields
        .iter()
        .zip(type_ids)
        .map(|(f, types)| FieldDef {
            id: f.item_id.as_u128(),
            kind: f.kind,
            options: &f.options,
            rules: Rules {
                all_types: f.all_types,
                issue_types: types,
                rules: &f.rules,
            },
        })
        .collect()
}

/// The types each field applies to, as numbers, in the fields' order.
pub fn type_ids_of(fields: &[Field]) -> Vec<Vec<u128>> {
    fields
        .iter()
        .map(|f| f.issue_type_ids.iter().map(Uuid::as_u128).collect())
        .collect()
}

/// The custom field values stored on a ticket, by field.
pub fn stored_values(fields: &serde_json::Value) -> BTreeMap<u128, Value> {
    fields
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, json)| {
            Some((
                Uuid::parse_str(id).ok()?.as_u128(),
                crate::fields::value_of(json)?,
            ))
        })
        .collect()
}

/// The identifiers a value holds: list values, users, groups, tickets.
pub fn ids_in(value: &Value) -> Vec<Uuid> {
    let one = |value: &Value| match value {
        Value::Text(text) => domain::fields::parse_uuid(text).map(Uuid::from_u128),
        _ => None,
    };
    match value {
        Value::List(items) => items.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

/// The values of custom fields as sent (`{fieldId: json}`), or the fields
/// whose value has no known form.
pub fn read_fields(
    fields: &BTreeMap<Uuid, serde_json::Value>,
) -> Result<BTreeMap<u128, Value>, Vec<FieldError>> {
    let mut values = BTreeMap::new();
    let mut errors = Vec::new();
    for (id, json) in fields {
        match crate::fields::value_of(json) {
            Some(value) => {
                values.insert(id.as_u128(), value);
            }
            None => errors.push(FieldError::new(format!("fields.{id}"), "wrong_type")),
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

/// Field errors for the users, groups and tickets that `values` name and
/// that do not exist (`validate_value` checks their form only). Fields
/// already in error are left alone.
pub async fn unknown_references(
    conn: &mut PgConnection,
    fields: &[Field],
    values: &BTreeMap<u128, Value>,
    errors: &[FieldError],
) -> Result<Vec<FieldError>, sqlx::Error> {
    let mut unknown = Vec::new();
    for field in fields {
        let path = format!("fields.{}", field.item_id);
        let Some(value) = values.get(&field.item_id.as_u128()) else {
            continue;
        };
        if errors.iter().any(|e| e.field == path) {
            continue;
        }
        let ids = ids_in(value);
        if ids.is_empty() {
            continue;
        }
        let (found, code) = match field.kind {
            Kind::User => (
                crate::rule_references::members(conn, &ids).await?.len(),
                "unknown_member",
            ),
            Kind::Group => (
                crate::rule_references::groups(conn, &ids).await?.len(),
                "unknown_group",
            ),
            Kind::IssueLink => {
                let mut found = 0;
                for id in &ids {
                    if crate::issues::find(conn, *id).await?.is_some() {
                        found += 1;
                    }
                }
                (found, "unknown_issue")
            }
            _ => continue,
        };
        if found < ids.iter().collect::<BTreeSet<_>>().len() {
            unknown.push(FieldError::new(path, code));
        }
    }
    Ok(unknown)
}

/// A rich text document as stored: read through the allow-list and
/// checked; `None` when empty.
pub fn read_description(json: &serde_json::Value) -> Result<Option<serde_json::Value>, FieldError> {
    let doc = crate::rich_text::parse(json).map_err(|_| FieldError::new("description", "invalid"))?;
    doc.validate()
        .map_err(|refusal| FieldError::new("description", refusal.code()))?;
    Ok((!doc.is_empty()).then(|| crate::rich_text::to_json(&doc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_by_field_or_refused() {
        let id = Uuid::from_u128(7);
        let read = read_fields(&BTreeMap::from([(id, serde_json::json!("text"))])).unwrap();
        assert_eq!(read.len(), 1);
        let refused = read_fields(&BTreeMap::from([(id, serde_json::json!({"odd": true}))])).unwrap_err();
        assert_eq!(refused[0].code, "wrong_type");
        assert_eq!(refused[0].field, format!("fields.{id}"));
    }

    #[test]
    fn identifiers_are_found_in_one_value_or_a_list() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(ids_in(&Value::Text(a.to_string())), [a]);
        assert_eq!(
            ids_in(&Value::List(vec![
                Value::Text(a.to_string()),
                Value::Text(b.to_string())
            ])),
            [a, b]
        );
        assert!(ids_in(&Value::Text("not an id".to_owned())).is_empty());
    }
}
