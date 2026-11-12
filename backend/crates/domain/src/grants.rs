//! Grants of roles on hierarchy nodes and their inheritance (specification
//! §4 and §14, L03-US03).
//!
//! A role is granted to a member or a group on a node. It applies to that
//! node and to its descendants, but never *into* a node that blocks
//! inheritance (a confidential project): only the grants made on that node
//! itself reach it. With several parents, access is the union of what every
//! path brings. Permissions are additive: no grant ever removes one.
//!
//! Everything here is a pure function over the node's ancestry and the
//! grants made on it; the infrastructure only loads them.

use std::collections::{HashMap, HashSet};

use crate::permissions::{Permission, PermissionSet};

/// Identifier of a node, role, member or group (a UUID as a number).
pub type Id = u128;

/// A node of an ancestry: the node whose access is computed, or one of its
/// ancestors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestryNode {
    pub id: Id,
    /// Blocks inheritance: grants made above do not reach it, nor pass
    /// through it.
    pub confidential: bool,
    pub parents: Vec<Id>,
}

/// The nodes whose grants reach `node`: the node itself, then every ancestor
/// from which a path leads down to it without entering a confidential node.
/// The ancestor at the top of the path may be confidential itself: its own
/// grants apply below it.
///
/// `ancestry` holds the node and its ancestors; nodes missing from it have
/// no known parent.
pub fn granting_nodes(node: Id, ancestry: &[AncestryNode]) -> Vec<Id> {
    let by_id: HashMap<Id, &AncestryNode> = ancestry.iter().map(|n| (n.id, n)).collect();
    let mut reached = vec![node];
    let mut seen = HashSet::from([node]);
    let mut next = 0;
    while let Some(current) = reached.get(next).copied() {
        next += 1;
        // Grants come down into `current` from its parents, unless it blocks
        // them.
        let Some(found) = by_id.get(&current).filter(|n| !n.confidential) else {
            continue;
        };
        for parent in &found.parents {
            if seen.insert(*parent) {
                reached.push(*parent);
            }
        }
    }
    reached
}

/// Who receives a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Principal {
    Member(Id),
    Group(Id),
}

/// A role granted on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Grant {
    pub node: Id,
    pub role: Id,
    pub principal: Principal,
}

/// A member with the groups they belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject<'a> {
    pub member: Id,
    pub groups: &'a [Id],
}

impl Subject<'_> {
    /// Whether a grant to `principal` is a grant to this member.
    pub fn receives(&self, principal: Principal) -> bool {
        match principal {
            Principal::Member(id) => id == self.member,
            Principal::Group(id) => self.groups.contains(&id),
        }
    }
}

/// The roles that reach `subject` on `node`, each once: granted to them or
/// to one of their groups, on the node or on a node whose grants reach it
/// ([`granting_nodes`]).
pub fn roles_reaching(
    node: Id,
    ancestry: &[AncestryNode],
    grants: &[Grant],
    subject: Subject<'_>,
) -> Vec<Id> {
    let sources: HashSet<Id> = granting_nodes(node, ancestry).into_iter().collect();
    let mut roles = Vec::new();
    for grant in grants {
        if sources.contains(&grant.node) && subject.receives(grant.principal) && !roles.contains(&grant.role)
        {
            roles.push(grant.role);
        }
    }
    roles
}

/// The effective permissions of `subject` on `node`: the union of the
/// permissions of the roles that reach them there. `roles` gives each
/// role's permissions; an unknown role counts for nothing.
pub fn effective_permissions(
    node: Id,
    ancestry: &[AncestryNode],
    grants: &[Grant],
    subject: Subject<'_>,
    roles: &[(Id, PermissionSet)],
) -> PermissionSet {
    let reaching = roles_reaching(node, ancestry, grants, subject);
    crate::permissions::effective(
        roles
            .iter()
            .filter(|(id, _)| reaching.contains(id))
            .map(|(_, permissions)| *permissions),
    )
}

/// Whether a member sees who has access to a node: tenant admins, and the
/// members with "manage members" there.
pub const fn may_see_access(tenant_admin: bool, own: PermissionSet) -> bool {
    tenant_admin || own.contains(Permission::ManageMembers)
}

