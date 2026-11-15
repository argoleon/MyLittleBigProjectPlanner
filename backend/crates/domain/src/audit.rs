//! The history of tickets (specification §12, §18, L05-US06): every write
//! records, in its own transaction, who changed what, when and from where,
//! field by field.
//!
//! A write compares the ticket before and after ([`diff`]) and records one
//! entry per changed field, with its old and new values; other actions
//! (creation, move, links, comments, attachments) record one entry each.
//! The Activity tab turns the entries into sentences.

use std::collections::BTreeMap;

/// Where a change came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Origin {
    /// The web application, through a session.
    #[default]
    Interface,
    /// The REST API, with a token.
    Api,
    /// The MCP server, for an AI client.
    Mcp,
    /// An import.
    Import,
    /// A background job (configuration migration…), on nobody's behalf.
    System,
}

impl Origin {
    pub const ALL: [Self; 5] = [Self::Interface, Self::Api, Self::Mcp, Self::Import, Self::System];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interface => "interface",
            Self::Api => "api",
            Self::Mcp => "mcp",
            Self::Import => "import",
            Self::System => "system",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == raw)
    }
}

/// What an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The ticket was created.
    Created,
    /// A field changed: its old and new values.
    Updated,
    /// The ticket moved to another project: its old and new keys.
    Moved,
    /// A link to another ticket was added (new value) or removed (old one).
    LinkAdded,
    LinkRemoved,
    /// A comment was written, edited or deleted.
    CommentCreated,
    CommentUpdated,
    CommentDeleted,
    /// A file was attached (new value) or removed (old one).
    AttachmentAdded,
    AttachmentRemoved,
}

impl Action {
    pub const ALL: [Self; 10] = [
        Self::Created,
        Self::Updated,
        Self::Moved,
        Self::LinkAdded,
        Self::LinkRemoved,
        Self::CommentCreated,
        Self::CommentUpdated,
        Self::CommentDeleted,
        Self::AttachmentAdded,
        Self::AttachmentRemoved,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Moved => "moved",
            Self::LinkAdded => "link_added",
            Self::LinkRemoved => "link_removed",
            Self::CommentCreated => "comment_created",
            Self::CommentUpdated => "comment_updated",
            Self::CommentDeleted => "comment_deleted",
            Self::AttachmentAdded => "attachment_added",
            Self::AttachmentRemoved => "attachment_removed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == raw)
    }

    /// Whether it concerns a comment, whose content the entry holds.
    pub const fn is_comment(self) -> bool {
        matches!(
            self,
            Self::CommentCreated | Self::CommentUpdated | Self::CommentDeleted
        )
    }
}

/// A field of a ticket, in the order the history lists the changes of one
/// write: system fields first, then custom fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Type,
    Status,
    Summary,
    Description,
    Priority,
    Reporter,
    Assignee,
    StartDate,
    DueDate,
    Parent,
    /// A custom field, by id.
    Custom(u128),
}

impl Field {
    const SYSTEM: [Self; 10] = [
        Self::Type,
        Self::Status,
        Self::Summary,
        Self::Description,
        Self::Priority,
        Self::Reporter,
        Self::Assignee,
        Self::StartDate,
        Self::DueDate,
        Self::Parent,
    ];

    /// The name stored: `status`, `dueDate`…, `fields.{id}` for a custom
    /// field (the id in its hyphenated form, built by the caller).
    pub const fn system_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Type => "type",
            Self::Status => "status",
            Self::Summary => "summary",
            Self::Description => "description",
            Self::Priority => "priority",
            Self::Reporter => "reporter",
            Self::Assignee => "assignee",
            Self::StartDate => "startDate",
            Self::DueDate => "dueDate",
            Self::Parent => "parent",
            Self::Custom(_) => return None,
        })
    }

    /// The system field named `raw`; custom fields are parsed by the caller.
    pub fn parse_system(raw: &str) -> Option<Self> {
        Self::SYSTEM.into_iter().find(|f| f.system_name() == Some(raw))
    }
}

/// The values of a ticket's fields at one moment: an absent field holds
/// nothing, like a field holding `V::default()` (null).
pub type Snapshot<V> = BTreeMap<Field, V>;

/// One changed field.
#[derive(Debug, Clone, PartialEq)]
pub struct Change<V> {
    pub field: Field,
    /// `V::default()` when it held nothing.
    pub old: V,
    pub new: V,
}

