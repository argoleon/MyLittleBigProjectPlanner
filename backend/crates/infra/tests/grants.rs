//! Integration tests on a real PostgreSQL: grants of roles on hierarchy
//! nodes (L03-US03) — inheritance through the closure table, union with
//! several parents, groups, confidential nodes, the visibility filter of the
//! nodes list, the effective access of a node, and tenant isolation.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::{Health, NodeType};
use domain::members::GroupInput;
use domain::permissions::Permission;
use infra::accounts::TenantRole;
use infra::db::begin_tenant_tx;
use infra::grants::{self, Candidate, GrantConflict, Grantee, Holder, NewGrant, NodeGrants};
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields, NodeFilter};
use infra::permissions::GrantSource;
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

async fn node(
    conn: &mut PgConnection,
    node_type: NodeType,
    name: &str,
    key: Option<&str>,
    parent: Option<Uuid>,
    creator: Uuid,
) -> Uuid {
    let new = NewNode {
        id: new_id(),
        node_type,
        key,
        fields: NodeFields {
            name,
            description: "",
            objectives: "",
            start_date: None,
            end_date: None,
            health: Health::OnTrack,
            owners: &[],
        },
        parent,
        created_by: creator,
    };
    nodes::insert(conn, &new).await.unwrap().unwrap().id
}

/// The default role labelled `label` (English) of the current tenant.
async fn role(conn: &mut PgConnection, label: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM role WHERE label ->> 'en' = $1")
        .bind(label)
        .fetch_one(conn)
        .await
        .unwrap()
}

async fn group(conn: &mut PgConnection, name: &str, members: &[Uuid]) -> Uuid {
    let input = GroupInput {
        name: name.to_owned(),
        description: None,
    };
    let group = infra::groups::insert(conn, new_id(), &input)
        .await
        .unwrap()
        .unwrap();
    for member in members {
        infra::groups::add_member(conn, group.id, *member).await.unwrap();
    }
    group.id
}

async fn grant(
    conn: &mut PgConnection,
    node: Uuid,
    role: Uuid,
    grantee: Grantee,
    by: Uuid,
) -> Result<Uuid, GrantConflict> {
    let new = NewGrant {
        id: new_id(),
        node,
        role,
        grantee,
        created_by: by,
    };
    grants::insert(conn, &new).await.unwrap().map(|()| new.id)
}

async fn member(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    db.add_member(tenant, TenantRole::Member).await.account_id
}

/// The labels (English) of the roles that reach `account` on `node`, sorted.
async fn roles_on(conn: &mut PgConnection, account: Uuid, node: Uuid) -> Vec<String> {
    let ids = NodeGrants.roles_of(conn, account, Some(node)).await.unwrap();
    let mut labels: Vec<String> = sqlx::query_scalar("SELECT label ->> 'en' FROM role WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(conn)
        .await
        .unwrap();
    labels.sort();
    labels
}

/// Names of the nodes `account` sees in the list, sorted.
async fn visible(conn: &mut PgConnection, account: Uuid) -> Vec<String> {
    let filter = NodeFilter {
        visible_to: Some(account),
        ..NodeFilter::default()
    };
    let mut names: Vec<String> = nodes::list(conn, &filter, None, 200)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|n| n.name)
        .collect();
    names.sort();
    names
}

/// Digital ─┬─ Modernization ─┬─ Identity platform ── Permissions
///          └─ Compliance ────┘
struct Hierarchy {
    digital: Uuid,
    modernization: Uuid,
    compliance: Uuid,
    plat: Uuid,
    permissions: Uuid,
}

async fn hierarchy(conn: &mut PgConnection, creator: Uuid) -> Hierarchy {
    let digital = node(conn, NodeType::Portfolio, "Digital", None, None, creator).await;
    let modernization = node(
        conn,
        NodeType::Program,
        "Modernization",
        None,
        Some(digital),
        creator,
    )
    .await;
    let compliance = node(
        conn,
        NodeType::Program,
        "Compliance",
        None,
        Some(digital),
        creator,
    )
    .await;
    let plat = node(
        conn,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        Some(modernization),
        creator,
    )
    .await;
    nodes::link(conn, compliance, plat, 100).await.unwrap().unwrap();
    let permissions = node(
        conn,
        NodeType::Project,
        "Permissions",
        Some("PERM"),
        Some(plat),
        creator,
    )
    .await;
    Hierarchy {
        digital,
        modernization,
        compliance,
        plat,
        permissions,
    }
}

