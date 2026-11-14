//! Tickets (specification §5, L05-US01): what a new ticket must satisfy in
//! its project's configuration.
//!
//! A ticket has a type of the configuration; a sub-task sits under a parent
//! of an allowed type, in the same project. Its custom fields follow the
//! rules of its type at creation: a hidden field takes no value, a
//! required one must have one, and every value fits its field
//! ([`crate::fields::validate_value`]). Every error at once, under the
//! field it names (`summary`, `parentId`, `fields.{id}`).

use std::collections::BTreeMap;

use crate::fields::{Kind, Mode, Options, Rules, Step, Value, format_uuid, mode, validate_value};
use crate::issue_types::TypeNode;
use crate::validation::{FieldError, Violations};

/// Longest summary of a ticket, in characters.
pub const SUMMARY_MAX_CHARS: usize = 255;

/// The priority of a ticket: a fixed list, the same everywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Highest,
    High,
    #[default]
    Medium,
    Low,
    Lowest,
}

impl Priority {
    pub const ALL: [Self; 5] = [Self::Highest, Self::High, Self::Medium, Self::Low, Self::Lowest];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Highest => "highest",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Lowest => "lowest",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == raw)
    }
}

/// The key of the ticket number `number` of a project (`PLAT-12`).
pub fn key(project_key: &str, number: u32) -> String {
    format!("{project_key}-{number}")
}

/// The project key and number a key names, if it is one (`PLAT-12`).
pub fn parse_key(raw: &str) -> Option<(&str, u32)> {
    let (project, number) = raw.rsplit_once('-')?;
    let valid_project = (2..=10).contains(&project.len())
        && project.starts_with(|c: char| c.is_ascii_uppercase())
        && project
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let number: u32 = number
        .parse()
        .ok()
        .filter(|n| *n > 0 && !number.starts_with('0'))?;
    valid_project.then_some((project, number))
}

/// A field of the configuration, with its rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldDef<'a> {
    pub id: u128,
    pub kind: Kind,
    pub options: &'a Options,
    pub rules: Rules<'a>,
}

/// The parent chosen for a new ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent {
    pub type_id: u128,
    /// Whether it is in the project of the new ticket.
    pub same_project: bool,
}

/// A ticket to create.
#[derive(Debug, Clone, PartialEq)]
pub struct NewIssue<'a> {
    pub type_id: u128,
    pub summary: &'a str,
    pub parent: Option<Parent>,
    /// The start is not after the due date (or one is missing).
    pub dates_in_order: bool,
    /// Custom field values, by field.
    pub fields: &'a BTreeMap<u128, Value>,
}

/// Whether `issue` can be created with the types and fields of its
/// project's configuration; every error at once.
pub fn validate_new(
    issue: &NewIssue<'_>,
    types: &[TypeNode],
    fields: &[FieldDef<'_>],
) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let summary = issue.summary.trim();
    if summary.is_empty() {
        v.add("summary", "required");
    } else if summary.chars().count() > SUMMARY_MAX_CHARS {
        v.add("summary", "too_long");
    }
    match types.iter().find(|t| t.id == issue.type_id) {
        None => v.add("typeId", "not_found"),
        Some(issue_type) => match issue.parent {
            None if issue_type.is_subtask => v.add("parentId", "required"),
            Some(parent) if !parent.same_project => v.add("parentId", "other_project"),
            Some(parent) if !issue_type.parents.contains(&parent.type_id) => v.add("parentId", "not_allowed"),
            None | Some(_) => {}
        },
    }
    if !issue.dates_in_order {
        v.add("dueDate", "before_start");
    }
    for (id, value) in issue.fields {
        let path = format!("fields.{}", format_uuid(*id));
        let Some(field) = fields.iter().find(|f| f.id == *id) else {
            v.add(path, "not_found");
            continue;
        };
        if mode(&field.rules, issue.type_id, Step::Creation) == Mode::Hidden {
            if !value.is_empty() {
                v.add(path, "not_applicable");
            }
            continue;
        }
        if let Err(error) = validate_value(field.kind, field.options, value, None) {
            v.add(path, error.code());
        }
    }
    for field in fields {
        let given = issue.fields.get(&field.id).is_some_and(|value| !value.is_empty());
        if !given && mode(&field.rules, issue.type_id, Step::Creation) == Mode::Required {
            v.add(format!("fields.{}", format_uuid(field.id)), "required");
        }
    }
    v.into_result()
}

