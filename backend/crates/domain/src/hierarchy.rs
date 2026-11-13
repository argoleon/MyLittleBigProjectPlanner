//! Portfolio, program and project hierarchy (specification §4, L03).
//!
//! Nodes form a directed acyclic graph: a node may have several parents,
//! within the attachment matrix ([`may_attach`]). Only projects hold tickets
//! and carry a key (`PLAT`), unique in their tenant.

use crate::validation::{FieldError, Violations};

/// Kind of node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeType {
    Portfolio,
    Program,
    Project,
}

impl NodeType {
    pub const ALL: [Self; 3] = [Self::Portfolio, Self::Program, Self::Project];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Portfolio => "portfolio",
            Self::Program => "program",
            Self::Project => "project",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == value)
    }
}

/// Whether a node of type `child` may be attached under one of type
/// `parent` (specification §4): a portfolio under a portfolio; a program
/// under a portfolio or a program; a project under any node.
pub const fn may_attach(parent: NodeType, child: NodeType) -> bool {
    matches!(
        (child, parent),
        (NodeType::Portfolio, NodeType::Portfolio)
            | (NodeType::Program, NodeType::Portfolio | NodeType::Program)
            | (NodeType::Project, _)
    )
}

/// How a node stands, as its owners see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Health {
    #[default]
    OnTrack,
    AtRisk,
    OffTrack,
}

impl Health {
    pub const ALL: [Self; 3] = [Self::OnTrack, Self::AtRisk, Self::OffTrack];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnTrack => "on_track",
            Self::AtRisk => "at_risk",
            Self::OffTrack => "off_track",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.as_str() == value)
    }
}

/// Longest node name, in characters.
pub const NAME_MAX_CHARS: usize = 200;
/// Longest description or objectives, in characters.
pub const TEXT_MAX_CHARS: usize = 10_000;
/// Most owners of a node.
pub const MAX_OWNERS: usize = 20;

/// Whether `key` is a project key: 2 to 10 capital letters and digits, a
/// letter first (`PLAT`, `ERP2`).
pub fn is_project_key(key: &str) -> bool {
    let mut chars = key.chars();
    let first_is_letter = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    (2..=10).contains(&key.len())
        && first_is_letter
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// What is set on a node. Dates are local `YYYY-MM-DD` dates, compared as
/// such (the API parses them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInput {
    pub name: String,
    /// Projects only.
    pub key: Option<String>,
    pub description: String,
    pub objectives: String,
    pub start_date: Option<(i32, u32, u32)>,
    pub end_date: Option<(i32, u32, u32)>,
    pub owner_count: usize,
}

/// Checks `input` for a node of type `node_type`; field names are those of
/// the JSON API.
pub fn validate_node(node_type: NodeType, input: &NodeInput) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let name = input.name.trim().chars().count();
    if name == 0 {
        v.add("name", "required");
    } else if name > NAME_MAX_CHARS {
        v.add("name", "too_long");
    }
    match (node_type, input.key.as_deref()) {
        (NodeType::Project, None) => v.add("key", "required"),
        (NodeType::Project, Some(key)) if !is_project_key(key) => v.add("key", "format"),
        (NodeType::Portfolio | NodeType::Program, Some(_)) => v.add("key", "not_allowed"),
        _ => {}
    }
    if input.description.chars().count() > TEXT_MAX_CHARS {
        v.add("description", "too_long");
    }
    if input.objectives.chars().count() > TEXT_MAX_CHARS {
        v.add("objectives", "too_long");
    }
    if let (Some(start), Some(end)) = (input.start_date, input.end_date)
        && end < start
    {
        v.add("endDate", "before_start");
    }
    if input.owner_count > MAX_OWNERS {
        v.add("ownerIds", "too_many");
    }
    v.into_result()
}

/// What allows a member to change the hierarchy, as far as nodes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights {
    /// Tenant admins administer every node.
    pub tenant_admin: bool,
    /// The member has "administer node" on the node concerned (directly or
    /// inherited): the parent for a creation, the node itself for a change.
    pub administers_node: bool,
}

