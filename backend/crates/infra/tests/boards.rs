//! Integration tests of the boards' SQL (L06-US05) on a real PostgreSQL:
//! row-level security keeps each tenant's boards and columns to itself,
//! names are unique per node, writes check the version, columns are
//! replaced in order; statuses by key, projects below a node, epics and
//! blockers within what the member sees; the migration's role backfill.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::{Health, NodeType};
use infra::boards::{self, BoardInput, ColumnInput, QuickFilter, WriteConflict};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

async fn node(
    conn: &mut PgConnection,
    node_type: NodeType,
    name: &str,
    key: &str,
    parent: Option<Uuid>,
    by: Uuid,
) -> Uuid {
    let new = NewNode {
        id: new_id(),
        node_type,
        key,
        fields: NodeFields {
            key: None,
            name,
            description: "",
            objectives: "",
            start_date: None,
            end_date: None,
            health: Health::OnTrack,
            owners: &[],
        },
        parent,
        created_by: by,
    };
    nodes::insert(conn, &new).await.unwrap().unwrap().id
}

fn columns() -> Vec<(String, Vec<String>, Option<u32>)> {
    vec![
        ("To do".to_owned(), vec!["todo".to_owned()], None),
        (
            "In progress".to_owned(),
            vec!["in_progress".to_owned(), "in_review".to_owned()],
            Some(3),
        ),
        ("Done".to_owned(), vec!["done".to_owned()], None),
    ]
}

async fn create(
    conn: &mut PgConnection,
    node: Uuid,
    by: Uuid,
    name: &str,
) -> Result<boards::Board, WriteConflict> {
    let columns = columns();
    let inputs: Vec<ColumnInput<'_>> = columns
        .iter()
        .map(|(name, statuses, wip_limit)| ColumnInput {
            name,
            statuses,
            wip_limit: *wip_limit,
        })
        .collect();
    let quick = [QuickFilter {
        label: "Mine".to_owned(),
        query: "assignee = currentUser()".to_owned(),
    }];
    let input = BoardInput {
        name,
        filter: "project = PLAT",
        estimation: "points",
        swimlanes: "assignee",
        quick_filters: &quick,
        columns: &inputs,
    };
    boards::insert(conn, new_id(), node, by, &input).await.unwrap()
}

async fn ticket(
    conn: &mut PgConnection,
    project: Uuid,
    key: &str,
    type_key: &str,
    status: &str,
    parent: Option<Uuid>,
) -> Uuid {
    let number: i32 = key.rsplit('-').next().unwrap().parse().unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, parent_id)
        VALUES ($1, app_current_tenant(), $2, $3, $4,
                (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                 AND v.state = 'published' WHERE t.key = $5 LIMIT 1),
                (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                 AND v.state = 'published' WHERE s.key = $6 LIMIT 1),
                $4, $7)
        RETURNING id
        "#,
    )
    .bind(new_id())
    .bind(project)
    .bind(number)
    .bind(key)
    .bind(type_key)
    .bind(status)
    .bind(parent)
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn admin_of(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    db.add_member(tenant, infra::accounts::TenantRole::TenantAdmin)
        .await
        .account_id
}

