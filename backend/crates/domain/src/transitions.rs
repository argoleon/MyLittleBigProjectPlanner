//! Rules of a workflow transition (specification §5, L04-US05): who may
//! take it (conditions), which fields it requires (validators), and what
//! it does to the ticket (post-functions).
//!
//! - A transition with no role and no group is open to whoever may edit
//!   the ticket; otherwise the actor needs one of its roles on the project,
//!   or to belong to one of its groups.
//! - Validators name fields of the configuration that must hold a value.
//! - Post-functions run in their order: assign the ticket, set or clear a
//!   field, add a comment.
//!
//! [`evaluate`] is pure: from the rules, a snapshot of the ticket and the
//! actor, it returns why the transition is refused, or the changes to
//! apply in one transaction. Tickets (L05) call it. The transition screen
//! comes with the screens (L04-US06).

use std::collections::{BTreeMap, BTreeSet};

use crate::fields::{Kind, Options, Value, validate_value};
use crate::validation::{FieldError, Violations};

/// Most post-functions of a transition.
pub const MAX_POST_FUNCTIONS: usize = 20;
/// Longest comment a post-function adds, in characters.
pub const COMMENT_MAX_CHARS: usize = 2000;

/// Who may take a transition: anyone (both empty), or the actors with one
/// of the roles on the project, or in one of the groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Condition {
    pub roles: Vec<u128>,
    pub groups: Vec<u128>,
}

impl Condition {
    pub fn is_open(&self) -> bool {
        self.roles.is_empty() && self.groups.is_empty()
    }
}

/// Whom the Assign post-function assigns the ticket to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignTo {
    /// Whoever takes the transition.
    CurrentUser,
    /// Whoever reported the ticket.
    Reporter,
    /// A chosen member of the organization.
    Member(u128),
    /// Nobody: the ticket is unassigned.
    Nobody,
}

/// What a transition does to the ticket, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum PostFunction {
    Assign(AssignTo),
    SetField { field: u128, value: Value },
    ClearField { field: u128 },
    AddComment { text: String },
}

/// The rules of a transition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rules {
    pub condition: Condition,
    /// Fields that must hold a value.
    pub validators: Vec<u128>,
    pub post_functions: Vec<PostFunction>,
}

impl Rules {
    /// The rules without any reference to the field `field`, removed from
    /// the configuration.
    pub fn without_field(&self, field: u128) -> Self {
        Self {
            condition: self.condition.clone(),
            validators: self.validators.iter().copied().filter(|f| *f != field).collect(),
            post_functions: self
                .post_functions
                .iter()
                .filter(|p| match p {
                    PostFunction::SetField { field: f, .. } | PostFunction::ClearField { field: f } => {
                        *f != field
                    }
                    PostFunction::Assign(_) | PostFunction::AddComment { .. } => true,
                })
                .cloned()
                .collect(),
        }
    }

    /// Whether it names the field `field`.
    pub fn names_field(&self, field: u128) -> bool {
        self.without_field(field) != *self
    }
}

/// A field of the configuration, as the rules may name it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldRef<'a> {
    pub kind: Kind,
    pub options: &'a Options,
}

/// What the rules may name, in the tenant and the configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct References<'a> {
    pub roles: BTreeSet<u128>,
    pub groups: BTreeSet<u128>,
    /// Active members of the organization.
    pub members: BTreeSet<u128>,
    pub fields: BTreeMap<u128, FieldRef<'a>>,
}