/// Whether a member grants or revokes a role of permissions `role` on a
/// node where they have `own`: tenant admins always; the others need
/// "manage members" there, and only handle roles within their own
/// permissions, so that nobody hands out more than they have.
pub const fn may_grant(tenant_admin: bool, own: PermissionSet, role: PermissionSet) -> bool {
    tenant_admin || (own.contains(Permission::ManageMembers) && own.contains_all(role))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUCAS: Id = 1;
    const MANAGERS: Id = 10;
    const PLATFORM_TEAM: Id = 11;
    const COMPLIANCE_TEAM: Id = 12;

    const READER: Id = 100;
    const CONTRIBUTOR: Id = 101;
    const PROJECT_MANAGER: Id = 102;

    // Digital ─┬─ Modernization ─┬─ PLAT ── Permissions
    //          └─ Compliance ────┘
    const DIGITAL: Id = 1000;
    const MODERNIZATION: Id = 1001;
    const COMPLIANCE: Id = 1002;
    const PLAT: Id = 1003;
    const PERMISSIONS: Id = 1004;

    fn node(id: Id, parents: &[Id]) -> AncestryNode {
        AncestryNode {
            id,
            confidential: false,
            parents: parents.to_vec(),
        }
    }

    fn hierarchy() -> Vec<AncestryNode> {
        vec![
            node(DIGITAL, &[]),
            node(MODERNIZATION, &[DIGITAL]),
            node(COMPLIANCE, &[DIGITAL]),
            node(PLAT, &[MODERNIZATION, COMPLIANCE]),
            node(PERMISSIONS, &[PLAT]),
        ]
    }

    fn confidential(mut nodes: Vec<AncestryNode>, id: Id) -> Vec<AncestryNode> {
        for n in &mut nodes {
            if n.id == id {
                n.confidential = true;
            }
        }
        nodes
    }

    fn roles() -> Vec<(Id, PermissionSet)> {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        vec![
            (READER, reader),
            (
                CONTRIBUTOR,
                reader.with(Permission::Edit).with(Permission::Comment),
            ),
            (
                PROJECT_MANAGER,
                reader.with(Permission::Edit).with(Permission::ManageSprints),
            ),
        ]
    }

    fn grant(node: Id, role: Id, principal: Principal) -> Grant {
        Grant {
            node,
            role,
            principal,
        }
    }

    fn lucas(groups: &[Id]) -> Subject<'_> {
        Subject {
            member: LUCAS,
            groups,
        }
    }

    fn sorted(mut ids: Vec<Id>) -> Vec<Id> {
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_grant_on_a_single_parent_chain_reaches_every_descendant() {
        let chain = vec![
            node(DIGITAL, &[]),
            node(MODERNIZATION, &[DIGITAL]),
            node(PLAT, &[MODERNIZATION]),
        ];
        assert_eq!(granting_nodes(PLAT, &chain), [PLAT, MODERNIZATION, DIGITAL]);
        assert_eq!(granting_nodes(DIGITAL, &chain), [DIGITAL]);
        // Group "Managers" is project manager on the portfolio.
        let grants = [grant(DIGITAL, PROJECT_MANAGER, Principal::Group(MANAGERS))];
        let manager = lucas(&[MANAGERS]);
        for target in [DIGITAL, MODERNIZATION, PLAT] {
            assert_eq!(
                roles_reaching(target, &chain, &grants, manager),
                [PROJECT_MANAGER]
            );
        }
        assert!(
            effective_permissions(PLAT, &chain, &grants, manager, &roles())
                .contains(Permission::ManageSprints)
        );
        // Nothing for someone outside the group, nor above the grant.
        assert!(roles_reaching(PLAT, &chain, &grants, lucas(&[])).is_empty());
        let lower = [grant(PLAT, READER, Principal::Member(LUCAS))];
        assert!(roles_reaching(MODERNIZATION, &chain, &lower, lucas(&[])).is_empty());
    }

    #[test]
    fn a_diamond_gives_the_union_of_its_parents_and_direct_roles() {
        let nodes = hierarchy();
        // The portfolio is reached by two paths, and counts once.
        assert_eq!(
            sorted(granting_nodes(PERMISSIONS, &nodes)),
            [DIGITAL, MODERNIZATION, COMPLIANCE, PLAT, PERMISSIONS]
        );
        let grants = [
            grant(MODERNIZATION, CONTRIBUTOR, Principal::Group(PLATFORM_TEAM)),
            grant(COMPLIANCE, READER, Principal::Group(COMPLIANCE_TEAM)),
            grant(PLAT, PROJECT_MANAGER, Principal::Member(LUCAS)),
            // To someone else: not Lucas's.
            grant(PLAT, PROJECT_MANAGER, Principal::Member(2)),
        ];
        let both = lucas(&[PLATFORM_TEAM, COMPLIANCE_TEAM]);
        assert_eq!(
            sorted(roles_reaching(PLAT, &nodes, &grants, both)),
            [READER, CONTRIBUTOR, PROJECT_MANAGER]
        );
        let all = effective_permissions(PLAT, &nodes, &grants, both, &roles());
        for permission in [
            Permission::View,
            Permission::Edit,
            Permission::Comment,
            Permission::ManageSprints,
        ] {
            assert!(all.contains(permission), "{permission:?}");
        }
        assert!(!all.contains(Permission::AdministerNode));
        // One group only: its role, plus the direct one.
        let one = lucas(&[COMPLIANCE_TEAM]);
        assert_eq!(
            sorted(roles_reaching(PLAT, &nodes, &grants, one)),
            [READER, PROJECT_MANAGER]
        );
        // On the other program, only what was granted there.
        assert_eq!(roles_reaching(COMPLIANCE, &nodes, &grants, both), [READER]);
    }

    #[test]
    fn the_same_role_from_several_grants_counts_once() {
        let nodes = hierarchy();
        let grants = [
            grant(DIGITAL, READER, Principal::Group(MANAGERS)),
            grant(PLAT, READER, Principal::Member(LUCAS)),
            grant(MODERNIZATION, READER, Principal::Member(LUCAS)),
        ];
        assert_eq!(
            roles_reaching(PLAT, &nodes, &grants, lucas(&[MANAGERS])),
            [READER]
        );
    }

    #[test]
    fn a_confidential_node_only_hears_its_own_grants() {
        let nodes = confidential(hierarchy(), PLAT);
        assert_eq!(granting_nodes(PLAT, &nodes), [PLAT]);
        let grants = [
            grant(DIGITAL, PROJECT_MANAGER, Principal::Group(MANAGERS)),
            grant(PLAT, READER, Principal::Member(LUCAS)),
        ];
        let manager = lucas(&[MANAGERS]);
        assert_eq!(roles_reaching(PLAT, &nodes, &grants, manager), [READER]);
        // Below it: its grants pass, those from above stay blocked.
        assert_eq!(granting_nodes(PERMISSIONS, &nodes), [PERMISSIONS, PLAT]);
        assert_eq!(roles_reaching(PERMISSIONS, &nodes, &grants, manager), [READER]);
        // Its parents are untouched.
        assert_eq!(
            roles_reaching(MODERNIZATION, &nodes, &grants, manager),
            [PROJECT_MANAGER]
        );
    }

    #[test]
    fn a_confidential_parent_blocks_its_path_only() {
        // Modernization is confidential: the portfolio still reaches PLAT
        // through Compliance, and Modernization's own grants still apply.
        let nodes = confidential(hierarchy(), MODERNIZATION);
        assert_eq!(
            sorted(granting_nodes(PLAT, &nodes)),
            [DIGITAL, MODERNIZATION, COMPLIANCE, PLAT]
        );
        // Both parents confidential: the portfolio no longer reaches PLAT.
        let nodes = confidential(nodes, COMPLIANCE);
        assert_eq!(
            sorted(granting_nodes(PLAT, &nodes)),
            [MODERNIZATION, COMPLIANCE, PLAT]
        );
        let grants = [
            grant(DIGITAL, READER, Principal::Member(LUCAS)),
            grant(COMPLIANCE, CONTRIBUTOR, Principal::Member(LUCAS)),
        ];
        assert_eq!(roles_reaching(PLAT, &nodes, &grants, lucas(&[])), [CONTRIBUTOR]);
        assert!(roles_reaching(COMPLIANCE, &nodes, &grants[..1], lucas(&[])).is_empty());
    }

    #[test]
    fn unknown_nodes_and_roles_count_for_nothing() {
        // A node missing from the ancestry has no parent to inherit from.
        assert_eq!(granting_nodes(42, &hierarchy()), [42]);
        let grants = [grant(PLAT, 999, Principal::Member(LUCAS))];
        assert_eq!(
            effective_permissions(PLAT, &hierarchy(), &grants, lucas(&[]), &roles()),
            PermissionSet::EMPTY
        );
    }

    #[test]
    fn managing_members_never_hands_out_more_than_one_has() {
        let reader: PermissionSet = [Permission::View].into_iter().collect();
        let manager = reader.with(Permission::Edit).with(Permission::ManageMembers);
        let contributor = reader.with(Permission::Edit);
        let node_admin = PermissionSet::all();
        // Tenant admins grant anything, whatever their own grants.
        assert!(may_grant(true, PermissionSet::EMPTY, node_admin));
        assert!(may_see_access(true, PermissionSet::EMPTY));
        // "Manage members": roles within one's own permissions only.
        assert!(may_grant(false, manager, contributor));
        assert!(may_grant(false, manager, manager));
        assert!(!may_grant(false, manager, node_admin), "more than they have");
        assert!(may_grant(false, node_admin, node_admin));
        assert!(may_see_access(false, manager));
        // Without it: nothing, even a narrower role.
        assert!(!may_grant(false, contributor, reader));
        assert!(!may_see_access(false, contributor));
    }

    /// A small deterministic generator (xorshift): the properties below run
    /// on the same random hierarchies every time.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// A random acyclic hierarchy of `size` nodes (a node's parents have
    /// smaller ids), some confidential, with random grants to three members
    /// and three groups.
    fn random_case(random: &mut Random, size: u64) -> (Vec<AncestryNode>, Vec<Grant>) {
        let mut nodes = Vec::new();
        for id in 0..size {
            let mut parents = Vec::new();
            for candidate in 0..id {
                if random.below(3) == 0 {
                    parents.push(Id::from(candidate));
                }
            }
            nodes.push(AncestryNode {
                id: Id::from(id),
                confidential: random.below(5) == 0,
                parents,
            });
        }
        let mut grants = Vec::new();
        for _ in 0..random.below(size * 2) {
            grants.push(random_grant(random, size));
        }
        (nodes, grants)
    }

    fn random_grant(random: &mut Random, size: u64) -> Grant {
        let principal = if random.below(2) == 0 {
            Principal::Member(Id::from(random.below(3)))
        } else {
            Principal::Group(Id::from(10 + random.below(3)))
        };
        Grant {
            node: Id::from(random.below(size)),
            role: [READER, CONTRIBUTOR, PROJECT_MANAGER][usize::try_from(random.below(3)).unwrap_or(0)],
            principal,
        }
    }

    /// The paths-based definition, written without [`granting_nodes`]: a
    /// grant on `source` reaches `node` if they are the same, or if `node`
    /// is not confidential and a grant on `source` reaches one of its
    /// parents.
    fn reaches(source: Id, node: Id, nodes: &[AncestryNode]) -> bool {
        if source == node {
            return true;
        }
        let Some(found) = nodes.iter().find(|n| n.id == node) else {
            return false;
        };
        !found.confidential && found.parents.iter().any(|parent| reaches(source, *parent, nodes))
    }

    #[test]
    fn property_granting_nodes_are_those_a_free_path_leads_from() {
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        for _ in 0..300 {
            let size = 2 + random.below(7);
            let (nodes, _) = random_case(&mut random, size);
            for target in 0..size {
                let target = Id::from(target);
                let expected: Vec<Id> = (0..size)
                    .map(Id::from)
                    .filter(|source| reaches(*source, target, &nodes))
                    .collect();
                assert_eq!(
                    sorted(granting_nodes(target, &nodes)),
                    expected,
                    "{target} in {nodes:?}"
                );
            }
        }
    }

    #[test]
    fn property_adding_a_grant_never_removes_a_permission() {
        let mut random = Random(0x2545_F491_4F6C_DD1D);
        let roles = roles();
        let groups = [10, 11];
        let subject = Subject {
            member: 0,
            groups: &groups,
        };
        for _ in 0..300 {
            let size = 2 + random.below(7);
            let (nodes, mut grants) = random_case(&mut random, size);
            let before: Vec<PermissionSet> = (0..size)
                .map(|n| effective_permissions(Id::from(n), &nodes, &grants, subject, &roles))
                .collect();
            grants.push(random_grant(&mut random, size));
            for (n, had) in (0..size).zip(before) {
                let has = effective_permissions(Id::from(n), &nodes, &grants, subject, &roles);
                assert!(
                    has.contains_all(had),
                    "node {n}: {had:?} then {has:?} in {nodes:?} {grants:?}"
                );
            }
        }
    }

    #[test]
    fn property_lifting_confidentiality_never_removes_a_permission() {
        let mut random = Random(0xD6E8_FEB8_6659_FD93);
        let roles = roles();
        let subject = Subject {
            member: 0,
            groups: &[10],
        };
        for _ in 0..300 {
            let size = 2 + random.below(7);
            let (nodes, grants) = random_case(&mut random, size);
            let open: Vec<AncestryNode> = nodes
                .iter()
                .map(|n| AncestryNode {
                    confidential: false,
                    ..n.clone()
                })
                .collect();
            for n in 0..size {
                let blocked = effective_permissions(Id::from(n), &nodes, &grants, subject, &roles);
                let free = effective_permissions(Id::from(n), &open, &grants, subject, &roles);
                assert!(free.contains_all(blocked), "node {n} in {nodes:?} {grants:?}");
            }
        }
    }
}
