//! Rules of a workflow transition (specification §5, L04-US05): who may
//! take it, the fields it requires and its post-functions, as the API
//! speaks them; checked against the roles, groups and members of the
//! tenant and the fields of the draft. Plus what the rules editor picks
//! from: the roles, and the groups and members found by name.
//!
//! The transitions carry their rules (`WorkflowTransition.rules`); the
//! workflow list names what they refer to (`references`). Tickets apply
//! them from L05 on (domain::transitions::evaluate).

use std::collections::{BTreeMap, BTreeSet};

use domain::fields::Value;
use domain::transitions::{AssignTo, Condition, FieldRef, PostFunction, References, Rules, validate_rules};
use domain::validation::{FieldError, Violations};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::configs::{TAG, require_config_admin};
use crate::extract::{AcceptLanguage, ApiJson, ApiQuery};
use crate::grants::{CandidateParams, CandidateView, RoleRefView};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::TenantTx;
use axum::response::{IntoResponse, Response};

/// What a post-function does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = PostFunctionKind)]
pub enum PostFunctionKind {
    /// Assign the ticket (`assignTo`).
    Assign,
    /// Give a field (`fieldId`) a value (`value`).
    SetField,
    /// Empty a field (`fieldId`).
    ClearField,
    /// Add a comment (`text`).
    AddComment,
}

/// Whom the Assign post-function assigns the ticket to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = AssignTarget)]
pub enum AssignTarget {
    /// Whoever takes the transition.
    CurrentUser,
    /// Whoever reported the ticket.
    Reporter,
    /// A member of the organization (`accountId`).
    Member,
    /// Nobody: unassigned.
    Nobody,
}

/// Who may take a transition: anyone when both lists are empty, otherwise
/// whoever has one of the roles on the project or belongs to one of the
/// groups.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = TransitionConditions)]
pub struct ConditionsBody {
    #[serde(default)]
    role_ids: Vec<Uuid>,
    #[serde(default)]
    group_ids: Vec<Uuid>,
}

/// One post-function; each kind takes its own properties.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = TransitionPostFunction)]
pub struct PostFunctionBody {
    kind: PostFunctionKind,
    /// `assign` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assign_to: Option<AssignTarget>,
    /// `assign` to a `member` only: their account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<Uuid>,
    /// `set_field` and `clear_field`: a field of the configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field_id: Option<Uuid>,
    /// `set_field`: a value that fits the field, as on a ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
    /// `add_comment`: 2,000 characters at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

/// The rules of a transition.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = TransitionRules)]
pub struct RulesBody {
    #[serde(default)]
    conditions: ConditionsBody,
    /// Fields of the configuration that must hold a value.
    #[serde(default)]
    validators: Vec<Uuid>,
    /// Run in their order, in the transaction of the transition.
    #[serde(default)]
    post_functions: Vec<PostFunctionBody>,
    /// A transition screen of the configuration, shown before the
    /// transition runs (L04-US06); `null`: none.
    #[serde(default)]
    screen_id: Option<Uuid>,
}

impl RulesBody {
    /// The rules as an admin reads them, with the transition screen.
    pub(crate) fn of(rules: &Rules, screen_id: Option<Uuid>) -> Self {
        let ids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect();
        Self {
            conditions: ConditionsBody {
                role_ids: ids(&rules.condition.roles),
                group_ids: ids(&rules.condition.groups),
            },
            validators: ids(&rules.validators),
            post_functions: rules.post_functions.iter().map(post_function_body).collect(),
            screen_id,
        }
    }