#[tokio::test]
async fn boards_and_their_columns_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Identity platform", "PLAT", None, by).await;
    let board = create(&mut tx, plat, by, "Platform").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(board.columns.len(), 3);

    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    assert_eq!(boards::find(&mut tx, board.id).await.unwrap(), None);
    assert_eq!(boards::list(&mut tx, None, None).await.unwrap(), vec![]);
    let columns: i64 = sqlx::query_scalar("SELECT count(*) FROM board_column")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(columns, 0);
    let input = BoardInput {
        name: "Theirs",
        filter: "",
        estimation: "points",
        swimlanes: "none",
        quick_filters: &[],
        columns: &[],
    };
    assert_eq!(
        boards::update(&mut tx, board.id, None, &input).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        boards::delete(&mut tx, board.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    tx.commit().await.unwrap();
    // Tenant B cannot hang a board on tenant A's node.
    let other = admin_of(&db, b).await;
    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    let refused = boards::insert(&mut tx, new_id(), plat, other, &input).await;
    assert!(refused.is_err(), "{refused:?}");
    tx.rollback().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let kept = boards::find(&mut tx, board.id).await.unwrap().unwrap();
    assert_eq!(kept.version, 1);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_board_is_written_with_its_columns_in_order() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Identity platform", "PLAT", None, by).await;
    let ops = node(&mut tx, NodeType::Project, "Operations", "OPS", None, by).await;
    let board = create(&mut tx, plat, by, "Platform").await.unwrap();
    let names: Vec<(&str, &[String], Option<u32>)> = board
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.statuses.as_slice(), c.wip_limit))
        .collect();
    assert_eq!(
        names,
        [
            ("To do", &["todo".to_owned()][..], None),
            (
                "In progress",
                &["in_progress".to_owned(), "in_review".to_owned()][..],
                Some(3)
            ),
            ("Done", &["done".to_owned()][..], None),
        ]
    );
    assert_eq!(
        (
            board.node_key.as_str(),
            board.swimlanes.as_str(),
            board.quick_filters.len()
        ),
        ("PLAT", "assignee", 1)
    );
    tx.commit().await.unwrap();
    // A name once per node, whatever its case (the failed insert ends its
    // transaction); another node may reuse it.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        create(&mut tx, plat, by, "PLATFORM").await.unwrap_err(),
        WriteConflict::NameTaken
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    create(&mut tx, ops, by, "Platform").await.unwrap();
    assert_eq!(boards::list(&mut tx, Some(&[ops]), None).await.unwrap().len(), 1);
    assert_eq!(boards::list(&mut tx, None, Some(plat)).await.unwrap().len(), 1);
    assert_eq!(boards::list(&mut tx, Some(&[]), None).await.unwrap().len(), 0);

    // Replaced with the version read: columns and all.
    let statuses = ["todo".to_owned(), "done".to_owned()];
    let single = [ColumnInput {
        name: "All",
        statuses: &statuses,
        wip_limit: Some(10),
    }];
    let input = BoardInput {
        name: "Platform",
        filter: "project = PLAT ORDER BY due",
        estimation: "hours",
        swimlanes: "epic",
        quick_filters: &[],
        columns: &single,
    };
    assert_eq!(
        boards::update(&mut tx, board.id, Some(&[2]), &input)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    let changed = boards::update(&mut tx, board.id, Some(&[1]), &input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            changed.version,
            changed.columns.len(),
            changed.columns[0].wip_limit,
            changed.estimation.as_str()
        ),
        (2, 1, Some(10), "hours")
    );
    assert_eq!(
        boards::delete(&mut tx, board.id, Some(&[1])).await.unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    boards::delete(&mut tx, board.id, Some(&[2]))
        .await
        .unwrap()
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM board_column WHERE board_id = $1")
        .bind(board.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(left, 0);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn statuses_projects_epics_and_blockers_follow_what_is_seen() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let digital = node(&mut tx, NodeType::Portfolio, "Digital", "DIGITAL", None, by).await;
    let plat = node(
        &mut tx,
        NodeType::Project,
        "Identity platform",
        "PLAT",
        Some(digital),
        by,
    )
    .await;
    let crm = node(
        &mut tx,
        NodeType::Project,
        "Customer relations",
        "CRM",
        Some(digital),
        by,
    )
    .await;
    let ops = node(&mut tx, NodeType::Project, "Operations", "OPS", None, by).await;

    let mut below = boards::projects_below(&mut tx, digital).await.unwrap();
    below.sort();
    let mut expected = vec![plat, crm];
    expected.sort();
    assert_eq!(below, expected);
    assert_eq!(boards::projects_below(&mut tx, ops).await.unwrap(), vec![ops]);

    let statuses = boards::statuses(&mut tx, Some(&[plat])).await.unwrap();
    let keys: Vec<(&str, &str)> = statuses
        .iter()
        .map(|s| (s.key.as_str(), s.category.as_str()))
        .collect();
    assert_eq!(
        keys,
        [
            ("cancelled", "done"),
            ("done", "done"),
            ("in_progress", "in_progress"),
            ("in_review", "in_progress"),
            ("todo", "todo"),
        ]
    );
    assert_eq!(statuses[3].label["en"], "In review");
    assert_eq!(boards::statuses(&mut tx, Some(&[])).await.unwrap(), vec![]);

    // An epic in CRM above a story of PLAT above a sub-task.
    let epic = ticket(&mut tx, crm, "CRM-1", "epic", "in_progress", None).await;
    let story = ticket(&mut tx, plat, "PLAT-1", "story", "todo", Some(epic)).await;
    let sub = ticket(&mut tx, plat, "PLAT-2", "subtask", "todo", Some(story)).await;
    let alone = ticket(&mut tx, plat, "PLAT-3", "task", "todo", None).await;
    let cards = [story, sub, alone];
    let epics = boards::epics_of(&mut tx, &cards, None).await.unwrap();
    assert_eq!(epics.len(), 2);
    assert_eq!(
        (epics[&story].key.as_str(), epics[&sub].id, epics[&sub].key_number),
        ("CRM-1", epic, 1)
    );
    // CRM out of sight: its epic stays unknown.
    assert!(
        boards::epics_of(&mut tx, &cards, Some(&[plat]))
            .await
            .unwrap()
            .is_empty()
    );

    // Blockers: unfinished ones, in sight.
    let done = ticket(&mut tx, plat, "PLAT-4", "task", "done", None).await;
    let hidden = ticket(&mut tx, crm, "CRM-2", "task", "todo", None).await;
    for source in [story, done, hidden] {
        sqlx::query(
            "INSERT INTO issue_link (id, tenant_id, source_id, target_id, link_type) \
             VALUES ($1, app_current_tenant(), $2, $3, 'blocks')",
        )
        .bind(new_id())
        .bind(source)
        .bind(alone)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    let blockers = boards::blockers_of(&mut tx, &cards, None).await.unwrap();
    assert_eq!(blockers[&alone], ["CRM-2", "PLAT-1"]);
    assert!(!blockers.contains_key(&story));
    let blockers = boards::blockers_of(&mut tx, &cards, Some(&[plat])).await.unwrap();
    assert_eq!(blockers[&alone], ["PLAT-1"]);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn roles_that_manage_sprints_get_to_manage_boards() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    // A new tenant's roles: project managers and node admins manage boards.
    let seeded: Vec<(String, bool)> = sqlx::query_as(
        "SELECT label ->> 'en', 'manage_boards' = ANY (permissions) FROM role \
         WHERE tenant_id = $1 ORDER BY created_at",
    )
    .bind(a.id)
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    assert_eq!(
        seeded,
        [
            ("Reader".to_owned(), false),
            ("Contributor".to_owned(), false),
            ("Project manager".to_owned(), true),
            ("Node admin".to_owned(), true),
        ]
    );

    // A role written before the permission existed, as the migration
    // finds it, in each tenant.
    let mut tx = db.owner_pool.begin().await.unwrap();
    for tenant in [&db.tenant_a, &db.tenant_b] {
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant.id.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO role (id, tenant_id, label, permissions) \
             VALUES ($1, $2, '{\"en\": \"Scrum master\"}', ARRAY['view', 'manage_sprints']), \
                    ($3, $2, '{\"en\": \"Watcher\"}', ARRAY['view'])",
        )
        .bind(new_id())
        .bind(tenant.id)
        .bind(new_id())
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    // Run in tenant B's context: tenant A's roles stay as they are.
    let changed: i32 = sqlx::query_scalar("SELECT app_roles_manage_boards()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(changed, 1);
    let again: i32 = sqlx::query_scalar("SELECT app_roles_manage_boards()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(again, 0);
    tx.commit().await.unwrap();
    let roles: Vec<(Uuid, String, Vec<String>, i64)> = sqlx::query_as(
        "SELECT tenant_id, label ->> 'en', permissions, version FROM role \
         WHERE label ->> 'en' IN ('Scrum master', 'Watcher') ORDER BY tenant_id = $1 DESC, label ->> 'en'",
    )
    .bind(a.id)
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    let view: Vec<(&str, Vec<&str>, i64)> = roles
        .iter()
        .map(|(_, label, permissions, version)| {
            (
                label.as_str(),
                permissions.iter().map(String::as_str).collect(),
                *version,
            )
        })
        .collect();
    assert_eq!(
        view,
        [
            ("Scrum master", vec!["view", "manage_sprints"], 1),
            ("Watcher", vec!["view"], 1),
            ("Scrum master", vec!["view", "manage_sprints", "manage_boards"], 2),
            ("Watcher", vec!["view"], 1),
        ]
    );
}