/// What a ticket is when it is changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Current<'a> {
    pub type_id: u128,
    pub status_id: u128,
    /// Its custom field values, by field.
    pub fields: &'a BTreeMap<u128, Value>,
}

/// Changes to a ticket (L05-US02); what is not given stays as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueChanges<'a> {
    pub summary: Option<&'a str>,
    /// The start is not after the due date, once changed (or one is missing).
    pub dates_in_order: bool,
    /// Custom field values given, by field; [`Value::Null`] clears one.
    pub fields: &'a BTreeMap<u128, Value>,
}

/// Whether `changes` can be made to a ticket that is `current`, with the
/// rules of its type at its status: a hidden field takes no value, a
/// required one is not cleared, and every value fits its field (a
/// deactivated list value only if the ticket already had it). Only the
/// fields changed are checked: a rule added since does not block other
/// changes. Every error at once.
pub fn validate_changes(
    changes: &IssueChanges<'_>,
    current: &Current<'_>,
    fields: &[FieldDef<'_>],
) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if let Some(summary) = changes.summary {
        let summary = summary.trim();
        if summary.is_empty() {
            v.add("summary", "required");
        } else if summary.chars().count() > SUMMARY_MAX_CHARS {
            v.add("summary", "too_long");
        }
    }
    if !changes.dates_in_order {
        v.add("dueDate", "before_start");
    }
    let step = Step::Status(current.status_id);
    for (id, value) in changes.fields {
        let path = format!("fields.{}", format_uuid(*id));
        let Some(field) = fields.iter().find(|f| f.id == *id) else {
            v.add(path, "not_found");
            continue;
        };
        match mode(&field.rules, current.type_id, step) {
            Mode::Hidden if !value.is_empty() => v.add(path, "not_applicable"),
            Mode::Hidden => {}
            Mode::Required if value.is_empty() => v.add(path, "required"),
            Mode::Required | Mode::Visible => {
                if let Err(error) = validate_value(field.kind, field.options, value, current.fields.get(id)) {
                    v.add(path, error.code());
                }
            }
        }
    }
    v.into_result()
}

/// A ticket that moves to another project: the one asked for, then its
/// sub-tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moving {
    pub id: u128,
    pub type_id: u128,
    pub status_id: u128,
}

/// A type of the target configuration, with the statuses of its workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetType {
    pub node: TypeNode,
    pub statuses: Vec<u128>,
}

/// Where each moving ticket lands: its type and status in the target
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landing {
    pub id: u128,
    pub type_id: u128,
    pub status_id: u128,
}

/// The type or status of the target configuration that `item` becomes:
/// the one mapped, else the same item if the target has it.
fn landing_of(item: u128, mapping: &BTreeMap<u128, u128>, exists: impl Fn(u128) -> bool) -> Option<u128> {
    mapping
        .get(&item)
        .copied()
        .or_else(|| exists(item).then_some(item))
}