    /// The rules, if each post-function has what its kind takes; the
    /// errors otherwise, under `rules.`.
    fn rules(&self) -> Result<Rules, Vec<FieldError>> {
        let mut v = Violations::new();
        let ids = |ids: &[Uuid]| ids.iter().map(Uuid::as_u128).collect();
        let mut post_functions = Vec::new();
        for (i, body) in self.post_functions.iter().enumerate() {
            match post_function(body) {
                Ok(post) => post_functions.push(post),
                Err((property, code)) => v.add(format!("rules.postFunctions[{i}].{property}"), code),
            }
        }
        v.into_result().map(|()| Rules {
            condition: Condition {
                roles: ids(&self.conditions.role_ids),
                groups: ids(&self.conditions.group_ids),
            },
            validators: ids(&self.validators),
            post_functions,
        })
    }
}

fn post_function_body(post: &PostFunction) -> PostFunctionBody {
    let mut body = PostFunctionBody {
        kind: PostFunctionKind::Assign,
        assign_to: None,
        account_id: None,
        field_id: None,
        value: None,
        text: None,
    };
    match post {
        PostFunction::Assign(to) => {
            body.assign_to = Some(match to {
                AssignTo::CurrentUser => AssignTarget::CurrentUser,
                AssignTo::Reporter => AssignTarget::Reporter,
                AssignTo::Member(member) => {
                    body.account_id = Some(Uuid::from_u128(*member));
                    AssignTarget::Member
                }
                AssignTo::Nobody => AssignTarget::Nobody,
            });
        }
        PostFunction::SetField { field, value } => {
            body.kind = PostFunctionKind::SetField;
            body.field_id = Some(Uuid::from_u128(*field));
            body.value = Some(infra::fields::json_of(value));
        }
        PostFunction::ClearField { field } => {
            body.kind = PostFunctionKind::ClearField;
            body.field_id = Some(Uuid::from_u128(*field));
        }
        PostFunction::AddComment { text } => {
            body.kind = PostFunctionKind::AddComment;
            body.text = Some(text.clone());
        }
    }
    body
}

/// The post-function, or the property it misses or should not have.
fn post_function(body: &PostFunctionBody) -> Result<PostFunction, (&'static str, &'static str)> {
    let unexpected = |present: bool, name: &'static str| {
        if present {
            Err((name, "not_applicable"))
        } else {
            Ok(())
        }
    };
    let required = |id: Option<Uuid>, name: &'static str| id.map(|id| id.as_u128()).ok_or((name, "required"));
    let kind = body.kind;
    unexpected(
        kind != PostFunctionKind::Assign && body.assign_to.is_some(),
        "assignTo",
    )?;
    unexpected(
        body.assign_to != Some(AssignTarget::Member) && body.account_id.is_some(),
        "accountId",
    )?;
    unexpected(
        !matches!(kind, PostFunctionKind::SetField | PostFunctionKind::ClearField) && body.field_id.is_some(),
        "fieldId",
    )?;
    unexpected(
        kind != PostFunctionKind::SetField && body.value.is_some(),
        "value",
    )?;
    unexpected(
        kind != PostFunctionKind::AddComment && body.text.is_some(),
        "text",
    )?;
    Ok(match kind {
        PostFunctionKind::Assign => {
            PostFunction::Assign(match body.assign_to.ok_or(("assignTo", "required"))? {
                AssignTarget::CurrentUser => AssignTo::CurrentUser,
                AssignTarget::Reporter => AssignTo::Reporter,
                AssignTarget::Member => AssignTo::Member(required(body.account_id, "accountId")?),
                AssignTarget::Nobody => AssignTo::Nobody,
            })
        }
        PostFunctionKind::SetField => PostFunction::SetField {
            field: required(body.field_id, "fieldId")?,
            value: match &body.value {
                None => Value::Null,
                Some(json) => infra::fields::value_of(json).ok_or(("value", "wrong_type"))?,
            },
        },
        PostFunctionKind::ClearField => PostFunction::ClearField {
            field: required(body.field_id, "fieldId")?,
        },
        PostFunctionKind::AddComment => PostFunction::AddComment {
            text: body.text.clone().unwrap_or_default(),
        },
    })
}