/// Whether a member with `rights` creates a node: at the root (`under_parent`
/// false) only tenant admins do; under a parent, also its administrators.
pub const fn may_create(rights: Rights, under_parent: bool) -> bool {
    rights.tenant_admin || (under_parent && rights.administers_node)
}

/// Whether a member with `rights` (on the node itself) changes it.
pub const fn may_change(rights: Rights) -> bool {
    rights.tenant_admin || rights.administers_node
}

/// Whether a node of this type can be made confidential (specification §4):
/// projects only. Portfolios and programs stay open to those above them.
pub const fn may_be_confidential(node_type: NodeType) -> bool {
    matches!(node_type, NodeType::Project)
}

/// Whether a node of this type can be archived (specification §4): projects
/// only.
pub const fn may_be_archived(node_type: NodeType) -> bool {
    matches!(node_type, NodeType::Project)
}

/// Whether a node accepts changes: an archived project is read-only. Its
/// fields, its confidentiality, its links, its children and its tickets
/// stay as they are until it is restored.
pub const fn accepts_changes(archived: bool) -> bool {
    !archived
}

/// Why a project cannot be archived or restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivingRefusal {
    /// It still has active sub-projects: they would stay in the active
    /// lists without their parent.
    ActiveChildren,
    /// It is under an archived project: that one is restored first.
    ArchivedParent,
}

/// What a project to archive or restore stands between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surroundings {
    /// One of its children is not archived.
    pub has_active_children: bool,
    /// One of its parents is archived.
    pub has_archived_parent: bool,
}

/// Why a project is not archived (`archive` true) or restored (false), if
/// it is not: an active project is never under an archived one.
pub const fn archiving_refusal(archive: bool, around: Surroundings) -> Option<ArchivingRefusal> {
    if archive && around.has_active_children {
        Some(ArchivingRefusal::ActiveChildren)
    } else if !archive && around.has_archived_parent {
        Some(ArchivingRefusal::ArchivedParent)
    } else {
        None
    }
}

/// Whether a member links a node and a parent, changes that link's
/// weighting or removes it: tenant admins, and the members who administer
/// both nodes (a parent's aggregates and inherited grants change with it).
pub const fn may_link(child: Rights, parent: Rights) -> bool {
    child.tenant_admin || (child.administers_node && parent.administers_node)
}

/// Default weighting of a link, in percent.
pub const DEFAULT_WEIGHT_PERCENT: i16 = 100;

/// Whether `percent` is a link weighting: 0 to 100.
pub const fn is_weight_percent(percent: i16) -> bool {
    0 <= percent && percent <= 100
}

/// Why a node cannot become another one's parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentRefusal {
    /// It already is.
    AlreadyParent,
    /// The attachment matrix does not allow the child's type under it.
    NotAllowed,
    /// It is the child or one of its descendants: the link would make a
    /// cycle.
    Cycle,
    /// The member may not link these two nodes.
    Forbidden,
}

impl ParentRefusal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyParent => "already_parent",
            Self::NotAllowed => "not_allowed",
            Self::Cycle => "cycle",
            Self::Forbidden => "forbidden",
        }
    }
}

/// A node considered as a new parent of `child_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentCandidate {
    pub candidate_type: NodeType,
    pub child_type: NodeType,
    /// The candidate is the child or one of its descendants.
    pub descends_from_child: bool,
    /// The candidate already is a parent of the child.
    pub already_parent: bool,
    /// The member may link the two ([`may_link`]).
    pub may_link: bool,
}