/// Where the tickets of `moving` (the ticket asked for first, then its
/// sub-tasks) land in the target configuration of `types`, with the type
/// and status mappings given. A type or status the target has keeps its
/// identity unless mapped (same configuration: nothing to map); otherwise
/// it needs a mapping (`typeMappings.{id}` / `statusMappings.{id}`
/// `required`). A mapped type must exist (`not_found`); the ticket asked
/// for lands on a type that is no sub-task, its sub-tasks on sub-task
/// types allowed under it (`not_allowed`); a status lands in the workflow
/// of its ticket's new type (`not_in_workflow`). Every error at once.
pub fn plan_move(
    moving: &[Moving],
    types: &[TargetType],
    type_mappings: &BTreeMap<u128, u128>,
    status_mappings: &BTreeMap<u128, u128>,
) -> Result<Vec<Landing>, Vec<FieldError>> {
    let mut v = Violations::new();
    let mut landings = Vec::new();
    let type_of = |id: u128| types.iter().find(|t| t.node.id == id);
    let mut root_type = None;
    for (index, ticket) in moving.iter().enumerate() {
        let type_path = format!("typeMappings.{}", format_uuid(ticket.type_id));
        let Some(target) = landing_of(ticket.type_id, type_mappings, |id| type_of(id).is_some()) else {
            v.add(type_path, "required");
            continue;
        };
        let Some(target_type) = type_of(target) else {
            v.add(type_path, "not_found");
            continue;
        };
        let allowed = if index == 0 {
            root_type = Some(target);
            !target_type.node.is_subtask
        } else {
            target_type.node.is_subtask
                && root_type.is_some_and(|root| target_type.node.parents.contains(&root))
        };
        if !allowed {
            v.add(type_path, "not_allowed");
            continue;
        }
        let status_path = format!("statusMappings.{}", format_uuid(ticket.status_id));
        match status_mappings.get(&ticket.status_id) {
            Some(status) if target_type.statuses.contains(status) => landings.push(Landing {
                id: ticket.id,
                type_id: target,
                status_id: *status,
            }),
            Some(_) => v.add(status_path, "not_in_workflow"),
            None if target_type.statuses.contains(&ticket.status_id) => landings.push(Landing {
                id: ticket.id,
                type_id: target,
                status_id: ticket.status_id,
            }),
            None => v.add(status_path, "required"),
        }
    }
    v.into_result().map(|()| landings)
}