/// The rules of `body`, checked against the roles, groups and members of
/// the tenant and the fields of `version`.
/// The rules of `body` and its transition screen, checked: the screen must
/// be a transition screen of `version`.
pub(crate) async fn checked(
    tx: &mut TenantTx,
    version: Uuid,
    body: &RulesBody,
) -> Result<(Rules, Option<Uuid>), Problem> {
    let rules = body.rules().map_err(Problem::validation)?;
    let uuids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect::<Vec<_>>();
    let members: Vec<u128> = rules
        .post_functions
        .iter()
        .filter_map(|p| match p {
            PostFunction::Assign(AssignTo::Member(member)) => Some(*member),
            _ => None,
        })
        .collect();
    let found = |items: Vec<Uuid>| {
        items
            .into_iter()
            .map(|id| id.as_u128())
            .collect::<BTreeSet<u128>>()
    };
    let roles = found(
        infra::rule_references::roles(tx, &uuids(&rules.condition.roles))
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
    );
    let groups = found(
        infra::rule_references::groups(tx, &uuids(&rules.condition.groups))
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
    );
    let members = found(
        infra::rule_references::members(tx, &uuids(&members))
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
    );
    let fields = infra::fields::list(tx, version).await?;
    let refs = References {
        roles,
        groups,
        members,
        fields: fields
            .iter()
            .map(|f| {
                (
                    f.item_id.as_u128(),
                    FieldRef {
                        kind: f.kind,
                        options: &f.options,
                    },
                )
            })
            .collect(),
    };
    let mut errors: Vec<FieldError> = validate_rules(&rules, &refs)
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|e| FieldError::new(format!("rules.{}", e.field), e.code))
        .collect();
    if let Some(screen) = body.screen_id {
        let found = infra::screens::find(tx, version, screen).await?;
        if !found.is_some_and(|s| s.kind == domain::screens::Kind::Transition) {
            errors.push(FieldError::new("rules.screenId", "not_found"));
        }
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    Ok((rules, body.screen_id))
}

/// A group or a member the rules name.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NamedRef)]
pub struct NamedRefView {
    id: Uuid,
    name: String,
}

/// What the rules of the transitions name, with their names: a role, a
/// group or a member missing here was deleted since, and matches nobody.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TransitionRuleReferences)]
pub struct ReferencesView {
    roles: Vec<RoleRefView>,
    groups: Vec<NamedRefView>,
    /// By account.
    members: Vec<NamedRefView>,
}

/// The roles, groups and members `rules` name, with their names.
pub(crate) async fn references<'a>(
    tx: &mut TenantTx,
    language: &Language,
    rules: impl Iterator<Item = &'a Rules>,
) -> Result<ReferencesView, Problem> {
    let mut roles = BTreeSet::new();
    let mut groups = BTreeSet::new();
    let mut members = BTreeSet::new();
    for rules in rules {
        roles.extend(rules.condition.roles.iter().map(|id| Uuid::from_u128(*id)));
        groups.extend(rules.condition.groups.iter().map(|id| Uuid::from_u128(*id)));
        members.extend(rules.post_functions.iter().filter_map(|p| match p {
            PostFunction::Assign(AssignTo::Member(member)) => Some(Uuid::from_u128(*member)),
            _ => None,
        }));
    }
    let list = |set: BTreeSet<Uuid>| set.into_iter().collect::<Vec<_>>();
    let named = |items: Vec<(Uuid, String)>| {
        items
            .into_iter()
            .map(|(id, name)| NamedRefView { id, name })
            .collect()
    };
    Ok(ReferencesView {
        roles: infra::rule_references::roles(tx, &list(roles))
            .await?
            .into_iter()
            .map(|(id, labels)| role_view(language, id, labels))
            .collect(),
        groups: named(infra::rule_references::groups(tx, &list(groups)).await?),
        members: named(infra::rule_references::members(tx, &list(members)).await?),
    })
}

fn role_view(language: &Language, id: Uuid, labels: BTreeMap<String, String>) -> RoleRefView {
    RoleRefView {
        id,
        label: language.label(&labels),
        label_translations: labels,
    }
}

/// Roles offered at most to the rules editor.
const MAX_ROLES: usize = 200;