#[tokio::test]
async fn a_group_grant_on_a_portfolio_reaches_the_projects_below() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let (lucas, ines) = (member(&db, &db.tenant_a).await, member(&db, &db.tenant_a).await);
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut tx, admin).await;
    let managers = group(&mut tx, "Managers", &[lucas]).await;
    let manager = role(&mut tx, "Project manager").await;

    assert!(roles_on(&mut tx, lucas, h.plat).await.is_empty());
    assert!(visible(&mut tx, lucas).await.is_empty());
    let granted = grant(&mut tx, h.digital, manager, Grantee::Group(managers), admin)
        .await
        .unwrap();

    // The role reaches every descendant, for the members of the group only.
    for target in [h.digital, h.modernization, h.plat, h.permissions] {
        assert_eq!(roles_on(&mut tx, lucas, target).await, ["Project manager"]);
        assert!(roles_on(&mut tx, ines, target).await.is_empty());
    }
    let reaching = NodeGrants.roles_of(&mut tx, lucas, Some(h.plat)).await.unwrap();
    let permissions = infra::roles::permissions_of(&mut tx, &reaching).await.unwrap();
    assert!(
        permissions.contains(Permission::ManageSprints) && !permissions.contains(Permission::AdministerNode)
    );
    assert_eq!(
        visible(&mut tx, lucas).await,
        [
            "Compliance",
            "Digital",
            "Identity platform",
            "Modernization",
            "Permissions"
        ]
    );
    assert!(visible(&mut tx, ines).await.is_empty());
    // Roles are granted on nodes: nothing tenant wide.
    assert!(
        NodeGrants
            .roles_of(&mut tx, lucas, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(NodeGrants.grants_of_role(&mut tx, manager).await.unwrap(), 1);

    // Removing the grant removes what was inherited.
    assert!(grants::delete(&mut tx, h.digital, granted).await.unwrap());
    assert!(!grants::delete(&mut tx, h.digital, granted).await.unwrap());
    assert!(roles_on(&mut tx, lucas, h.plat).await.is_empty());
    assert!(visible(&mut tx, lucas).await.is_empty());
    assert_eq!(NodeGrants.grants_of_role(&mut tx, manager).await.unwrap(), 0);
}

#[tokio::test]
async fn two_parents_give_the_union_of_their_roles_and_the_direct_ones() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let lucas = member(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut tx, admin).await;
    let platform_team = group(&mut tx, "Platform team", &[lucas]).await;
    let compliance_team = group(&mut tx, "Compliance team", &[lucas]).await;
    let (reader, contributor, node_admin) = (
        role(&mut tx, "Reader").await,
        role(&mut tx, "Contributor").await,
        role(&mut tx, "Node admin").await,
    );
    grant(
        &mut tx,
        h.modernization,
        contributor,
        Grantee::Group(platform_team),
        admin,
    )
    .await
    .unwrap();
    grant(
        &mut tx,
        h.compliance,
        reader,
        Grantee::Group(compliance_team),
        admin,
    )
    .await
    .unwrap();
    grant(&mut tx, h.plat, node_admin, Grantee::Member(lucas), admin)
        .await
        .unwrap();

    assert_eq!(
        roles_on(&mut tx, lucas, h.plat).await,
        ["Contributor", "Node admin", "Reader"]
    );
    assert_eq!(roles_on(&mut tx, lucas, h.compliance).await, ["Reader"]);
    assert_eq!(roles_on(&mut tx, lucas, h.modernization).await, ["Contributor"]);
    // Nothing goes up: the portfolio stays out of reach.
    assert!(roles_on(&mut tx, lucas, h.digital).await.is_empty());
    assert_eq!(
        visible(&mut tx, lucas).await,
        ["Compliance", "Identity platform", "Modernization", "Permissions"]
    );

    // The same grant twice, and grantees or roles that do not exist: each
    // refusal aborts its statement, back to the savepoint.
    sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
    assert_eq!(
        grant(&mut tx, h.plat, node_admin, Grantee::Member(lucas), admin).await,
        Err(GrantConflict::Exists)
    );
    sqlx::query("ROLLBACK TO SAVEPOINT s")
        .execute(&mut *tx)
        .await
        .unwrap();
    for (node, role, grantee, conflict) in [
        (
            new_id(),
            reader,
            Grantee::Member(lucas),
            GrantConflict::NodeNotFound,
        ),
        (
            h.plat,
            new_id(),
            Grantee::Member(lucas),
            GrantConflict::RoleNotFound,
        ),
        (
            h.plat,
            reader,
            Grantee::Member(new_id()),
            GrantConflict::UnknownMember,
        ),
        (
            h.plat,
            reader,
            Grantee::Group(new_id()),
            GrantConflict::GroupNotFound,
        ),
    ] {
        assert_eq!(grant(&mut tx, node, role, grantee, admin).await, Err(conflict));
        sqlx::query("ROLLBACK TO SAVEPOINT s")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_confidential_node_only_hears_its_own_grants() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let (lucas, ines) = (member(&db, &db.tenant_a).await, member(&db, &db.tenant_a).await);
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut tx, admin).await;
    let (reader, contributor) = (role(&mut tx, "Reader").await, role(&mut tx, "Contributor").await);
    grant(&mut tx, h.digital, reader, Grantee::Member(lucas), admin)
        .await
        .unwrap();
    grant(&mut tx, h.plat, contributor, Grantee::Member(ines), admin)
        .await
        .unwrap();
    sqlx::query("UPDATE node SET confidential = true WHERE id = $1")
        .bind(h.plat)
        .execute(&mut *tx)
        .await
        .unwrap();

    // What comes from above stops at the confidential project...
    assert!(roles_on(&mut tx, lucas, h.plat).await.is_empty());
    assert!(roles_on(&mut tx, lucas, h.permissions).await.is_empty());
    assert_eq!(roles_on(&mut tx, lucas, h.modernization).await, ["Reader"]);
    assert_eq!(
        visible(&mut tx, lucas).await,
        ["Compliance", "Digital", "Modernization"]
    );
    // ... while its own grants apply on it and below.
    assert_eq!(roles_on(&mut tx, ines, h.plat).await, ["Contributor"]);
    assert_eq!(roles_on(&mut tx, ines, h.permissions).await, ["Contributor"]);
    assert_eq!(visible(&mut tx, ines).await, ["Identity platform", "Permissions"]);
    let access = grants::access(&mut tx, h.plat).await.unwrap();
    assert_eq!(access.len(), 1, "the inherited grant does not apply: {access:?}");

    // One confidential parent out of two: the other path still brings the role.
    sqlx::query("UPDATE node SET confidential = (id = $1) WHERE id IN ($1, $2)")
        .bind(h.modernization)
        .bind(h.plat)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(roles_on(&mut tx, lucas, h.plat).await, ["Reader"]);
    assert!(roles_on(&mut tx, lucas, h.modernization).await.is_empty());
    assert_eq!(
        visible(&mut tx, lucas).await,
        ["Compliance", "Digital", "Identity platform", "Permissions"]
    );
}