/// The custom field values a ticket keeps once of `type_id` in the target
/// configuration of `fields`: those of a field the target has, that
/// applies to the type, and whose value fits it there. The others are
/// dropped: their field ids, in order.
pub fn kept_values(
    values: &BTreeMap<u128, Value>,
    type_id: u128,
    fields: &[FieldDef<'_>],
) -> (BTreeMap<u128, Value>, Vec<u128>) {
    let mut kept = BTreeMap::new();
    let mut dropped = Vec::new();
    for (id, value) in values {
        let fits = fields.iter().find(|f| f.id == *id).is_some_and(|field| {
            field.rules.applies_to(type_id)
                && validate_value(field.kind, field.options, value, Some(value)).is_ok()
        });
        if fits {
            kept.insert(*id, value.clone());
        } else {
            dropped.push(*id);
        }
    }
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{ListValue, Rule};

    const EPIC: u128 = 1;
    const STORY: u128 = 2;
    const BUG: u128 = 3;
    const SUBTASK: u128 = 4;
    const APPLICATION: u128 = 10;
    const POINTS: u128 = 11;
    const ENVIRONMENT: u128 = 12;
    const DIRECTORY: u128 = 100;

    fn types() -> Vec<TypeNode> {
        vec![
            TypeNode {
                id: EPIC,
                is_subtask: false,
                parents: Vec::new(),
            },
            TypeNode {
                id: STORY,
                is_subtask: false,
                parents: vec![EPIC],
            },
            TypeNode {
                id: BUG,
                is_subtask: false,
                parents: vec![EPIC],
            },
            TypeNode {
                id: SUBTASK,
                is_subtask: true,
                parents: vec![STORY, BUG],
            },
        ]
    }

    fn new<'a>(type_id: u128, fields: &'a BTreeMap<u128, Value>) -> NewIssue<'a> {
        NewIssue {
            type_id,
            summary: "Migrate the authentication to OIDC",
            parent: None,
            dates_in_order: true,
            fields,
        }
    }

    fn errors(result: Result<(), Vec<FieldError>>) -> Vec<(String, &'static str)> {
        result
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn priorities_and_keys_round_trip() {
        for priority in Priority::ALL {
            assert_eq!(Priority::parse(priority.as_str()), Some(priority));
        }
        assert_eq!(Priority::default(), Priority::Medium);
        assert_eq!(key("PLAT", 142), "PLAT-142");
        assert_eq!(parse_key("PLAT-142"), Some(("PLAT", 142)));
        assert_eq!(parse_key("CRM2-7"), Some(("CRM2", 7)));
        for bad in [
            "PLAT",
            "PLAT-0",
            "PLAT-012",
            "plat-1",
            "P-1",
            "PLAT-x",
            "2PLAT-1",
            "ABCDEFGHIJK-1",
        ] {
            assert_eq!(parse_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_ticket_has_a_summary_a_type_of_the_configuration_and_ordered_dates() {
        let none = BTreeMap::new();
        assert_eq!(validate_new(&new(STORY, &none), &types(), &[]), Ok(()));
        let wrong = NewIssue {
            type_id: 99,
            summary: "  ",
            dates_in_order: false,
            ..new(STORY, &none)
        };
        assert_eq!(
            errors(validate_new(&wrong, &types(), &[])),
            [
                ("summary".to_owned(), "required"),
                ("typeId".to_owned(), "not_found"),
                ("dueDate".to_owned(), "before_start"),
            ]
        );
        let long = "x".repeat(SUMMARY_MAX_CHARS + 1);
        let too_long = NewIssue {
            summary: &long,
            ..new(STORY, &none)
        };
        assert_eq!(
            errors(validate_new(&too_long, &types(), &[])),
            [("summary".to_owned(), "too_long")]
        );
    }

    #[test]
    fn a_sub_task_sits_under_an_allowed_parent_of_its_project() {
        let none = BTreeMap::new();
        let under = |type_id, same_project| NewIssue {
            parent: Some(Parent {
                type_id,
                same_project,
            }),
            ..new(SUBTASK, &none)
        };
        assert_eq!(validate_new(&under(STORY, true), &types(), &[]), Ok(()));
        assert_eq!(
            errors(validate_new(&new(SUBTASK, &none), &types(), &[])),
            [("parentId".to_owned(), "required")]
        );
        assert_eq!(
            errors(validate_new(&under(EPIC, true), &types(), &[])),
            [("parentId".to_owned(), "not_allowed")]
        );
        assert_eq!(
            errors(validate_new(&under(STORY, false), &types(), &[])),
            [("parentId".to_owned(), "other_project")]
        );
        // A story may sit under an epic, or alone.
        let story_under_epic = NewIssue {
            parent: Some(Parent {
                type_id: EPIC,
                same_project: true,
            }),
            ..new(STORY, &none)
        };
        assert_eq!(validate_new(&story_under_epic, &types(), &[]), Ok(()));
    }

    #[test]
    fn custom_fields_follow_their_rules_at_creation_and_fit_their_type() {
        let application_options = Options {
            values: Some(vec![ListValue {
                id: DIRECTORY,
                labels: [("en".to_owned(), "Directory".to_owned())].into(),
                active: true,
                children: Vec::new(),
            }]),
            ..Options::default()
        };
        let points_options = Options {
            decimals: Some(1),
            ..Options::default()
        };
        let none = Options::default();
        // Application: required for bugs at creation. Points: stories only.
        // Environment: hidden for stories at creation.
        let bug_required = [Rule {
            issue_type: BUG,
            step: Step::Creation,
            mode: Mode::Required,
        }];
        let story_hidden = [Rule {
            issue_type: STORY,
            step: Step::Creation,
            mode: Mode::Hidden,
        }];
        let fields = [
            FieldDef {
                id: APPLICATION,
                kind: Kind::SingleSelect,
                options: &application_options,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &bug_required,
                },
            },
            FieldDef {
                id: POINTS,
                kind: Kind::Number,
                options: &points_options,
                rules: Rules {
                    all_types: false,
                    issue_types: &[STORY],
                    rules: &[],
                },
            },
            FieldDef {
                id: ENVIRONMENT,
                kind: Kind::ShortText,
                options: &none,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &story_hidden,
                },
            },
        ];
        let directory = Value::Text(format_uuid(DIRECTORY));
        // A bug without its application: refused, the field named.
        let empty = BTreeMap::new();
        assert_eq!(
            errors(validate_new(&new(BUG, &empty), &types(), &fields)),
            [(format!("fields.{}", format_uuid(APPLICATION)), "required")]
        );
        let bug: BTreeMap<u128, Value> = [
            (APPLICATION, directory.clone()),
            (ENVIRONMENT, Value::Text("Staging".into())),
        ]
        .into();
        assert_eq!(validate_new(&new(BUG, &bug), &types(), &fields), Ok(()));
        // A story: points fit their decimals, the environment is hidden.
        let story: BTreeMap<u128, Value> = [
            (POINTS, Value::Number(2.25)),
            (ENVIRONMENT, Value::Text("Staging".into())),
            (99, Value::Text("x".into())),
        ]
        .into();
        assert_eq!(
            errors(validate_new(&new(STORY, &story), &types(), &fields)),
            [
                (format!("fields.{}", format_uuid(POINTS)), "too_many_decimals"),
                (format!("fields.{}", format_uuid(ENVIRONMENT)), "not_applicable"),
                (format!("fields.{}", format_uuid(99)), "not_found"),
            ]
        );
        // A hidden field sent empty, or a field not of the type: no value, fine.
        let blank: BTreeMap<u128, Value> = [(ENVIRONMENT, Value::Null)].into();
        assert_eq!(validate_new(&new(STORY, &blank), &types(), &fields), Ok(()));
        let points_on_bug: BTreeMap<u128, Value> =
            [(APPLICATION, directory), (POINTS, Value::Number(3.0))].into();
        assert_eq!(
            errors(validate_new(&new(BUG, &points_on_bug), &types(), &fields)),
            [(format!("fields.{}", format_uuid(POINTS)), "not_applicable")]
        );
    }

    const TODO: u128 = 20;
    const IN_PROGRESS: u128 = 21;
    const IN_REVIEW: u128 = 22;
    const DONE: u128 = 23;
    const MAIL: u128 = 101;

    fn application_options() -> Options {
        let value = |id, en: &str, active| ListValue {
            id,
            labels: [("en".to_owned(), en.to_owned())].into(),
            active,
            children: Vec::new(),
        };
        Options {
            values: Some(vec![
                value(DIRECTORY, "Directory", true),
                value(MAIL, "Mail", false),
            ]),
            ..Options::default()
        }
    }

    #[test]
    fn changes_follow_the_rules_of_the_type_at_its_status() {
        let options = application_options();
        let none = Options::default();
        // Application: required for stories once done. Environment: hidden
        // for stories in review.
        let required_done = [Rule {
            issue_type: STORY,
            step: Step::Status(DONE),
            mode: Mode::Required,
        }];
        let hidden_review = [Rule {
            issue_type: STORY,
            step: Step::Status(IN_REVIEW),
            mode: Mode::Hidden,
        }];
        let fields = [
            FieldDef {
                id: APPLICATION,
                kind: Kind::SingleSelect,
                options: &options,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &required_done,
                },
            },
            FieldDef {
                id: ENVIRONMENT,
                kind: Kind::ShortText,
                options: &none,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &hidden_review,
                },
            },
        ];
        let had_mail: BTreeMap<u128, Value> = [(APPLICATION, Value::Text(format_uuid(MAIL)))].into();
        let current = |status_id| Current {
            type_id: STORY,
            status_id,
            fields: &had_mail,
        };
        fn change<'a>(fields: &'a BTreeMap<u128, Value>, summary: Option<&'a str>) -> IssueChanges<'a> {
            IssueChanges {
                summary,
                dates_in_order: true,
                fields,
            }
        }
        let path = |id| format!("fields.{}", format_uuid(id));

        // Nothing given: nothing to check, even a required field left empty.
        let nothing = BTreeMap::new();
        assert_eq!(
            validate_changes(&change(&nothing, None), &current(DONE), &fields),
            Ok(())
        );
        // A deactivated value is kept, never newly chosen.
        assert_eq!(
            validate_changes(&change(&had_mail, None), &current(IN_PROGRESS), &fields),
            Ok(())
        );
        let empty: BTreeMap<u128, Value> = BTreeMap::new();
        let fresh = Current {
            fields: &empty,
            ..current(IN_PROGRESS)
        };
        assert_eq!(
            errors(validate_changes(&change(&had_mail, None), &fresh, &fields)),
            [(path(APPLICATION), "inactive_value")]
        );
        // Required once done: not cleared. Hidden in review: not set, but cleared.
        let cleared: BTreeMap<u128, Value> = [(APPLICATION, Value::Null), (ENVIRONMENT, Value::Null)].into();
        assert_eq!(
            errors(validate_changes(&change(&cleared, None), &current(DONE), &fields)),
            [(path(APPLICATION), "required")]
        );
        assert_eq!(
            validate_changes(&change(&cleared, None), &current(IN_REVIEW), &fields),
            Ok(())
        );
        let environment: BTreeMap<u128, Value> = [
            (ENVIRONMENT, Value::Text("Staging".into())),
            (99, Value::Bool(true)),
        ]
        .into();
        assert_eq!(
            errors(validate_changes(
                &change(&environment, Some(" ")),
                &current(IN_REVIEW),
                &fields
            )),
            [
                ("summary".to_owned(), "required"),
                (path(ENVIRONMENT), "not_applicable"),
                (path(99), "not_found"),
            ]
        );
        let unordered = IssueChanges {
            dates_in_order: false,
            ..change(&nothing, Some("Renamed"))
        };
        assert_eq!(
            errors(validate_changes(&unordered, &current(TODO), &fields)),
            [("dueDate".to_owned(), "before_start")]
        );
    }

    /// Story and bug follow a workflow of To do, In progress, Done; the
    /// sub-task one of To do, Done.
    fn target() -> Vec<TargetType> {
        types()
            .into_iter()
            .map(|node| TargetType {
                statuses: if node.is_subtask {
                    vec![TODO, DONE]
                } else {
                    vec![TODO, IN_PROGRESS, DONE]
                },
                node,
            })
            .collect()
    }

    #[test]
    fn a_move_within_the_same_configuration_needs_no_mapping() {
        let moving = [
            Moving {
                id: 1000,
                type_id: STORY,
                status_id: IN_PROGRESS,
            },
            Moving {
                id: 1001,
                type_id: SUBTASK,
                status_id: DONE,
            },
        ];
        let none = BTreeMap::new();
        assert_eq!(
            plan_move(&moving, &target(), &none, &none),
            Ok(vec![
                Landing {
                    id: 1000,
                    type_id: STORY,
                    status_id: IN_PROGRESS
                },
                Landing {
                    id: 1001,
                    type_id: SUBTASK,
                    status_id: DONE
                },
            ])
        );
    }

    #[test]
    fn a_move_to_another_configuration_maps_what_it_lacks() {
        const TASK: u128 = 5;
        const REVIEW_TASK: u128 = 6;
        // From another configuration: a task in review, with a review sub-task.
        let moving = [
            Moving {
                id: 1000,
                type_id: TASK,
                status_id: IN_REVIEW,
            },
            Moving {
                id: 1001,
                type_id: REVIEW_TASK,
                status_id: IN_REVIEW,
            },
        ];
        let none = BTreeMap::new();
        let path = |kind: &str, id| format!("{kind}Mappings.{}", format_uuid(id));
        assert_eq!(
            errors(plan_move(&moving, &target(), &none, &none).map(|_| ())),
            [
                (path("type", TASK), "required"),
                (path("type", REVIEW_TASK), "required")
            ]
        );
        // Types mapped, the status not yet; then to a status out of the workflow.
        let types: BTreeMap<u128, u128> = [(TASK, STORY), (REVIEW_TASK, SUBTASK)].into();
        assert_eq!(
            errors(plan_move(&moving, &target(), &types, &none).map(|_| ())),
            [
                (path("status", IN_REVIEW), "required"),
                (path("status", IN_REVIEW), "required")
            ]
        );
        let to_progress: BTreeMap<u128, u128> = [(IN_REVIEW, IN_PROGRESS)].into();
        assert_eq!(
            errors(plan_move(&moving, &target(), &types, &to_progress).map(|_| ())),
            [(path("status", IN_REVIEW), "not_in_workflow")]
        );
        let to_done: BTreeMap<u128, u128> = [(IN_REVIEW, DONE)].into();
        assert_eq!(
            plan_move(&moving, &target(), &types, &to_done).unwrap(),
            [
                Landing {
                    id: 1000,
                    type_id: STORY,
                    status_id: DONE
                },
                Landing {
                    id: 1001,
                    type_id: SUBTASK,
                    status_id: DONE
                },
            ]
        );
        // The ticket on a sub-task type, a sub-task under a type that holds
        // none, or on a type the target lacks.
        let wrong: BTreeMap<u128, u128> = [(TASK, SUBTASK), (REVIEW_TASK, 77)].into();
        assert_eq!(
            errors(plan_move(&moving, &target(), &wrong, &to_done).map(|_| ())),
            [
                (path("type", TASK), "not_allowed"),
                (path("type", REVIEW_TASK), "not_found")
            ]
        );
        let under_epic: BTreeMap<u128, u128> = [(TASK, EPIC), (REVIEW_TASK, SUBTASK)].into();
        assert_eq!(
            errors(plan_move(&moving, &target(), &under_epic, &to_done).map(|_| ())),
            [(path("type", REVIEW_TASK), "not_allowed")]
        );
    }

    #[test]
    fn values_the_target_cannot_hold_are_dropped() {
        let options = application_options();
        let none = Options::default();
        let stories = [STORY];
        let fields = [
            FieldDef {
                id: APPLICATION,
                kind: Kind::SingleSelect,
                options: &options,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &[],
                },
            },
            FieldDef {
                id: POINTS,
                kind: Kind::Number,
                options: &none,
                rules: Rules {
                    all_types: false,
                    issue_types: &stories,
                    rules: &[],
                },
            },
        ];
        let values: BTreeMap<u128, Value> = [
            (APPLICATION, Value::Text(format_uuid(MAIL))),
            (POINTS, Value::Number(3.0)),
            (ENVIRONMENT, Value::Text("Staging".into())),
        ]
        .into();
        // A story keeps its application (even deactivated) and points; the
        // environment field is not in the target.
        let (kept, dropped) = kept_values(&values, STORY, &fields);
        assert_eq!(kept.keys().copied().collect::<Vec<_>>(), [APPLICATION, POINTS]);
        assert_eq!(dropped, [ENVIRONMENT]);
        // A bug: points do not apply. An unknown list value is dropped too.
        let unknown: BTreeMap<u128, Value> = [(APPLICATION, Value::Text(format_uuid(999)))].into();
        assert_eq!(kept_values(&values, BUG, &fields).1, [POINTS, ENVIRONMENT]);
        assert_eq!(kept_values(&unknown, STORY, &fields).1, [APPLICATION]);
    }
}
