//! Integration tests on a real PostgreSQL: the hierarchy's nodes, their
//! owners, the attachment matrix, cycles and the closure table (L03-US01),
//! several weighted parents and the candidate parents (L03-US02).

#![allow(clippy::unwrap_used)]

use chrono::NaiveDate;
use domain::hierarchy::{Health, NodeType};
use infra::accounts::TenantRole;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, LinkChange, NewNode, Node, NodeFields, NodeFilter, WriteConflict};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

fn fields<'a>(name: &'a str, owners: &'a [Uuid]) -> NodeFields<'a> {
    NodeFields {
        name,
        description: "",
        objectives: "",
        start_date: NaiveDate::from_ymd_opt(2026, 1, 5),
        end_date: NaiveDate::from_ymd_opt(2026, 12, 18),
        health: Health::OnTrack,
        owners,
    }
}

async fn create(
    conn: &mut PgConnection,
    node_type: NodeType,
    name: &str,
    key: Option<&str>,
    parent: Option<Uuid>,
    creator: Uuid,
) -> Result<Node, WriteConflict> {
    nodes::insert(
        conn,
        &NewNode {
            id: new_id(),
            node_type,
            key,
            fields: fields(name, &[]),
            parent,
            created_by: creator,
        },
    )
    .await
    .unwrap()
}

async fn admin(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    db.add_member(tenant, TenantRole::TenantAdmin).await.account_id
}

