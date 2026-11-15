//! Links between tickets and the parent of a ticket (specification §5,
//! L05-US03).
//!
//! Four kinds of link, stored from one ticket (the source) to another (the
//! target), each read one way from the source and the other way from the
//! target: blocks / blocked by, depends on / required by, duplicates /
//! duplicated by, relates to (the same both ways). A ticket is never
//! linked to itself, and two tickets have each kind of link once (a
//! "relates to" whatever its direction).
//!
//! A ticket's parent is in its project, of a type its type may sit under,
//! and never the ticket itself or one below it; a sub-task always has one.

use crate::issue_types::TypeNode;

/// A kind of link, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkType {
    Blocks,
    DependsOn,
    Duplicates,
    RelatesTo,
}

impl LinkType {
    pub const ALL: [Self; 4] = [Self::Blocks, Self::DependsOn, Self::Duplicates, Self::RelatesTo];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::DependsOn => "depends_on",
            Self::Duplicates => "duplicates",
            Self::RelatesTo => "relates_to",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == raw)
    }

    /// Read the same way from both tickets.
    pub const fn is_symmetric(self) -> bool {
        matches!(self, Self::RelatesTo)
    }
}

/// How a link reads from one of its tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    Blocks,
    BlockedBy,
    DependsOn,
    RequiredBy,
    Duplicates,
    DuplicatedBy,
    RelatesTo,
}

impl Relation {
    pub const ALL: [Self; 7] = [
        Self::Blocks,
        Self::BlockedBy,
        Self::DependsOn,
        Self::RequiredBy,
        Self::Duplicates,
        Self::DuplicatedBy,
        Self::RelatesTo,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::BlockedBy => "blocked_by",
            Self::DependsOn => "depends_on",
            Self::RequiredBy => "required_by",
            Self::Duplicates => "duplicates",
            Self::DuplicatedBy => "duplicated_by",
            Self::RelatesTo => "relates_to",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == raw)
    }

    /// How a link of `link_type` reads from its source (`from_source`) or
    /// its target.
    pub const fn of(link_type: LinkType, from_source: bool) -> Self {
        match (link_type, from_source) {
            (LinkType::Blocks, true) => Self::Blocks,
            (LinkType::Blocks, false) => Self::BlockedBy,
            (LinkType::DependsOn, true) => Self::DependsOn,
            (LinkType::DependsOn, false) => Self::RequiredBy,
            (LinkType::Duplicates, true) => Self::Duplicates,
            (LinkType::Duplicates, false) => Self::DuplicatedBy,
            (LinkType::RelatesTo, _) => Self::RelatesTo,
        }
    }

    /// The link to store when a ticket says it is in this relation with
    /// another: its type, and whether the ticket is the source.
    pub const fn stored(self) -> (LinkType, bool) {
        match self {
            Self::Blocks => (LinkType::Blocks, true),
            Self::BlockedBy => (LinkType::Blocks, false),
            Self::DependsOn => (LinkType::DependsOn, true),
            Self::RequiredBy => (LinkType::DependsOn, false),
            Self::Duplicates => (LinkType::Duplicates, true),
            Self::DuplicatedBy => (LinkType::Duplicates, false),
            Self::RelatesTo => (LinkType::RelatesTo, true),
        }
    }
}

/// A link as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub source: u128,
    pub target: u128,
    pub link_type: LinkType,
}

impl Link {
    /// The link `ticket` asks for with `other` in `relation`.
    pub const fn new(ticket: u128, relation: Relation, other: u128) -> Self {
        let (link_type, from_source) = relation.stored();
        let (source, target) = if from_source {
            (ticket, other)
        } else {
            (other, ticket)
        };
        Self {
            source,
            target,
            link_type,
        }
    }

    /// Whether it joins the same tickets the same way as `other`; a
    /// "relates to" in either direction.
    pub fn same_as(&self, other: &Self) -> bool {
        self.link_type == other.link_type
            && ((self.source == other.source && self.target == other.target)
                || (self.link_type.is_symmetric()
                    && self.source == other.target
                    && self.target == other.source))
    }
}

/// Why a link is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// A ticket linked to itself.
    ToItself,
    /// The two tickets already have this link.
    Exists,
}

impl LinkError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ToItself => "self",
            Self::Exists => "exists",
        }
    }
}

/// Whether `link` can join its tickets, given the links they already have.
pub fn validate_link(link: &Link, existing: &[Link]) -> Result<(), LinkError> {
    if link.source == link.target {
        return Err(LinkError::ToItself);
    }
    if existing.iter().any(|e| e.same_as(link)) {
        return Err(LinkError::Exists);
    }
    Ok(())
}

/// The parent asked for a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewParent {
    pub id: u128,
    pub type_id: u128,
    pub same_project: bool,
    /// The parent sits (at any depth) under the ticket.
    pub is_below_ticket: bool,
}

/// Why a parent is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentError {
    /// A sub-task without a parent.
    Required,
    /// The ticket's type may not sit under the parent's.
    NotAllowed,
    /// The parent is in another project.
    OtherProject,
    /// The parent is the ticket itself or one below it.
    Cycle,
}

impl ParentError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::NotAllowed => "not_allowed",
            Self::OtherProject => "other_project",
            Self::Cycle => "cycle",
        }
    }
}