/// The rules an admin sets, checked against what exists: every error, by
/// its place in the rules (`conditions.roleIds[0]`, `postFunctions[1].value`).
pub fn validate_rules(rules: &Rules, refs: &References<'_>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    ids_errors(&mut v, "conditions.roleIds", &rules.condition.roles, &refs.roles);
    ids_errors(
        &mut v,
        "conditions.groupIds",
        &rules.condition.groups,
        &refs.groups,
    );
    let known: BTreeSet<u128> = refs.fields.keys().copied().collect();
    ids_errors(&mut v, "validators", &rules.validators, &known);
    if rules.post_functions.len() > MAX_POST_FUNCTIONS {
        v.add("postFunctions", "too_many");
    }
    for (i, post) in rules.post_functions.iter().enumerate() {
        let path = format!("postFunctions[{i}]");
        match post {
            PostFunction::Assign(AssignTo::Member(member)) if !refs.members.contains(member) => {
                v.add(format!("{path}.accountId"), "unknown_member");
            }
            PostFunction::SetField { field, value } => match refs.fields.get(field) {
                None => v.add(format!("{path}.fieldId"), "not_found"),
                Some(_) if value.is_empty() => v.add(format!("{path}.value"), "required"),
                Some(found) => {
                    if let Err(error) = validate_value(found.kind, found.options, value, None) {
                        v.add(format!("{path}.value"), error.code());
                    }
                }
            },
            PostFunction::ClearField { field } if !refs.fields.contains_key(field) => {
                v.add(format!("{path}.fieldId"), "not_found");
            }
            PostFunction::Assign(_) | PostFunction::ClearField { .. } => {}
            PostFunction::AddComment { text } => {
                if text.trim().is_empty() {
                    v.add(format!("{path}.text"), "required");
                } else if text.chars().count() > COMMENT_MAX_CHARS {
                    v.add(format!("{path}.text"), "too_long");
                }
            }
        }
    }
    v.into_result()
}

/// Each of `ids` must be one of `known`, and named once.
fn ids_errors(v: &mut Violations, path: &str, ids: &[u128], known: &BTreeSet<u128>) {
    let mut seen = BTreeSet::new();
    for (i, id) in ids.iter().enumerate() {
        if !known.contains(id) {
            v.add(format!("{path}[{i}]"), "not_found");
        } else if !seen.insert(*id) {
            v.add(format!("{path}[{i}]"), "duplicate");
        }
    }
}

/// Who takes the transition: their account, their roles on the project of
/// the ticket (granted to them or their groups, there or above), their
/// groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Actor {
    pub account: u128,
    pub roles: BTreeSet<u128>,
    pub groups: BTreeSet<u128>,
}

/// What the rules read of the ticket.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ticket {
    pub reporter: Option<u128>,
    pub assignee: Option<u128>,
    /// The values of its fields; a field missing holds nothing.
    pub fields: BTreeMap<u128, Value>,
}

/// Whether the actor may take a transition with `condition`: it is open, or
/// they have one of its roles, or belong to one of its groups. A role or a
/// group deleted since matches nobody.
pub fn allowed(condition: &Condition, actor: &Actor) -> bool {
    condition.is_open()
        || condition.roles.iter().any(|r| actor.roles.contains(r))
        || condition.groups.iter().any(|g| actor.groups.contains(g))
}

/// Why a transition is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The actor has none of its roles and is in none of its groups.
    NotAllowed,
    /// This field must hold a value first.
    FieldRequired(u128),
}

/// A change to apply to the ticket.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The new assignee; none: unassigned.
    Assign(Option<u128>),
    SetField {
        field: u128,
        value: Value,
    },
    ClearField {
        field: u128,
    },
    AddComment {
        text: String,
    },
}

/// The transition requested on `ticket` by `actor`: refused (whoever may
/// not take it is told nothing more; otherwise every field missing, in the
/// order of the validators), or the changes of its post-functions, in their
/// order.
pub fn evaluate(rules: &Rules, ticket: &Ticket, actor: &Actor) -> Result<Vec<Change>, Vec<Refusal>> {
    if !allowed(&rules.condition, actor) {
        return Err(vec![Refusal::NotAllowed]);
    }
    let missing: Vec<Refusal> = rules
        .validators
        .iter()
        .filter(|field| ticket.fields.get(field).is_none_or(Value::is_empty))
        .map(|field| Refusal::FieldRequired(*field))
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    Ok(rules
        .post_functions
        .iter()
        .map(|post| match post {
            PostFunction::Assign(to) => Change::Assign(match to {
                AssignTo::CurrentUser => Some(actor.account),
                AssignTo::Reporter => ticket.reporter,
                AssignTo::Member(member) => Some(*member),
                AssignTo::Nobody => None,
            }),
            PostFunction::SetField { field, value } => Change::SetField {
                field: *field,
                value: value.clone(),
            },
            PostFunction::ClearField { field } => Change::ClearField { field: *field },
            PostFunction::AddComment { text } => Change::AddComment { text: text.clone() },
        })
        .collect())
}

