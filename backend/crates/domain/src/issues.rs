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
}