/// Whether the ticket `ticket` of type `type_id` may sit under `parent`
/// (`None`: at the top), with the types of its configuration.
pub fn validate_parent(
    ticket: u128,
    type_id: u128,
    parent: Option<&NewParent>,
    types: &[TypeNode],
) -> Result<(), ParentError> {
    let own = types.iter().find(|t| t.id == type_id);
    let Some(parent) = parent else {
        return if own.is_some_and(|t| t.is_subtask) {
            Err(ParentError::Required)
        } else {
            Ok(())
        };
    };
    if parent.id == ticket || parent.is_below_ticket {
        return Err(ParentError::Cycle);
    }
    if !parent.same_project {
        return Err(ParentError::OtherProject);
    }
    if !own.is_some_and(|t| t.parents.contains(&parent.type_id)) {
        return Err(ParentError::NotAllowed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_link_reads_one_way_from_its_source_and_the_other_from_its_target() {
        for link_type in LinkType::ALL {
            assert_eq!(LinkType::parse(link_type.as_str()), Some(link_type));
            for from_source in [true, false] {
                let relation = Relation::of(link_type, from_source);
                assert_eq!(Relation::parse(relation.as_str()), Some(relation));
                assert_eq!(relation.stored().0, link_type);
            }
        }
        let labels = |t| (Relation::of(t, true).as_str(), Relation::of(t, false).as_str());
        assert_eq!(labels(LinkType::Blocks), ("blocks", "blocked_by"));
        assert_eq!(labels(LinkType::DependsOn), ("depends_on", "required_by"));
        assert_eq!(labels(LinkType::Duplicates), ("duplicates", "duplicated_by"));
        assert_eq!(labels(LinkType::RelatesTo), ("relates_to", "relates_to"));
        assert_eq!(Relation::parse("blocked"), None);
    }

    #[test]
    fn a_link_asked_from_its_target_is_stored_from_the_other_ticket() {
        // CRM-224 says it is blocked by PLAT-142: PLAT-142 blocks CRM-224.
        assert_eq!(
            Link::new(224, Relation::BlockedBy, 142),
            Link {
                source: 142,
                target: 224,
                link_type: LinkType::Blocks
            }
        );
        assert_eq!(
            Link::new(142, Relation::RelatesTo, 224),
            Link {
                source: 142,
                target: 224,
                link_type: LinkType::RelatesTo
            }
        );
    }

    #[test]
    fn a_ticket_is_never_linked_to_itself_nor_twice_the_same_way() {
        let blocks = Link::new(1, Relation::Blocks, 2);
        assert_eq!(
            validate_link(&Link::new(1, Relation::Blocks, 1), &[]),
            Err(LinkError::ToItself)
        );
        assert_eq!(validate_link(&blocks, &[]), Ok(()));
        assert_eq!(validate_link(&blocks, &[blocks]), Err(LinkError::Exists));
        // Asked from the other side: the same link.
        assert_eq!(
            validate_link(&Link::new(2, Relation::BlockedBy, 1), &[blocks]),
            Err(LinkError::Exists)
        );
        // The other way round, or another kind: another link.
        assert_eq!(
            validate_link(&Link::new(2, Relation::Blocks, 1), &[blocks]),
            Ok(())
        );
        assert_eq!(
            validate_link(&Link::new(1, Relation::DependsOn, 2), &[blocks]),
            Ok(())
        );
        // "Relates to" is the same whatever its direction.
        let relates = Link::new(1, Relation::RelatesTo, 2);
        assert_eq!(
            validate_link(&Link::new(2, Relation::RelatesTo, 1), &[relates]),
            Err(LinkError::Exists)
        );
        assert_eq!(LinkError::Exists.code(), "exists");
    }

    fn types() -> Vec<TypeNode> {
        vec![
            TypeNode {
                id: 1,
                is_subtask: false,
                parents: Vec::new(),
            },
            TypeNode {
                id: 2,
                is_subtask: false,
                parents: vec![1],
            },
            TypeNode {
                id: 4,
                is_subtask: true,
                parents: vec![2],
            },
        ]
    }

    #[test]
    fn a_parent_is_in_the_project_of_an_allowed_type_and_never_below_the_ticket() {
        const EPIC: u128 = 1;
        const STORY: u128 = 2;
        const SUBTASK: u128 = 4;
        let parent = |id, type_id| NewParent {
            id,
            type_id,
            same_project: true,
            is_below_ticket: false,
        };
        // A story under an epic, or at the top; a sub-task under a story only.
        assert_eq!(
            validate_parent(10, STORY, Some(&parent(20, EPIC)), &types()),
            Ok(())
        );
        assert_eq!(validate_parent(10, STORY, None, &types()), Ok(()));
        assert_eq!(
            validate_parent(10, SUBTASK, None, &types()),
            Err(ParentError::Required)
        );
        assert_eq!(
            validate_parent(10, SUBTASK, Some(&parent(20, EPIC)), &types()),
            Err(ParentError::NotAllowed)
        );
        let elsewhere = NewParent {
            same_project: false,
            ..parent(20, EPIC)
        };
        assert_eq!(
            validate_parent(10, STORY, Some(&elsewhere), &types()),
            Err(ParentError::OtherProject)
        );
        // Itself, or one below it.
        assert_eq!(
            validate_parent(10, STORY, Some(&parent(10, EPIC)), &types()),
            Err(ParentError::Cycle)
        );
        let below = NewParent {
            is_below_ticket: true,
            ..parent(30, EPIC)
        };
        assert_eq!(
            validate_parent(10, STORY, Some(&below), &types()),
            Err(ParentError::Cycle)
        );
        assert_eq!(ParentError::Cycle.code(), "cycle");
    }
}
