//! Integration tests on a real PostgreSQL: the hierarchy's nodes, their
//! owners, the attachment matrix, cycles and the closure table (L03-US01).

#![allow(clippy::unwrap_used)]

use chrono::NaiveDate;
use domain::hierarchy::{Health, NodeType};
use infra::accounts::TenantRole;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, NewNode, Node, NodeFields, WriteConflict};
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
    let page = nodes::list(&mut tx, None, None, 50).await.unwrap();
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