/// The fields of `ticket` once `changes` are applied in their order, and
/// its assignee: what a later change of the same field leaves.
pub fn apply(ticket: &Ticket, changes: &[Change]) -> Ticket {
    let mut after = ticket.clone();
    for change in changes {
        match change {
            Change::Assign(assignee) => after.assignee = *assignee,
            Change::SetField { field, value } => {
                after.fields.insert(*field, value.clone());
            }
            Change::ClearField { field } => {
                after.fields.remove(field);
            }
            Change::AddComment { .. } => {}
        }
    }
    after
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::ListValue;

    const REVIEWER: u128 = 1;
    const DEVELOPER: u128 = 2;
    const QUALITY: u128 = 10;
    const ALICE: u128 = 100;
    const BOB: u128 = 101;
    const REMAINING: u128 = 200;
    const APPLICATION: u128 = 201;
    const REASON: u128 = 202;
    const ERP: u128 = 300;

    fn actor(roles: &[u128], groups: &[u128]) -> Actor {
        Actor {
            account: ALICE,
            roles: roles.iter().copied().collect(),
            groups: groups.iter().copied().collect(),
        }
    }

    fn ticket(fields: &[(u128, Value)]) -> Ticket {
        Ticket {
            reporter: Some(BOB),
            assignee: None,
            fields: fields.iter().cloned().collect(),
        }
    }

    fn uuid(id: u128) -> String {
        let hex = format!("{id:032x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    fn number() -> Options {
        Options {
            decimals: Some(2),
            ..Options::default()
        }
    }

    fn application() -> Options {
        Options {
            values: Some(vec![ListValue {
                id: ERP,
                labels: [("en".to_owned(), "ERP".to_owned())].into(),
                active: true,
                children: Vec::new(),
            }]),
            ..Options::default()
        }
    }

    fn rules(condition: Condition, validators: &[u128], post_functions: Vec<PostFunction>) -> Rules {
        Rules {
            condition,
            validators: validators.to_vec(),
            post_functions,
        }
    }

    #[test]
    fn an_open_transition_is_for_everyone() {
        let open = Condition::default();
        assert!(open.is_open());
        assert!(allowed(&open, &actor(&[], &[])));
    }

    #[test]
    fn a_restricted_transition_needs_one_of_its_roles_or_groups() {
        // "Validate" for reviewers, or the quality group.
        let validate = Condition {
            roles: vec![REVIEWER],
            groups: vec![QUALITY],
        };
        assert!(allowed(&validate, &actor(&[DEVELOPER, REVIEWER], &[])));
        assert!(allowed(&validate, &actor(&[], &[QUALITY])));
        assert!(!allowed(&validate, &actor(&[DEVELOPER], &[])));
        let roles_only = Condition {
            roles: vec![REVIEWER],
            groups: Vec::new(),
        };
        assert!(!allowed(&roles_only, &actor(&[], &[QUALITY])));
        assert_eq!(
            evaluate(
                &rules(validate, &[], Vec::new()),
                &ticket(&[]),
                &actor(&[DEVELOPER], &[])
            ),
            Err(vec![Refusal::NotAllowed])
        );
    }

    #[test]
    fn validators_name_every_field_missing_and_only_to_those_allowed() {
        // "Send to review" requires the remaining work, then the application.
        let review = rules(Condition::default(), &[REMAINING, APPLICATION], Vec::new());
        assert_eq!(
            evaluate(&review, &ticket(&[]), &actor(&[], &[])),
            Err(vec![
                Refusal::FieldRequired(REMAINING),
                Refusal::FieldRequired(APPLICATION)
            ])
        );
        let blank = ticket(&[(REMAINING, Value::Null), (APPLICATION, Value::Text(" ".into()))]);
        assert_eq!(evaluate(&review, &blank, &actor(&[], &[])).unwrap_err().len(), 2);
        let filled = ticket(&[
            (REMAINING, Value::Number(0.0)),
            (APPLICATION, Value::Text(uuid(ERP))),
        ]);
        assert_eq!(evaluate(&review, &filled, &actor(&[], &[])), Ok(Vec::new()));
        // Not allowed: the fields are not even checked.
        let restricted = Rules {
            condition: Condition {
                roles: vec![REVIEWER],
                groups: Vec::new(),
            },
            ..review
        };
        assert_eq!(
            evaluate(&restricted, &ticket(&[]), &actor(&[], &[])),
            Err(vec![Refusal::NotAllowed])
        );
    }

    #[test]
    fn assign_follows_its_target() {
        let assign = |to: AssignTo| rules(Condition::default(), &[], vec![PostFunction::Assign(to)]);
        let run = |to: AssignTo, ticket: &Ticket| evaluate(&assign(to), ticket, &actor(&[], &[])).unwrap();
        let mut current = ticket(&[]);
        current.assignee = Some(BOB);
        assert_eq!(
            run(AssignTo::CurrentUser, &current),
            [Change::Assign(Some(ALICE))]
        );
        assert_eq!(run(AssignTo::Reporter, &current), [Change::Assign(Some(BOB))]);
        assert_eq!(run(AssignTo::Member(BOB), &current), [Change::Assign(Some(BOB))]);
        assert_eq!(run(AssignTo::Nobody, &current), [Change::Assign(None)]);
        // No reporter (an import): unassigned.
        current.reporter = None;
        assert_eq!(run(AssignTo::Reporter, &current), [Change::Assign(None)]);
    }

    #[test]
    fn post_functions_run_in_their_order() {
        let start = rules(
            Condition::default(),
            &[],
            vec![
                PostFunction::Assign(AssignTo::CurrentUser),
                PostFunction::SetField {
                    field: REMAINING,
                    value: Value::Number(8.0),
                },
                PostFunction::AddComment {
                    text: "Started".into(),
                },
                PostFunction::ClearField { field: REMAINING },
                PostFunction::SetField {
                    field: APPLICATION,
                    value: Value::Text(uuid(ERP)),
                },
            ],
        );
        let before = ticket(&[(REMAINING, Value::Number(3.0))]);
        let changes = evaluate(&start, &before, &actor(&[], &[])).unwrap();
        assert_eq!(
            changes,
            [
                Change::Assign(Some(ALICE)),
                Change::SetField {
                    field: REMAINING,
                    value: Value::Number(8.0)
                },
                Change::AddComment {
                    text: "Started".into()
                },
                Change::ClearField { field: REMAINING },
                Change::SetField {
                    field: APPLICATION,
                    value: Value::Text(uuid(ERP))
                },
            ]
        );
        // The same run, the same changes.
        assert_eq!(evaluate(&start, &before, &actor(&[], &[])).unwrap(), changes);
        // Applied in order: the clear wins over the set before it.
        let after = apply(&before, &changes);
        assert_eq!(after.assignee, Some(ALICE));
        assert!(!after.fields.contains_key(&REMAINING));
        assert_eq!(after.fields[&APPLICATION], Value::Text(uuid(ERP)));
    }

    fn references<'a>(numbers: &'a Options, lists: &'a Options) -> References<'a> {
        References {
            roles: [REVIEWER, DEVELOPER].into(),
            groups: [QUALITY].into(),
            members: [ALICE, BOB].into(),
            fields: [
                (
                    REMAINING,
                    FieldRef {
                        kind: Kind::Number,
                        options: numbers,
                    },
                ),
                (
                    APPLICATION,
                    FieldRef {
                        kind: Kind::SingleSelect,
                        options: lists,
                    },
                ),
            ]
            .into(),
        }
    }

    #[test]
    fn rules_name_what_exists_and_set_fitting_values() {
        let (numbers, lists) = (number(), application());
        let refs = references(&numbers, &lists);
        let fine = rules(
            Condition {
                roles: vec![REVIEWER],
                groups: vec![QUALITY],
            },
            &[REMAINING],
            vec![
                PostFunction::Assign(AssignTo::Member(BOB)),
                PostFunction::SetField {
                    field: APPLICATION,
                    value: Value::Text(uuid(ERP)),
                },
                PostFunction::ClearField { field: REMAINING },
                PostFunction::AddComment {
                    text: "Validated".into(),
                },
            ],
        );
        assert_eq!(validate_rules(&fine, &refs), Ok(()));
        let wrong = rules(
            Condition {
                roles: vec![REVIEWER, 99, REVIEWER],
                groups: vec![98],
            },
            &[REASON, REMAINING, REMAINING],
            vec![
                PostFunction::Assign(AssignTo::Member(999)),
                PostFunction::SetField {
                    field: REMAINING,
                    value: Value::Number(1.234),
                },
                PostFunction::SetField {
                    field: REASON,
                    value: Value::Text("x".into()),
                },
                PostFunction::SetField {
                    field: APPLICATION,
                    value: Value::Null,
                },
                PostFunction::ClearField { field: REASON },
                PostFunction::AddComment { text: "  ".into() },
                PostFunction::AddComment {
                    text: "a".repeat(COMMENT_MAX_CHARS + 1),
                },
            ],
        );
        let errors: Vec<(String, &str)> = validate_rules(&wrong, &refs)
            .unwrap_err()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect();
        assert_eq!(
            errors,
            [
                ("conditions.roleIds[1]".to_owned(), "not_found"),
                ("conditions.roleIds[2]".to_owned(), "duplicate"),
                ("conditions.groupIds[0]".to_owned(), "not_found"),
                ("validators[0]".to_owned(), "not_found"),
                ("validators[2]".to_owned(), "duplicate"),
                ("postFunctions[0].accountId".to_owned(), "unknown_member"),
                ("postFunctions[1].value".to_owned(), "too_many_decimals"),
                ("postFunctions[2].fieldId".to_owned(), "not_found"),
                ("postFunctions[3].value".to_owned(), "required"),
                ("postFunctions[4].fieldId".to_owned(), "not_found"),
                ("postFunctions[5].text".to_owned(), "required"),
                ("postFunctions[6].text".to_owned(), "too_long"),
            ]
        );
        let many = rules(
            Condition::default(),
            &[],
            vec![PostFunction::Assign(AssignTo::Nobody); MAX_POST_FUNCTIONS + 1],
        );
        assert_eq!(
            validate_rules(&many, &refs).unwrap_err()[0],
            FieldError::new("postFunctions", "too_many")
        );
    }

    #[test]
    fn a_removed_field_leaves_the_rules() {
        let start = rules(
            Condition::default(),
            &[REMAINING, APPLICATION],
            vec![
                PostFunction::Assign(AssignTo::CurrentUser),
                PostFunction::SetField {
                    field: REMAINING,
                    value: Value::Number(1.0),
                },
                PostFunction::ClearField { field: REMAINING },
                PostFunction::ClearField { field: APPLICATION },
            ],
        );
        assert!(start.names_field(REMAINING));
        let rest = start.without_field(REMAINING);
        assert!(!rest.names_field(REMAINING));
        assert_eq!(rest.validators, [APPLICATION]);
        assert_eq!(
            rest.post_functions,
            [
                PostFunction::Assign(AssignTo::CurrentUser),
                PostFunction::ClearField { field: APPLICATION }
            ]
        );
        assert!(!start.names_field(REASON));
    }
}
