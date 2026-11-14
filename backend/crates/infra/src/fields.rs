//! SQL of the fields of a configuration version and of their rules
//! (L04-US03). Row-level security scopes every query to the current tenant.
//!
//! A field is addressed by its `item_id`, which it keeps from one version
//! to the next; so are the types and statuses its rules name.

use std::collections::BTreeMap;

use domain::fields::{Kind, ListValue, Mode, Options, Rule, Step, Unit, Value};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

type Labels = BTreeMap<String, String>;

/// A field as stored in a version, with its rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Its identity across versions.
    pub item_id: Uuid,
    pub key: String,
    pub label_translations: Labels,
    pub kind: Kind,
    pub options: Options,
    pub is_system: bool,
    /// Applies to every type, including the ones added later.
    pub all_types: bool,
    /// The types it applies to when not all, by `item_id`.
    pub issue_type_ids: Vec<Uuid>,
    /// Its Hidden and Required cells; the others are Visible.
    pub rules: Vec<Rule>,
    pub position: i32,
    pub version: i64,
}

/// What a field is made of; its key and type are set once, when it is
/// added.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldFields<'a> {
    pub label_translations: &'a Labels,
    pub options: &'a Options,
    pub all_types: bool,
    pub issue_type_ids: &'a [Uuid],
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another field of the version has this key.
    KeyTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// The options as stored: what the API spells under `options`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptionsDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decimals: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    multiple: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<Vec<ValueDoc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ValueDoc {
    id: Uuid,
    label: Labels,
    active: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<ValueDoc>,
}

impl From<&Options> for OptionsDoc {
    fn from(options: &Options) -> Self {
        Self {
            max_length: options.max_length,
            decimals: options.decimals,
            min: options.min,
            max: options.max,
            unit: options.unit.map(|u| u.as_str().to_owned()),
            multiple: options.multiple,
            values: options
                .values
                .as_ref()
                .map(|values| values.iter().map(ValueDoc::from).collect()),
        }
    }
}

impl From<OptionsDoc> for Options {
    fn from(doc: OptionsDoc) -> Self {
        Self {
            max_length: doc.max_length,
            decimals: doc.decimals,
            min: doc.min,
            max: doc.max,
            unit: doc.unit.as_deref().and_then(Unit::parse),
            multiple: doc.multiple,
            values: doc
                .values
                .map(|values| values.into_iter().map(ListValue::from).collect()),
        }
    }
}

impl From<&ListValue> for ValueDoc {
    fn from(value: &ListValue) -> Self {
        Self {
            id: Uuid::from_u128(value.id),
            label: trimmed(&value.labels),
            active: value.active,
            children: value.children.iter().map(Self::from).collect(),
        }
    }
}

impl From<ValueDoc> for ListValue {
    fn from(doc: ValueDoc) -> Self {
        Self {
            id: doc.id.as_u128(),
            labels: doc.label,
            active: doc.active,
            children: doc.children.into_iter().map(Self::from).collect(),
        }
    }
}

struct Row {
    item_id: Uuid,
    key: String,
    label: Json<Labels>,
    kind: String,
    options: Json<OptionsDoc>,
    is_system: bool,
    all_types: bool,
    issue_type_ids: Vec<Uuid>,
    position: i32,
    version: i64,
}

struct RuleRow {
    field_id: Uuid,
    issue_type_id: Uuid,
    status_id: Option<Uuid>,
    mode: String,
}

impl RuleRow {
    fn rule(&self) -> Rule {
        Rule {
            issue_type: self.issue_type_id.as_u128(),
            step: self
                .status_id
                .map_or(Step::Creation, |s| Step::Status(s.as_u128())),
            // Only these two are stored.
            mode: if self.mode == "required" {
                Mode::Required
            } else {
                Mode::Hidden
            },
        }
    }
}

/// A value as JSON holds it, for a field; none for an object, which no
/// field takes.
pub fn value_of(json: &serde_json::Value) -> Option<Value> {
    Some(match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::Number(n.as_f64()?),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(items) => Value::List(items.iter().map(value_of).collect::<Option<_>>()?),
        serde_json::Value::Object(_) => return None,
    })
}

/// The JSON of a value.
pub fn json_of(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => {
            serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::List(items) => serde_json::Value::Array(items.iter().map(json_of).collect()),
    }
}

/// Labels trimmed, empty ones dropped.
fn trimmed(labels: &Labels) -> Labels {
    labels
        .iter()
        .map(|(locale, label)| (locale.clone(), label.trim().to_owned()))
        .filter(|(_, label)| !label.is_empty())
        .collect()
}