/// The fields whose value differs between `before` and `after`, in field
/// order. A field absent from one side holds `V::default()` there, so
/// clearing a field and removing it are the same change, and a field absent
/// from both never changes.
pub fn diff<V: Clone + Default + PartialEq>(before: &Snapshot<V>, after: &Snapshot<V>) -> Vec<Change<V>> {
    let empty = V::default();
    let mut fields: Vec<Field> = before.keys().chain(after.keys()).copied().collect();
    fields.sort_unstable();
    fields.dedup();
    fields
        .into_iter()
        .filter_map(|field| {
            let old = before.get(&field).unwrap_or(&empty);
            let new = after.get(&field).unwrap_or(&empty);
            (old != new).then(|| Change {
                field,
                old: old.clone(),
                new: new.clone(),
            })
        })
        .collect()
}

/// Whether the member reading the history sees the content of a deleted
/// comment: moderators only (`administer_node` on the project, or tenant
/// admins), who could delete it themselves.
pub const fn shows_deleted_comment(moderates: bool) -> bool {
    moderates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::Value;

    fn snapshot(values: &[(Field, Value)]) -> Snapshot<Value> {
        values.iter().cloned().collect()
    }

    fn text(raw: &str) -> Value {
        Value::Text(raw.to_owned())
    }

    #[test]
    fn origins_and_actions_round_trip_through_their_names() {
        for origin in Origin::ALL {
            assert_eq!(Origin::parse(origin.as_str()), Some(origin));
        }
        for action in Action::ALL {
            assert_eq!(Action::parse(action.as_str()), Some(action));
        }
        assert_eq!(Origin::parse("web"), None);
        assert_eq!(Origin::default(), Origin::Interface);
        assert!(Action::CommentDeleted.is_comment());
        assert!(!Action::Updated.is_comment());
    }

    #[test]
    fn system_fields_round_trip_custom_ones_have_no_system_name() {
        for field in Field::SYSTEM {
            assert_eq!(Field::parse_system(field.system_name().unwrap()), Some(field));
        }
        assert_eq!(Field::Custom(7).system_name(), None);
        assert_eq!(Field::parse_system("fields.x"), None);
    }

    #[test]
    fn nothing_changed_nothing_listed() {
        let ticket = snapshot(&[
            (Field::Summary, text("Migrate")),
            (Field::Custom(1), Value::Number(3.0)),
        ]);
        assert!(diff(&ticket, &ticket.clone()).is_empty());
    }

    #[test]
    fn system_fields_changed_list_old_and_new() {
        let before = snapshot(&[
            (Field::Status, text("todo")),
            (Field::Summary, text("Migrate")),
            (Field::Assignee, Value::Null),
        ]);
        let after = snapshot(&[
            (Field::Status, text("doing")),
            (Field::Summary, text("Migrate")),
            (Field::Assignee, text("lucas")),
        ]);
        assert_eq!(
            diff(&before, &after),
            vec![
                Change {
                    field: Field::Status,
                    old: text("todo"),
                    new: text("doing")
                },
                Change {
                    field: Field::Assignee,
                    old: Value::Null,
                    new: text("lucas")
                },
            ]
        );
    }

    #[test]
    fn custom_fields_set_cleared_and_changed_follow_the_system_ones() {
        let before = snapshot(&[
            (Field::Custom(2), text("a")),
            (Field::Custom(3), Value::List(vec![text("x")])),
            (Field::DueDate, text("2026-11-30")),
        ]);
        let after = snapshot(&[
            (Field::Custom(1), Value::Bool(false)),
            (Field::Custom(3), Value::List(vec![text("x"), text("y")])),
            (Field::DueDate, text("2026-12-04")),
        ]);
        let fields: Vec<Field> = diff(&before, &after).into_iter().map(|c| c.field).collect();
        assert_eq!(
            fields,
            vec![
                Field::DueDate,
                Field::Custom(1),
                Field::Custom(2),
                Field::Custom(3)
            ]
        );
        let cleared = diff(&before, &after)
            .into_iter()
            .find(|c| c.field == Field::Custom(2))
            .unwrap();
        assert_eq!((cleared.old, cleared.new), (text("a"), Value::Null));
    }

    #[test]
    fn an_absent_field_is_the_same_as_a_null_one() {
        let before = snapshot(&[(Field::Custom(1), Value::Null)]);
        let after = snapshot(&[]);
        assert!(diff(&before, &after).is_empty());
        // An unticked checkbox holds a value: false is not nothing.
        let unticked = snapshot(&[(Field::Custom(1), Value::Bool(false))]);
        assert_eq!(diff(&after, &unticked).len(), 1);
    }

    #[test]
    fn only_moderators_see_a_deleted_comment() {
        assert!(shows_deleted_comment(true));
        assert!(!shows_deleted_comment(false));
    }
}