/// The nodes list filters in SQL what the permission engine computes node
/// by node: both must agree, whatever the hierarchy.
#[tokio::test]
async fn the_list_shows_exactly_the_nodes_the_engine_lets_see() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let lucas = member(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut tx, admin).await;
    let team = group(&mut tx, "Team", &[lucas]).await;
    let reader = role(&mut tx, "Reader").await;
    // A role without "view": it gives rights, not sight.
    let blind: Uuid = sqlx::query_scalar(
        "INSERT INTO role (id, tenant_id, label, permissions)
         VALUES ($1, app_current_tenant(), '{\"en\": \"Commenter\"}', ARRAY['comment']) RETURNING id",
    )
    .bind(new_id())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let all = [h.digital, h.modernization, h.compliance, h.plat, h.permissions];

    // Every way to place one grant and one confidential node.
    for granted_on in all {
        for (role, grantee) in [
            (reader, Grantee::Member(lucas)),
            (reader, Grantee::Group(team)),
            (blind, Grantee::Member(lucas)),
        ] {
            for confidential in all {
                sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
                grant(&mut tx, granted_on, role, grantee, admin).await.unwrap();
                sqlx::query("UPDATE node SET confidential = true WHERE id = $1")
                    .bind(confidential)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                let mut seen = Vec::new();
                for target in all {
                    let roles = NodeGrants.roles_of(&mut tx, lucas, Some(target)).await.unwrap();
                    let permissions = infra::roles::permissions_of(&mut tx, &roles).await.unwrap();
                    if permissions.contains(Permission::View) {
                        let name: String = sqlx::query_scalar("SELECT name FROM node WHERE id = $1")
                            .bind(target)
                            .fetch_one(&mut *tx)
                            .await
                            .unwrap();
                        seen.push(name);
                    }
                }
                seen.sort();
                assert_eq!(
                    visible(&mut tx, lucas).await,
                    seen,
                    "grant on {granted_on}, confidential {confidential}"
                );
                sqlx::query("ROLLBACK TO SAVEPOINT s")
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn the_access_of_a_node_tells_direct_grants_from_inherited_ones() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let lucas = member(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut tx, admin).await;
    let managers = group(&mut tx, "Managers", &[]).await;
    let (reader, contributor, manager) = (
        role(&mut tx, "Reader").await,
        role(&mut tx, "Contributor").await,
        role(&mut tx, "Project manager").await,
    );
    grant(&mut tx, h.digital, manager, Grantee::Group(managers), admin)
        .await
        .unwrap();
    grant(&mut tx, h.compliance, reader, Grantee::Member(lucas), admin)
        .await
        .unwrap();
    let direct = grant(&mut tx, h.plat, contributor, Grantee::Member(lucas), admin)
        .await
        .unwrap();
    // Below the project: not part of its access.
    grant(&mut tx, h.permissions, reader, Grantee::Group(managers), admin)
        .await
        .unwrap();

    let access = grants::access(&mut tx, h.plat).await.unwrap();
    let lines: Vec<(bool, &str, &str, &str)> = access
        .iter()
        .map(|a| {
            let holder = match &a.holder {
                Holder::Member { .. } => "member",
                Holder::Group { name, .. } => name.as_str(),
            };
            (
                a.direct,
                holder,
                a.role_label["en"].as_str(),
                a.origin.name.as_str(),
            )
        })
        .collect();
    // The direct grant first, then the inherited ones by holder name.
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0], (true, "member", "Contributor", "Identity platform"));
    assert!(lines.contains(&(false, "Managers", "Project manager", "Digital")));
    assert!(lines.contains(&(false, "member", "Reader", "Compliance")));
    assert_eq!(access[0].grant_id, direct);
    assert_eq!(access[0].origin.node_type, NodeType::Project);
    assert_eq!(
        access[0].holder,
        Holder::Member {
            account_id: lucas,
            display_name: sqlx::query_scalar("SELECT display_name FROM account WHERE id = $1")
                .bind(lucas)
                .fetch_one(&mut *tx)
                .await
                .unwrap()
        }
    );
    assert_eq!(
        grants::role_of(&mut tx, h.plat, direct).await.unwrap(),
        Some(contributor)
    );
    // A grant is revoked on the node it was made on.
    assert_eq!(grants::role_of(&mut tx, h.digital, direct).await.unwrap(), None);
    assert!(!grants::delete(&mut tx, h.digital, direct).await.unwrap());
}