/// The fields of `version`, in their order, with their rules.
pub async fn list(conn: &mut PgConnection, version: Uuid) -> Result<Vec<Field>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT item_id, key, label AS "label: Json<Labels>", kind, options AS "options: Json<OptionsDoc>",
               is_system, all_types, issue_type_ids, position, version
        FROM field_def WHERE version_id = $1
        ORDER BY position, key
        "#,
        version
    )
    .fetch_all(&mut *conn)
    .await?;
    let rules = sqlx::query_as!(
        RuleRow,
        r#"
        SELECT r.field_id, r.issue_type_id, r.status_id, r.mode
        FROM field_rule r
        JOIN issue_type t ON t.version_id = r.version_id AND t.item_id = r.issue_type_id
        LEFT JOIN workflow_status s ON s.version_id = r.version_id AND s.item_id = r.status_id
        WHERE r.version_id = $1
        ORDER BY t.position, r.status_id IS NOT NULL, s.position
        "#,
        version
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Field {
            rules: rules
                .iter()
                .filter(|r| r.field_id == row.item_id)
                .map(RuleRow::rule)
                .collect(),
            item_id: row.item_id,
            key: row.key,
            label_translations: row.label.0,
            // The database only holds the kinds of the domain.
            kind: Kind::parse(&row.kind).unwrap_or(Kind::ShortText),
            options: row.options.0.into(),
            is_system: row.is_system,
            all_types: row.all_types,
            issue_type_ids: row.issue_type_ids,
            position: row.position,
            version: row.version,
        })
        .collect())
}

/// The field `item` of `version`.
pub async fn find(conn: &mut PgConnection, version: Uuid, item: Uuid) -> Result<Option<Field>, sqlx::Error> {
    Ok(list(conn, version).await?.into_iter().find(|f| f.item_id == item))
}

/// Adds the custom field `item` to `version`, last, without rules.
pub async fn insert(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    key: &str,
    kind: Kind,
    fields: &FieldFields<'_>,
) -> WriteResult<()> {
    let inserted = sqlx::query!(
        r#"
        INSERT INTO field_def
            (id, tenant_id, version_id, item_id, key, label, kind, options, all_types, issue_type_ids, position)
        SELECT $1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9,
               coalesce((SELECT max(position) + 1 FROM field_def WHERE version_id = $2), 0)
        "#,
        crate::id::new_id(),
        version,
        item,
        key,
        Json(trimmed(fields.label_translations)) as _,
        kind.as_str(),
        Json(OptionsDoc::from(fields.options)) as _,
        fields.all_types,
        fields.issue_type_ids
    )
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => Ok(Ok(())),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some("field_def_key_unique") => {
            Ok(Err(WriteConflict::KeyTaken))
        }
        Err(err) => Err(err),
    }
}

/// Changes the field `item` of `version`, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
    fields: &FieldFields<'_>,
) -> WriteResult<()> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE field_def
        SET label = $4, options = $5, all_types = $6, issue_type_ids = $7, version = version + 1
        WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        RETURNING item_id
        "#,
        version,
        item,
        expected_versions,
        Json(trimmed(fields.label_translations)) as _,
        Json(OptionsDoc::from(fields.options)) as _,
        fields.all_types,
        fields.issue_type_ids
    )
    .fetch_optional(&mut *conn)
    .await?;
    match updated {
        Some(_) => Ok(Ok(())),
        None => Ok(Err(missing_or_stale(conn, version, item).await?)),
    }
}

/// Replaces the rules of the field `item` of `version` with `rules` (its
/// Hidden and Required cells; Visible ones are not stored).
pub async fn set_rules(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    rules: &[Rule],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM field_rule WHERE version_id = $1 AND field_id = $2",
        version,
        item
    )
    .execute(&mut *conn)
    .await?;
    let stored: Vec<&Rule> = rules.iter().filter(|r| r.mode != Mode::Visible).collect();
    let types: Vec<Uuid> = stored.iter().map(|r| Uuid::from_u128(r.issue_type)).collect();
    let statuses: Vec<Option<Uuid>> = stored
        .iter()
        .map(|r| match r.step {
            Step::Creation => None,
            Step::Status(status) => Some(Uuid::from_u128(status)),
        })
        .collect();
    let modes: Vec<&str> = stored.iter().map(|r| r.mode.as_str()).collect();
    let ids: Vec<Uuid> = stored.iter().map(|_| crate::id::new_id()).collect();
    sqlx::query!(
        r#"
        INSERT INTO field_rule (id, tenant_id, version_id, field_id, issue_type_id, status_id, mode)
        SELECT r.id, app_current_tenant(), $1, $2, r.issue_type_id, r.status_id, r.mode
        FROM unnest($3::uuid[], $4::uuid[], $5::uuid[], $6::text[]) AS r (id, issue_type_id, status_id, mode)
        "#,
        version,
        item,
        &ids,
        &types,
        &statuses as &[Option<Uuid>],
        &modes as &[&str]
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the field `item` from `version`, with its rules, if its version
/// is expected.
pub async fn delete(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM field_def WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))",
        version,
        item,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 0 {
        return Ok(Err(missing_or_stale(conn, version, item).await?));
    }
    Ok(Ok(()))
}