/// Why `candidate` cannot become the parent, if it cannot: the first of
/// already a parent, the matrix, a cycle, the member's rights.
pub const fn parent_refusal(candidate: ParentCandidate) -> Option<ParentRefusal> {
    if candidate.already_parent {
        Some(ParentRefusal::AlreadyParent)
    } else if !may_attach(candidate.candidate_type, candidate.child_type) {
        Some(ParentRefusal::NotAllowed)
    } else if candidate.descends_from_child {
        Some(ParentRefusal::Cycle)
    } else if !candidate.may_link {
        Some(ParentRefusal::Forbidden)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> NodeInput {
        NodeInput {
            name: "Identity platform".to_owned(),
            key: Some("PLAT".to_owned()),
            description: String::new(),
            objectives: String::new(),
            start_date: Some((2026, 1, 5)),
            end_date: Some((2026, 12, 18)),
            owner_count: 2,
        }
    }

    fn codes(node_type: NodeType, input: &NodeInput) -> Vec<(String, &'static str)> {
        validate_node(node_type, input)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn only_projects_are_archived_and_archived_ones_are_read_only() {
        assert!(may_be_archived(NodeType::Project));
        assert!(!may_be_archived(NodeType::Program) && !may_be_archived(NodeType::Portfolio));
        assert!(accepts_changes(false));
        assert!(!accepts_changes(true));
    }

    #[test]
    fn an_active_project_is_never_left_under_an_archived_one() {
        let around = |has_active_children, has_archived_parent| Surroundings {
            has_active_children,
            has_archived_parent,
        };
        // Archiving: its sub-projects first.
        assert_eq!(archiving_refusal(true, around(false, false)), None);
        assert_eq!(
            archiving_refusal(true, around(true, false)),
            Some(ArchivingRefusal::ActiveChildren)
        );
        // An archived sub-project is archived whatever its parent.
        assert_eq!(archiving_refusal(true, around(false, true)), None);
        // Restoring: its parent project first.
        assert_eq!(archiving_refusal(false, around(false, false)), None);
        assert_eq!(
            archiving_refusal(false, around(false, true)),
            Some(ArchivingRefusal::ArchivedParent)
        );
        assert_eq!(archiving_refusal(false, around(true, false)), None);
    }

    #[test]
    fn the_matrix_follows_the_specification() {
        use NodeType::{Portfolio, Program, Project};
        let allowed = [
            (Portfolio, Portfolio, true),
            (Program, Portfolio, false),
            (Project, Portfolio, false),
            (Portfolio, Program, true),
            (Program, Program, true),
            (Project, Program, false),
            (Portfolio, Project, true),
            (Program, Project, true),
            (Project, Project, true),
        ];
        for (parent, child, expected) in allowed {
            assert_eq!(may_attach(parent, child), expected, "{child:?} under {parent:?}");
        }
    }

    #[test]
    fn identifiers_parse_back() {
        for t in NodeType::ALL {
            assert_eq!(NodeType::parse(t.as_str()), Some(t));
        }
        for h in Health::ALL {
            assert_eq!(Health::parse(h.as_str()), Some(h));
        }
        assert_eq!(NodeType::parse("team"), None);
        assert_eq!(Health::default(), Health::OnTrack);
    }

    #[test]
    fn project_keys_are_short_capitals_and_digits() {
        for good in ["PLAT", "ERP2", "AB", "ABCDEFGHIJ"] {
            assert!(is_project_key(good), "{good}");
        }
        for bad in ["P", "plat", "2PLAT", "PL-AT", "ABCDEFGHIJK", "", "PLÂT"] {
            assert!(!is_project_key(bad), "{bad}");
        }
    }

    #[test]
    fn a_project_needs_a_key_and_the_others_have_none() {
        assert!(codes(NodeType::Project, &input()).is_empty());
        let keyless = NodeInput { key: None, ..input() };
        assert_eq!(
            codes(NodeType::Project, &keyless),
            [("key".to_owned(), "required")]
        );
        assert_eq!(
            codes(
                NodeType::Project,
                &NodeInput {
                    key: Some("plat".to_owned()),
                    ..input()
                }
            ),
            [("key".to_owned(), "format")]
        );
        assert!(codes(NodeType::Portfolio, &keyless).is_empty());
        assert_eq!(
            codes(NodeType::Program, &input()),
            [("key".to_owned(), "not_allowed")]
        );
    }

    #[test]
    fn names_texts_dates_and_owners_are_bounded() {
        let wrong = NodeInput {
            name: "  ".to_owned(),
            description: "x".repeat(TEXT_MAX_CHARS + 1),
            objectives: "x".repeat(TEXT_MAX_CHARS + 1),
            start_date: Some((2026, 12, 18)),
            end_date: Some((2026, 1, 5)),
            owner_count: MAX_OWNERS + 1,
            ..input()
        };
        assert_eq!(
            codes(NodeType::Project, &wrong),
            [
                ("name".to_owned(), "required"),
                ("description".to_owned(), "too_long"),
                ("objectives".to_owned(), "too_long"),
                ("endDate".to_owned(), "before_start"),
                ("ownerIds".to_owned(), "too_many"),
            ]
        );
        let long_name = NodeInput {
            name: "x".repeat(NAME_MAX_CHARS + 1),
            ..input()
        };
        assert_eq!(
            codes(NodeType::Project, &long_name),
            [("name".to_owned(), "too_long")]
        );
        let same_day = NodeInput {
            end_date: Some((2026, 1, 5)),
            ..input()
        };
        assert!(codes(NodeType::Project, &same_day).is_empty());
    }

    #[test]
    fn tenant_admins_create_anywhere_node_admins_under_their_nodes() {
        let tenant_admin = Rights {
            tenant_admin: true,
            administers_node: false,
        };
        let node_admin = Rights {
            tenant_admin: false,
            administers_node: true,
        };
        let member = Rights {
            tenant_admin: false,
            administers_node: false,
        };
        assert!(may_create(tenant_admin, false) && may_create(tenant_admin, true));
        assert!(!may_create(node_admin, false), "the root is the tenant admins'");
        assert!(may_create(node_admin, true));
        assert!(!may_create(member, true) && !may_create(member, false));
        assert!(may_change(tenant_admin) && may_change(node_admin) && !may_change(member));
    }

    #[test]
    fn only_projects_are_confidential() {
        assert!(may_be_confidential(NodeType::Project));
        assert!(!may_be_confidential(NodeType::Program) && !may_be_confidential(NodeType::Portfolio));
    }

    #[test]
    fn linking_needs_both_nodes_administered() {
        let tenant_admin = Rights {
            tenant_admin: true,
            administers_node: false,
        };
        let admin = Rights {
            tenant_admin: false,
            administers_node: true,
        };
        let member = Rights {
            tenant_admin: false,
            administers_node: false,
        };
        assert!(may_link(tenant_admin, tenant_admin));
        assert!(may_link(admin, admin));
        assert!(!may_link(admin, member), "not the parent's administrator");
        assert!(!may_link(member, admin), "not the child's administrator");
    }

    #[test]
    fn weightings_are_percentages() {
        assert!(is_weight_percent(0) && is_weight_percent(40) && is_weight_percent(100));
        assert!(!is_weight_percent(-1) && !is_weight_percent(101));
        assert!(is_weight_percent(DEFAULT_WEIGHT_PERCENT));
    }

    #[test]
    fn refusals_come_in_order() {
        use NodeType::{Program, Project};
        let allowed = ParentCandidate {
            candidate_type: Program,
            child_type: Project,
            descends_from_child: false,
            already_parent: false,
            may_link: true,
        };
        assert_eq!(parent_refusal(allowed), None);
        let cases = [
            (
                ParentCandidate {
                    already_parent: true,
                    may_link: false,
                    ..allowed
                },
                ParentRefusal::AlreadyParent,
            ),
            // A program under a project: the matrix speaks first.
            (
                ParentCandidate {
                    candidate_type: Project,
                    child_type: Program,
                    descends_from_child: true,
                    ..allowed
                },
                ParentRefusal::NotAllowed,
            ),
            // A sub-project of the project: a cycle.
            (
                ParentCandidate {
                    candidate_type: Project,
                    descends_from_child: true,
                    may_link: false,
                    ..allowed
                },
                ParentRefusal::Cycle,
            ),
            (
                ParentCandidate {
                    may_link: false,
                    ..allowed
                },
                ParentRefusal::Forbidden,
            ),
        ];
        for (candidate, refusal) in cases {
            assert_eq!(parent_refusal(candidate), Some(refusal), "{candidate:?}");
        }
        assert_eq!(
            [
                ParentRefusal::AlreadyParent,
                ParentRefusal::NotAllowed,
                ParentRefusal::Cycle,
                ParentRefusal::Forbidden
            ]
            .map(ParentRefusal::as_str),
            ["already_parent", "not_allowed", "cycle", "forbidden"]
        );
    }
}