/// The closure as (ancestor name, descendant name, paths), sorted.
async fn closure(conn: &mut PgConnection) -> Vec<(String, String, i64)> {
    sqlx::query_as(
        "SELECT a.name, d.name, c.path_count FROM node_closure c
         JOIN node a ON a.id = c.ancestor_id JOIN node d ON d.id = c.descendant_id
         WHERE c.ancestor_id <> c.descendant_id ORDER BY 1, 2",
    )
    .fetch_all(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn project_keys_are_unique_per_tenant_and_nodes_stay_in_theirs() {
    let db = TestDb::new().await;
    let (alpha, beta) = (admin(&db, &db.tenant_a).await, admin(&db, &db.tenant_b).await);

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = create(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        None,
        alpha,
    )
    .await
    .unwrap();
    assert_eq!(
        (plat.key.as_deref(), plat.node_type, plat.version),
        (Some("PLAT"), NodeType::Project, 1)
    );
    assert_eq!(
        create(&mut tx, NodeType::Project, "Another", Some("PLAT"), None, alpha).await,
        Err(WriteConflict::KeyTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    create(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        None,
        alpha,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Another tenant has its own PLAT, and sees nothing of alpha's.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    create(&mut tx, NodeType::Project, "Platform", Some("PLAT"), None, beta)
        .await
        .unwrap();
    let page = nodes::list(&mut tx, &NodeFilter::default(), None, 50)
        .await
        .unwrap();
    assert_eq!(
        page.items.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
        ["Platform"]
    );
    assert!(!nodes::exists(&mut tx, plat.id).await.unwrap());
}

#[tokio::test]
async fn the_database_enforces_the_attachment_matrix() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let portfolio = create(&mut tx, NodeType::Portfolio, "Digital", None, None, admin)
        .await
        .unwrap();
    let program = create(
        &mut tx,
        NodeType::Program,
        "IT Modernization",
        None,
        Some(portfolio.id),
        admin,
    )
    .await
    .unwrap();
    let project = create(
        &mut tx,
        NodeType::Project,
        "Identity",
        Some("PLAT"),
        Some(program.id),
        admin,
    )
    .await
    .unwrap();
    assert_eq!(
        project
            .parents
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["IT Modernization"]
    );
    create(
        &mut tx,
        NodeType::Project,
        "Sub-project",
        Some("SUB"),
        Some(project.id),
        admin,
    )
    .await
    .unwrap();
    create(
        &mut tx,
        NodeType::Portfolio,
        "Sub-portfolio",
        None,
        Some(portfolio.id),
        admin,
    )
    .await
    .unwrap();

    for (node_type, parent) in [
        (NodeType::Program, project.id),
        (NodeType::Portfolio, program.id),
        (NodeType::Portfolio, project.id),
    ] {
        // Each refusal aborts its statement: back to the savepoint.
        sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
        assert_eq!(
            create(&mut tx, node_type, "Refused", None, Some(parent), admin).await,
            Err(WriteConflict::ParentNotAllowed),
            "{node_type:?}"
        );
        sqlx::query("ROLLBACK TO SAVEPOINT s")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    assert_eq!(
        create(
            &mut tx,
            NodeType::Project,
            "Orphan",
            Some("ORPH"),
            Some(new_id()),
            admin
        )
        .await,
        Err(WriteConflict::ParentNotFound)
    );
    // A node keeps its type.
    sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
    let changed = sqlx::query("UPDATE node SET node_type = 'portfolio' WHERE id = $1")
        .bind(program.id)
        .execute(&mut *tx)
        .await;
    assert!(changed.unwrap_err().to_string().contains("node.type_fixed"));
}

#[tokio::test]
async fn the_closure_counts_paths_and_refuses_cycles() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let portfolio = create(&mut tx, NodeType::Portfolio, "P", None, None, admin)
        .await
        .unwrap();
    let a = create(&mut tx, NodeType::Program, "A", None, Some(portfolio.id), admin)
        .await
        .unwrap();
    let b = create(&mut tx, NodeType::Program, "B", None, Some(portfolio.id), admin)
        .await
        .unwrap();
    let x = create(&mut tx, NodeType::Project, "X", Some("XX"), Some(a.id), admin)
        .await
        .unwrap();
    let y = create(&mut tx, NodeType::Project, "Y", Some("YY"), Some(x.id), admin)
        .await
        .unwrap();
    // X under B too: P reaches X and Y by two paths.
    let link = |parent: Uuid, child: Uuid| {
        sqlx::query("INSERT INTO node_edge (id, tenant_id, parent_id, child_id) VALUES ($1, app_current_tenant(), $2, $3)")
            .bind(new_id())
            .bind(parent)
            .bind(child)
    };
    link(b.id, x.id).execute(&mut *tx).await.unwrap();
    assert_eq!(
        closure(&mut tx).await,
        [
            ("A".to_owned(), "X".to_owned(), 1),
            ("A".to_owned(), "Y".to_owned(), 1),
            ("B".to_owned(), "X".to_owned(), 1),
            ("B".to_owned(), "Y".to_owned(), 1),
            ("P".to_owned(), "A".to_owned(), 1),
            ("P".to_owned(), "B".to_owned(), 1),
            ("P".to_owned(), "X".to_owned(), 2),
            ("P".to_owned(), "Y".to_owned(), 2),
            ("X".to_owned(), "Y".to_owned(), 1),
        ]
    );

    // Y cannot become X's parent, nor X its own.
    for (parent, child) in [(y.id, x.id), (x.id, x.id)] {
        sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
        let refused = link(parent, child)
            .execute(&mut *tx)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("node.cycle") || refused.contains("node_edge_check"),
            "{refused}"
        );
        sqlx::query("ROLLBACK TO SAVEPOINT s")
            .execute(&mut *tx)
            .await
            .unwrap();
    }

    // Unlinking B → X takes one path away, and B's pairs.
    sqlx::query("DELETE FROM node_edge WHERE parent_id = $1 AND child_id = $2")
        .bind(b.id)
        .bind(x.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        closure(&mut tx).await,
        [
            ("A".to_owned(), "X".to_owned(), 1),
            ("A".to_owned(), "Y".to_owned(), 1),
            ("P".to_owned(), "A".to_owned(), 1),
            ("P".to_owned(), "B".to_owned(), 1),
            ("P".to_owned(), "X".to_owned(), 1),
            ("P".to_owned(), "Y".to_owned(), 1),
            ("X".to_owned(), "Y".to_owned(), 1),
        ]
    );
}

#[tokio::test]
async fn owners_are_members_and_changes_check_the_version() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let stranger = db.add_member(&db.tenant_b, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let node = create(&mut tx, NodeType::Portfolio, "Digital", None, None, admin)
        .await
        .unwrap();

    let owners = [lucas, admin];
    let changed = NodeFields {
        description: "The digital transformation",
        objectives: "Ship it",
        health: Health::AtRisk,
        ..fields("  Digital transformation ", &owners)
    };
    let updated = nodes::update(&mut tx, node.id, Some(&[1]), &changed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.name, "Digital transformation");
    assert_eq!((updated.health, updated.version), (Health::AtRisk, 2));
    let mut names: Vec<Uuid> = updated.owners.iter().map(|o| o.account_id).collect();
    names.sort();
    let mut expected = vec![lucas, admin];
    expected.sort();
    assert_eq!(names, expected);
    assert_eq!(
        nodes::update(&mut tx, node.id, Some(&[1]), &changed)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        nodes::update(&mut tx, new_id(), None, &changed).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    // Removing one owner keeps the other.
    let fewer = [lucas];
    let updated = nodes::update(
        &mut tx,
        node.id,
        None,
        &NodeFields {
            owners: &fewer,
            ..changed.clone()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        updated.owners.iter().map(|o| o.account_id).collect::<Vec<_>>(),
        [lucas]
    );

    // A member of another tenant cannot own it.
    let strangers = [stranger];
    assert_eq!(
        nodes::update(
            &mut tx,
            node.id,
            None,
            &NodeFields {
                owners: &strangers,
                ..changed
            }
        )
        .await
        .unwrap(),
        Err(WriteConflict::UnknownOwner)
    );
}

/// Names of the nodes of `filter`, by name.
async fn names(conn: &mut PgConnection, filter: NodeFilter<'_>) -> Vec<String> {
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

#[tokio::test]
async fn a_project_has_several_weighted_parents() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let digital = create(&mut tx, NodeType::Portfolio, "Digital", None, None, admin)
        .await
        .unwrap();
    let modernization = create(
        &mut tx,
        NodeType::Program,
        "IT Modernization",
        None,
        Some(digital.id),
        admin,
    )
    .await
    .unwrap();
    let compliance = create(
        &mut tx,
        NodeType::Program,
        "Compliance",
        None,
        Some(digital.id),
        admin,
    )
    .await
    .unwrap();
    let plat = create(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        Some(modernization.id),
        admin,
    )
    .await
    .unwrap();

    assert_eq!(
        nodes::link(&mut tx, compliance.id, plat.id, 40).await.unwrap(),
        Ok(LinkChange::Created)
    );
    let parents = nodes::parent_links(&mut tx, plat.id).await.unwrap();
    assert_eq!(
        parents
            .iter()
            .map(|l| (l.name.as_str(), l.weight_percent))
            .collect::<Vec<_>>(),
        [("Compliance", 40), ("IT Modernization", 100)]
    );
    // Each parent comes with its own parents.
    assert_eq!(parents[0].parents[0].name, "Digital");

    // The same weighting changes nothing; another one updates the link.
    assert_eq!(
        nodes::link(&mut tx, compliance.id, plat.id, 40).await.unwrap(),
        Ok(LinkChange::Unchanged)
    );
    assert_eq!(
        nodes::link(&mut tx, compliance.id, plat.id, 60).await.unwrap(),
        Ok(LinkChange::Updated)
    );
    let children = nodes::child_links(&mut tx, compliance.id).await.unwrap();
    assert_eq!(
        children
            .iter()
            .map(|l| (l.name.as_str(), l.key.as_deref(), l.weight_percent))
            .collect::<Vec<_>>(),
        [("Identity platform", Some("PLAT"), 60)]
    );

    // A diamond: Digital reaches PLAT by two paths, and lists it once.
    let paths: i64 = sqlx::query_scalar(
        "SELECT path_count FROM node_closure WHERE ancestor_id = $1 AND descendant_id = $2",
    )
    .bind(digital.id)
    .bind(plat.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(paths, 2);
    let descendants = NodeFilter {
        descendant_of: Some(digital.id),
        ..NodeFilter::default()
    };
    assert_eq!(
        names(&mut tx, descendants).await,
        ["Compliance", "IT Modernization", "Identity platform"]
    );
    let ancestors = NodeFilter {
        ancestor_of: Some(plat.id),
        ..NodeFilter::default()
    };
    assert_eq!(
        names(&mut tx, ancestors).await,
        ["Compliance", "Digital", "IT Modernization"]
    );

    // Unknown nodes; a refused link.
    assert_eq!(
        nodes::link(&mut tx, compliance.id, new_id(), 100).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        nodes::link(&mut tx, new_id(), plat.id, 100).await.unwrap(),
        Err(WriteConflict::ParentNotFound)
    );
    sqlx::query("SAVEPOINT s").execute(&mut *tx).await.unwrap();
    assert_eq!(
        nodes::link(&mut tx, plat.id, compliance.id, 100).await.unwrap(),
        Err(WriteConflict::ParentNotAllowed)
    );
    sqlx::query("ROLLBACK TO SAVEPOINT s")
        .execute(&mut *tx)
        .await
        .unwrap();

    // Removing a parent link: one path left, Compliance no longer above.
    assert!(nodes::unlink(&mut tx, modernization.id, plat.id).await.unwrap());
    assert!(!nodes::unlink(&mut tx, modernization.id, plat.id).await.unwrap());
    assert_eq!(names(&mut tx, ancestors).await, ["Compliance", "Digital"]);
    let paths: i64 = sqlx::query_scalar(
        "SELECT path_count FROM node_closure WHERE ancestor_id = $1 AND descendant_id = $2",
    )
    .bind(digital.id)
    .bind(plat.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(paths, 1);
}

#[tokio::test]
async fn candidate_parents_say_why_they_are_refused() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let security = create(&mut tx, NodeType::Program, "Cybersecurity", None, None, admin)
        .await
        .unwrap();
    let plat = create(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        Some(security.id),
        admin,
    )
    .await
    .unwrap();
    create(
        &mut tx,
        NodeType::Project,
        "Cyber permissions",
        Some("CYBP"),
        Some(plat.id),
        admin,
    )
    .await
    .unwrap();
    create(
        &mut tx,
        NodeType::Project,
        "Cyber resilience",
        Some("CYBR"),
        None,
        admin,
    )
    .await
    .unwrap();
    create(
        &mut tx,
        NodeType::Project,
        "Payroll 50%",
        Some("PAY"),
        None,
        admin,
    )
    .await
    .unwrap();

    let found = nodes::parent_candidates(&mut tx, plat.id, Some(" CYBER"), 20)
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|c| (c.name.as_str(), c.descends_from_child, c.already_parent))
            .collect::<Vec<_>>(),
        [
            ("Cyber permissions", true, false),
            ("Cyber resilience", false, false),
            ("Cybersecurity", false, true),
        ]
    );
    assert_eq!(found[0].parents[0].name, "Identity platform");
    // Keys match too, wildcards literally; the node itself is never offered.
    let by_key = nodes::parent_candidates(&mut tx, plat.id, Some("cybr"), 20)
        .await
        .unwrap();
    assert_eq!(
        by_key.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Cyber resilience"]
    );
    let literal = nodes::parent_candidates(&mut tx, plat.id, Some("50%"), 20)
        .await
        .unwrap();
    assert_eq!(
        literal.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Payroll 50%"]
    );
    let all = nodes::parent_candidates(&mut tx, plat.id, None, 2).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(
        nodes::parent_candidates(&mut tx, plat.id, Some("identity"), 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn two_links_made_at_once_cannot_close_a_cycle() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let a = create(&mut tx, NodeType::Project, "A", Some("AA"), None, admin)
        .await
        .unwrap();
    let b = create(&mut tx, NodeType::Project, "B", Some("BB"), None, admin)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // A over B, not committed yet; meanwhile, B over A waits for it.
    let mut first = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        nodes::link(&mut first, a.id, b.id, 100).await.unwrap(),
        Ok(LinkChange::Created)
    );
    let pool = db.app_pool.clone();
    let tenant = db.tenant_a.id;
    let second = tokio::spawn(async move {
        let mut tx = begin_tenant_tx(&pool, tenant).await.unwrap();
        nodes::link(&mut tx, b.id, a.id, 100).await.unwrap()
    });
    // Wait until the second link waits for the hierarchy lock.
    let mut waiting = false;
    for _ in 0..100 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks l JOIN pg_database d ON d.oid = l.database
             WHERE d.datname = current_database() AND l.locktype = 'advisory' AND NOT l.granted",
        )
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
        if blocked > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(waiting, "the second link should wait for the first");
    first.commit().await.unwrap();
    assert_eq!(second.await.unwrap(), Err(WriteConflict::Cycle));
}

#[tokio::test]
async fn confidentiality_is_set_with_the_version_it_was_read_at() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = create(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        None,
        admin,
    )
    .await
    .unwrap();
    assert!(!plat.confidential);

    let confidential = nodes::set_confidential(&mut tx, plat.id, Some(&[1]), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((confidential.confidential, confidential.version), (true, 2));
    assert_eq!(
        nodes::set_confidential(&mut tx, plat.id, Some(&[1]), false)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        nodes::set_confidential(&mut tx, new_id(), None, true)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    let open = nodes::set_confidential(&mut tx, plat.id, None, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((open.confidential, open.version), (false, 3));

    // The opaque id of a veiled node: stable, and not the node's.
    assert_eq!(nodes::opaque_id(plat.id), nodes::opaque_id(plat.id));
    assert_ne!(nodes::opaque_id(plat.id), plat.id);
    assert_ne!(nodes::opaque_id(plat.id), nodes::opaque_id(new_id()));
}

#[tokio::test]
async fn an_archived_project_leaves_the_active_lists_until_it_is_restored() {
    let db = TestDb::new().await;
    let admin = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let program = create(&mut tx, NodeType::Program, "IT Modernization", None, None, admin)
        .await
        .unwrap();
    let plat = create(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        Some(program.id),
        admin,
    )
    .await
    .unwrap();
    let cyber = create(
        &mut tx,
        NodeType::Project,
        "Cyber permissions",
        Some("CYBP"),
        Some(plat.id),
        admin,
    )
    .await
    .unwrap();
    let archived_only = NodeFilter {
        archived: true,
        ..NodeFilter::default()
    };

    // What the project stands between.
    let around = nodes::surroundings(&mut tx, plat.id).await.unwrap();
    assert!(around.has_active_children && !around.has_archived_parent);

    // The sub-project first, with the version it was read at.
    let archived = nodes::set_archived(&mut tx, cyber.id, Some(&[cyber.version]), true)
        .await
        .unwrap()
        .unwrap();
    assert!(archived.archived);
    assert_eq!(archived.version, cyber.version + 1);
    let archived_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT archived_at FROM node WHERE id = $1")
            .bind(cyber.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(archived_at.is_some());
    assert_eq!(
        nodes::set_archived(&mut tx, cyber.id, Some(&[cyber.version]), false)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(
        nodes::set_archived(&mut tx, new_id(), None, true).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    // Archived again: it keeps the date it was archived at.
    nodes::set_archived(&mut tx, cyber.id, None, true)
        .await
        .unwrap()
        .unwrap();
    let again: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT archived_at FROM node WHERE id = $1")
            .bind(cyber.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(again, archived_at);

    // Out of the active lists, in the archived one; out of its parent's children.
    assert_eq!(
        names(&mut tx, NodeFilter::default()).await,
        ["IT Modernization", "Identity platform"]
    );
    assert_eq!(names(&mut tx, archived_only).await, ["Cyber permissions"]);
    assert!(nodes::child_links(&mut tx, plat.id).await.unwrap().is_empty());
    let around = nodes::surroundings(&mut tx, plat.id).await.unwrap();
    assert!(!around.has_active_children);
    let around = nodes::surroundings(&mut tx, cyber.id).await.unwrap();
    assert!(!around.has_archived_parent);

    // Its parent archived too: it shows its sub-project, archived with it.
    nodes::set_archived(&mut tx, plat.id, None, true)
        .await
        .unwrap()
        .unwrap();
    let children: Vec<String> = nodes::child_links(&mut tx, plat.id)
        .await
        .unwrap()
        .into_iter()
        .map(|link| link.name)
        .collect();
    assert_eq!(children, ["Cyber permissions"]);
    assert!(nodes::child_links(&mut tx, program.id).await.unwrap().is_empty());
    assert!(
        nodes::surroundings(&mut tx, cyber.id)
            .await
            .unwrap()
            .has_archived_parent
    );
    assert_eq!(
        names(&mut tx, archived_only).await,
        ["Cyber permissions", "Identity platform"]
    );

    // Restored: back in the lists and under its parent.
    let restored = nodes::set_archived(&mut tx, plat.id, None, false)
        .await
        .unwrap()
        .unwrap();
    assert!(!restored.archived);
    assert_eq!(
        names(&mut tx, NodeFilter::default()).await,
        ["IT Modernization", "Identity platform"]
    );
    let children: Vec<String> = nodes::child_links(&mut tx, program.id)
        .await
        .unwrap()
        .into_iter()
        .map(|link| link.name)
        .collect();
    assert_eq!(children, ["Identity platform"]);
}