/// Takes the type `issue_type`, removed from `version`, out of the types of
/// its fields (its cells went with it); the fields that changed.
pub async fn forget_type(
    conn: &mut PgConnection,
    version: Uuid,
    issue_type: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        UPDATE field_def SET issue_type_ids = array_remove(issue_type_ids, $2), version = version + 1
        WHERE version_id = $1 AND $2 = ANY(issue_type_ids)
        RETURNING item_id
        "#,
        version,
        issue_type
    )
    .fetch_all(conn)
    .await
}

/// Removes the cells of `issue_type` that name a status of another workflow
/// than `workflow`, the one it now follows; the fields that changed.
pub async fn prune_rules(
    conn: &mut PgConnection,
    version: Uuid,
    issue_type: Uuid,
    workflow: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        DELETE FROM field_rule r
        WHERE r.version_id = $1 AND r.issue_type_id = $2 AND r.status_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM workflow_status s
              WHERE s.version_id = $1 AND s.item_id = r.status_id AND s.workflow_id = $3
          )
        RETURNING r.field_id
        "#,
        version,
        issue_type,
        workflow
    )
    .fetch_all(conn)
    .await
    .map(|mut fields| {
        fields.sort_unstable();
        fields.dedup();
        fields
    })
}

/// Copies the fields of the version `from` into `to`, with their rules,
/// keeping their identity and order. The types and workflows go first.
pub async fn copy(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO field_def
            (id, tenant_id, version_id, item_id, key, label, kind, options, is_system, all_types, issue_type_ids, position)
        SELECT uuidv7(), tenant_id, $2, item_id, key, label, kind, options, is_system, all_types, issue_type_ids, position
        FROM field_def WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"
        INSERT INTO field_rule (id, tenant_id, version_id, field_id, issue_type_id, status_id, mode)
        SELECT uuidv7(), tenant_id, $2, field_id, issue_type_id, status_id, mode
        FROM field_rule WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives `version` the system fields (`app_seed_default_fields`), after its
/// types.
pub async fn seed_defaults(conn: &mut PgConnection, version: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT app_seed_default_fields($1)", version)
        .execute(conn)
        .await?;
    Ok(())
}

async fn missing_or_stale(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM field_def WHERE version_id = $1 AND item_id = $2) AS "exists!""#,
        version,
        item
    )
    .fetch_one(conn)
    .await?;
    Ok(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_through_their_stored_form() {
        let options = Options {
            decimals: Some(2),
            unit: Some(Unit::Hours),
            values: Some(vec![ListValue {
                id: 7,
                labels: [("fr".to_owned(), " ERP ".to_owned())].into(),
                active: false,
                children: vec![ListValue {
                    id: 8,
                    labels: [("en".to_owned(), "Finance".to_owned())].into(),
                    active: true,
                    children: Vec::new(),
                }],
            }]),
            ..Options::default()
        };
        let doc = OptionsDoc::from(&options);
        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json["unit"], "hours");
        assert!(json.get("maxLength").is_none(), "unset options are left out");
        let back: Options = serde_json::from_value::<OptionsDoc>(json).unwrap().into();
        let erp = &back.values.as_ref().unwrap()[0];
        assert_eq!(erp.labels["fr"], "ERP", "labels are trimmed");
        assert!(!erp.active && erp.children[0].active);
        assert_eq!(back.unit, Some(Unit::Hours));
    }

    #[test]
    fn values_convert_from_and_to_json() {
        let json = serde_json::json!(["a", 1.5, true, null, []]);
        let value = value_of(&json).unwrap();
        assert_eq!(
            value,
            Value::List(vec![
                Value::Text("a".into()),
                Value::Number(1.5),
                Value::Bool(true),
                Value::Null,
                Value::List(Vec::new())
            ])
        );
        assert_eq!(json_of(&value), json);
        assert_eq!(value_of(&serde_json::json!({"a": 1})), None);
        assert_eq!(value_of(&serde_json::json!([{"a": 1}])), None);
    }
}
