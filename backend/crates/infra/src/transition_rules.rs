//! The rules of a workflow transition as `workflow_transition.rules`
//! stores them (L04-US05): the JSON the API speaks, read into and written
//! from `domain::transitions::Rules`.

use domain::transitions::{AssignTo, Condition, PostFunction, Rules};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fields::{json_of, value_of};

/// The stored rules; an empty object is a transition without rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesDoc {
    #[serde(default)]
    conditions: ConditionsDoc,
    #[serde(default)]
    validators: Vec<Uuid>,
    #[serde(default)]
    post_functions: Vec<PostFunctionDoc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConditionsDoc {
    #[serde(default)]
    role_ids: Vec<Uuid>,
    #[serde(default)]
    group_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostFunctionDoc {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assign_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

impl PostFunctionDoc {
    fn of(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            assign_to: None,
            account_id: None,
            field_id: None,
            value: None,
            text: None,
        }
    }

    /// The post-function; none if the stored one is not understood.
    fn post_function(&self) -> Option<PostFunction> {
        let field = self.field_id.map(|f| f.as_u128());
        match self.kind.as_str() {
            "assign" => Some(PostFunction::Assign(match self.assign_to.as_deref()? {
                "current_user" => AssignTo::CurrentUser,
                "reporter" => AssignTo::Reporter,
                "member" => AssignTo::Member(self.account_id?.as_u128()),
                "nobody" => AssignTo::Nobody,
                _ => return None,
            })),
            "set_field" => Some(PostFunction::SetField {
                field: field?,
                value: value_of(self.value.as_ref()?)?,
            }),
            "clear_field" => Some(PostFunction::ClearField { field: field? }),
            "add_comment" => Some(PostFunction::AddComment {
                text: self.text.clone()?,
            }),
            _ => None,
        }
    }
}

impl From<&PostFunction> for PostFunctionDoc {
    fn from(post: &PostFunction) -> Self {
        match post {
            PostFunction::Assign(to) => {
                let (assign_to, account) = match to {
                    AssignTo::CurrentUser => ("current_user", None),
                    AssignTo::Reporter => ("reporter", None),
                    AssignTo::Member(member) => ("member", Some(Uuid::from_u128(*member))),
                    AssignTo::Nobody => ("nobody", None),
                };
                Self {
                    assign_to: Some(assign_to.to_owned()),
                    account_id: account,
                    ..Self::of("assign")
                }
            }
            PostFunction::SetField { field, value } => Self {
                field_id: Some(Uuid::from_u128(*field)),
                value: Some(json_of(value)),
                ..Self::of("set_field")
            },
            PostFunction::ClearField { field } => Self {
                field_id: Some(Uuid::from_u128(*field)),
                ..Self::of("clear_field")
            },
            PostFunction::AddComment { text } => Self {
                text: Some(text.trim().to_owned()),
                ..Self::of("add_comment")
            },
        }
    }
}

impl From<&Rules> for RulesDoc {
    fn from(rules: &Rules) -> Self {
        let ids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect();
        Self {
            conditions: ConditionsDoc {
                role_ids: ids(&rules.condition.roles),
                group_ids: ids(&rules.condition.groups),
            },
            validators: ids(&rules.validators),
            post_functions: rules.post_functions.iter().map(PostFunctionDoc::from).collect(),
        }
    }
}

impl From<&RulesDoc> for Rules {
    fn from(doc: &RulesDoc) -> Self {
        let ids = |ids: &[Uuid]| ids.iter().map(Uuid::as_u128).collect();
        Self {
            condition: Condition {
                roles: ids(&doc.conditions.role_ids),
                groups: ids(&doc.conditions.group_ids),
            },
            validators: ids(&doc.validators),
            post_functions: doc
                .post_functions
                .iter()
                .filter_map(PostFunctionDoc::post_function)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::fields::Value;

    use super::*;

    #[test]
    fn rules_round_trip_through_their_stored_form() {
        let rules = Rules {
            condition: Condition {
                roles: vec![1],
                groups: vec![2],
            },
            validators: vec![3],
            post_functions: vec![
                PostFunction::Assign(AssignTo::CurrentUser),
                PostFunction::Assign(AssignTo::Member(4)),
                PostFunction::SetField {
                    field: 3,
                    value: Value::List(vec![Value::Text("a".into()), Value::Number(2.5)]),
                },
                PostFunction::ClearField { field: 3 },
                PostFunction::AddComment {
                    text: " Started ".into(),
                },
            ],
        };
        let json = serde_json::to_value(RulesDoc::from(&rules)).unwrap();
        assert_eq!(
            json["postFunctions"][0],
            serde_json::json!({"kind": "assign", "assignTo": "current_user"})
        );
        assert_eq!(json["conditions"]["roleIds"][0], Uuid::from_u128(1).to_string());
        let back = Rules::from(&serde_json::from_value::<RulesDoc>(json).unwrap());
        assert_eq!(
            back.post_functions[4],
            PostFunction::AddComment {
                text: "Started".into()
            }
        );
        assert_eq!(back.post_functions[..4], rules.post_functions[..4]);
        assert_eq!(back.condition, rules.condition);
        // A transition without rules.
        let empty: RulesDoc = serde_json::from_str("{}").unwrap();
        assert_eq!(Rules::from(&empty), Rules::default());
    }
}