#[tokio::test]
async fn grants_go_with_their_group_their_member_and_their_node_and_keep_their_role() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let (lucas, ines) = (member(&db, &db.tenant_a).await, member(&db, &db.tenant_a).await);
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut tx, admin).await;
    let team = group(&mut tx, "Team", &[lucas]).await;
    let reader = role(&mut tx, "Reader").await;
    grant(&mut tx, h.digital, reader, Grantee::Group(team), admin)
        .await
        .unwrap();
    grant(&mut tx, h.plat, reader, Grantee::Member(ines), admin)
        .await
        .unwrap();
    grant(&mut tx, h.permissions, reader, Grantee::Member(ines), admin)
        .await
        .unwrap();
    assert_eq!(NodeGrants.grants_of_role(&mut tx, reader).await.unwrap(), 3);

    // A role in use stays.
    sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
    let refused = sqlx::query("DELETE FROM role WHERE id = $1")
        .bind(reader)
        .execute(&mut *tx)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("node_role_grant_role_fkey"), "{refused}");
    sqlx::query("ROLLBACK TO SAVEPOINT s")
        .execute(&mut *tx)
        .await
        .unwrap();

    // A member leaving the group loses its role at once; the group gone,
    // its grant goes too.
    infra::groups::remove_member(&mut tx, team, lucas).await.unwrap();
    assert!(roles_on(&mut tx, lucas, h.plat).await.is_empty());
    sqlx::query(r#"DELETE FROM "group" WHERE id = $1"#)
        .bind(team)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(NodeGrants.grants_of_role(&mut tx, reader).await.unwrap(), 2);
    // A node gone, its grants go.
    sqlx::query("DELETE FROM node WHERE id = $1")
        .bind(h.permissions)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(NodeGrants.grants_of_role(&mut tx, reader).await.unwrap(), 1);
    // A member gone (removed or erased), their grants go.
    infra::members::remove(&mut tx, ines, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(NodeGrants.grants_of_role(&mut tx, reader).await.unwrap(), 0);
}

#[tokio::test]
async fn grants_never_leak_between_tenants() {
    let db = TestDb::new().await;
    let admin_a = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let admin_b = db
        .add_member(&db.tenant_b, TenantRole::TenantAdmin)
        .await
        .account_id;
    let lucas = member(&db, &db.tenant_a).await;
    let stranger = member(&db, &db.tenant_b).await;

    let mut a = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let h = hierarchy(&mut a, admin_a).await;
    let reader_a = role(&mut a, "Reader").await;
    let team_a = group(&mut a, "Team", &[lucas]).await;
    grant(&mut a, h.digital, reader_a, Grantee::Member(lucas), admin_a)
        .await
        .unwrap();
    grant(&mut a, h.digital, reader_a, Grantee::Group(team_a), admin_a)
        .await
        .unwrap();
    // A member of another tenant is no grantee here.
    assert_eq!(
        grant(&mut a, h.plat, reader_a, Grantee::Member(stranger), admin_a).await,
        Err(GrantConflict::UnknownMember)
    );
    a.commit().await.unwrap();

    let mut b = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let elsewhere = node(&mut b, NodeType::Portfolio, "Elsewhere", None, None, admin_b).await;
    let reader_b = role(&mut b, "Reader").await;
    // Tenant B sees nothing of A's grants, even for A's member and nodes.
    assert!(grants::access(&mut b, h.plat).await.unwrap().is_empty());
    assert!(roles_on(&mut b, lucas, h.plat).await.is_empty());
    assert!(visible(&mut b, lucas).await.is_empty());
    assert_eq!(NodeGrants.grants_of_role(&mut b, reader_a).await.unwrap(), 0);
    // Nor can it grant with A's role, group, member or node.
    sqlx::query("SAVEPOINT s").execute(&mut *b).await.unwrap();
    for (node, role, grantee, conflict) in [
        (
            elsewhere,
            reader_a,
            Grantee::Member(stranger),
            GrantConflict::RoleNotFound,
        ),
        (
            elsewhere,
            reader_b,
            Grantee::Group(team_a),
            GrantConflict::GroupNotFound,
        ),
        (
            elsewhere,
            reader_b,
            Grantee::Member(lucas),
            GrantConflict::UnknownMember,
        ),
        (
            h.plat,
            reader_b,
            Grantee::Member(stranger),
            GrantConflict::NodeNotFound,
        ),
    ] {
        assert_eq!(grant(&mut b, node, role, grantee, admin_b).await, Err(conflict));
        sqlx::query("ROLLBACK TO SAVEPOINT s")
            .execute(&mut *b)
            .await
            .unwrap();
    }
    // Candidates are the tenant's own.
    let found = grants::candidates(&mut b, None, 50).await.unwrap();
    assert!(found.iter().all(|c| match c {
        Candidate::Member { account_id, .. } => *account_id != lucas && *account_id != admin_a,
        Candidate::Group { id, .. } => *id != team_a,
    }));
    assert!(
        found
            .iter()
            .any(|c| matches!(c, Candidate::Member { account_id, .. } if *account_id == stranger))
    );
}

#[tokio::test]
async fn candidates_are_groups_then_active_members_by_name() {
    let db = TestDb::new().await;
    let lucas = member(&db, &db.tenant_a).await;
    let suspended = member(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    sqlx::query("UPDATE account SET display_name = 'Lucas Bernard' WHERE id = $1")
        .bind(lucas)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE account SET display_name = 'Luc Suspended' WHERE id = $1")
        .bind(suspended)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE membership SET status = 'suspended' WHERE account_id = $1")
        .bind(suspended)
        .execute(&mut *tx)
        .await
        .unwrap();
    let lucky = group(&mut tx, "Lucky team", &[]).await;
    group(&mut tx, "Managers", &[]).await;

    let found = grants::candidates(&mut tx, Some(" LUC"), 10).await.unwrap();
    assert_eq!(
        found,
        [
            Candidate::Group {
                id: lucky,
                name: "Lucky team".to_owned()
            },
            Candidate::Member {
                account_id: lucas,
                display_name: "Lucas Bernard".to_owned(),
                email: sqlx::query_scalar("SELECT email FROM account WHERE id = $1")
                    .bind(lucas)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap(),
            },
        ]
    );
    // One of each at most; wildcards are literal.
    assert_eq!(grants::candidates(&mut tx, None, 1).await.unwrap().len(), 2);
    assert!(
        grants::candidates(&mut tx, Some("%"), 10)
            .await
            .unwrap()
            .is_empty()
    );
}