/// `GET /api/v1/project-configs/transition-roles`.
#[utoipa::path(
    get,
    path = "/project-configs/transition-roles",
    tag = TAG,
    operation_id = "listTransitionRoles",
    summary = "List the roles a transition can be restricted to",
    description = "Tenant admins and configuration admins. Every role of the tenant, from the \
                   oldest.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    responses(
        (status = 200, description = "The roles", body = Vec<RoleRefView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn roles(mut tx: TenantTx, accept: AcceptLanguage) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let roles = infra::roles::list(&mut tx, None, MAX_ROLES).await?.items;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let views: Vec<RoleRefView> = roles
        .into_iter()
        .map(|role| role_view(&language, role.id, role.label_translations))
        .collect();
    let mut response = ApiJson(views).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/project-configs/transition-candidates`.
#[utoipa::path(
    get,
    path = "/project-configs/transition-candidates",
    tag = TAG,
    operation_id = "listTransitionCandidates",
    summary = "Search the groups and members transition rules can name",
    description = "Tenant admins and configuration admins. The groups, then the active members of \
                   the tenant, whose name (or email) holds `search`, each by name: a condition \
                   names groups, the Assign post-function a member.",
    params(CandidateParams),
    responses(
        (status = 200, description = "The candidates", body = Vec<CandidateView>),
        (status = 400, description = "Invalid `limit`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn candidates(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<CandidateParams>,
) -> Result<ApiJson<Vec<CandidateView>>, Problem> {
    require_config_admin(&tx)?;
    let limit = params.limit.unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("limit: 1 to 50"));
    }
    let search = params.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let found = infra::grants::candidates(&mut tx, search, limit).await?;
    tx.commit().await?;
    Ok(ApiJson(found.into_iter().map(CandidateView::from).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: serde_json::Value) -> PostFunctionBody {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn each_post_function_takes_its_own_properties() {
        let member = Uuid::from_u128(7);
        let field = Uuid::from_u128(8);
        for (json, expected) in [
            (
                serde_json::json!({"kind": "assign", "assignTo": "current_user"}),
                PostFunction::Assign(AssignTo::CurrentUser),
            ),
            (
                serde_json::json!({"kind": "assign", "assignTo": "member", "accountId": member}),
                PostFunction::Assign(AssignTo::Member(7)),
            ),
            (
                serde_json::json!({"kind": "set_field", "fieldId": field, "value": [1.5]}),
                PostFunction::SetField {
                    field: 8,
                    value: Value::List(vec![Value::Number(1.5)]),
                },
            ),
            (
                serde_json::json!({"kind": "clear_field", "fieldId": field}),
                PostFunction::ClearField { field: 8 },
            ),
            (
                serde_json::json!({"kind": "add_comment", "text": "Started"}),
                PostFunction::AddComment {
                    text: "Started".into(),
                },
            ),
        ] {
            let body = body(json);
            let post = post_function(&body).unwrap();
            assert_eq!(post, expected);
            // And back, as an admin reads it.
            assert_eq!(
                serde_json::to_value(post_function_body(&post)).unwrap(),
                serde_json::to_value(&body).unwrap()
            );
        }
    }

    #[test]
    fn a_post_function_names_what_it_misses_or_should_not_have() {
        for (json, expected) in [
            (serde_json::json!({"kind": "assign"}), ("assignTo", "required")),
            (
                serde_json::json!({"kind": "assign", "assignTo": "member"}),
                ("accountId", "required"),
            ),
            (
                serde_json::json!({"kind": "assign", "assignTo": "nobody", "accountId": Uuid::from_u128(1)}),
                ("accountId", "not_applicable"),
            ),
            (
                serde_json::json!({"kind": "set_field", "value": 1}),
                ("fieldId", "required"),
            ),
            (
                serde_json::json!({"kind": "set_field", "fieldId": Uuid::from_u128(1), "value": {"a": 1}}),
                ("value", "wrong_type"),
            ),
            (
                serde_json::json!({"kind": "clear_field", "fieldId": Uuid::from_u128(1), "value": 1}),
                ("value", "not_applicable"),
            ),
            (
                serde_json::json!({"kind": "add_comment", "assignTo": "nobody"}),
                ("assignTo", "not_applicable"),
            ),
        ] {
            assert_eq!(
                post_function(&body(json.clone())).unwrap_err(),
                expected,
                "{json}"
            );
        }
        // Missing text is the domain's: an empty comment is `required` there.
        assert_eq!(
            post_function(&body(serde_json::json!({"kind": "add_comment"}))).unwrap(),
            PostFunction::AddComment { text: String::new() }
        );
        let rules: RulesBody = serde_json::from_value(serde_json::json!({
            "postFunctions": [{"kind": "assign", "assignTo": "nobody"}, {"kind": "clear_field"}]
        }))
        .unwrap();
        assert_eq!(
            rules.rules().unwrap_err(),
            [FieldError::new("rules.postFunctions[1].fieldId", "required")]
        );
    }
}
