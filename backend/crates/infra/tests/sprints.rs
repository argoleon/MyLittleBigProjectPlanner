//! Integration tests of the sprints' SQL (L09-US01) on a real PostgreSQL:
//! row-level security keeps sprints, their scope and the ranks to their
//! tenant; a ticket is in one open sprint at most and its stays are kept as
//! history; ranks place a ticket between two others, rank the unranked
//! ones last, and rebalance in order.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::{Health, NodeType};
use infra::boards::{self, BoardInput};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields};
use infra::sprints::{self, Place, SprintInput, WriteConflict};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

async fn project(conn: &mut PgConnection, key: &str, by: Uuid) -> Uuid {
    let new = NewNode {
        id: new_id(),
        node_type: NodeType::Project,
        key,
        fields: NodeFields {
            key: None,
            name: key,
            description: "",
            objectives: "",
            start_date: None,
            end_date: None,
            health: Health::OnTrack,
            owners: &[],
        },
        parent: None,
        created_by: by,
    };
    nodes::insert(conn, &new).await.unwrap().unwrap().id
}

async fn board(conn: &mut PgConnection, node: Uuid, by: Uuid, name: &str, kind: &str) -> Uuid {
    let input = BoardInput {
        kind,
        name,
        filter: "",
        estimation: "points",
        swimlanes: "none",
        quick_filters: &[],
        columns: &[],
    };
    boards::insert(conn, new_id(), node, by, &input)
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn ticket(conn: &mut PgConnection, project: Uuid, number: i32) -> Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary)
        VALUES ($1, app_current_tenant(), $2, $3, 'PLAT-' || $3,
                (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                 AND v.state = 'published' WHERE t.key = 'task' LIMIT 1),
                (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                 AND v.state = 'published' WHERE s.key = 'todo' LIMIT 1),
                'Ticket ' || $3)
        RETURNING id
        "#,
    )
    .bind(new_id())
    .bind(project)
    .bind(number)
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn sprint(conn: &mut PgConnection, board: Uuid, by: Uuid, name: &str) -> Uuid {
    sprints::lock_board(conn, board).await.unwrap();
    let sequence = sprints::next_sequence(conn, board).await.unwrap();
    let input = SprintInput {
        name,
        goal: None,
        start_date: None,
        end_date: None,
    };
    sprints::insert(conn, new_id(), board, sequence, by, &input)
        .await
        .unwrap()
        .id
}

async fn admin_of(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    db.add_member(tenant, infra::accounts::TenantRole::TenantAdmin)
        .await
        .account_id
}

async fn count(conn: &mut PgConnection, table: &str) -> i64 {
    // The table names are this file's own constants.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn sprints_their_scope_and_ranks_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, "PLAT", by).await;
    let scrum = board(&mut tx, plat, by, "Scrum", "scrum").await;
    let issue = ticket(&mut tx, plat, 1).await;
    let s1 = sprint(&mut tx, scrum, by, "PLAT Sprint 1").await;
    assert!(sprints::put(&mut tx, issue, s1, by).await.unwrap());
    sprints::rank_last(&mut tx, scrum, &[issue]).await.unwrap();
    tx.commit().await.unwrap();

    let other = admin_of(&db, b).await;
    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    for table in ["sprint", "sprint_issue", "board_rank"] {
        assert_eq!(count(&mut tx, table).await, 0, "{table}");
    }
    assert_eq!(sprints::find(&mut tx, s1).await.unwrap(), None);
    assert!(
        sprints::open_sprint_of(&mut tx, &[issue])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        sprints::ranks_of(&mut tx, scrum, &[issue])
            .await
            .unwrap()
            .is_empty()
    );
    let input = SprintInput {
        name: "Theirs",
        goal: None,
        start_date: None,
        end_date: None,
    };
    assert_eq!(
        sprints::update(&mut tx, s1, None, &input).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        sprints::delete(&mut tx, s1, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    tx.commit().await.unwrap();
    // Tenant B cannot hang a sprint on tenant A's board.
    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    let refused = sprints::insert(&mut tx, new_id(), scrum, 1, other, &input).await;
    assert!(refused.is_err(), "{refused:?}");
    tx.rollback().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(sprints::find(&mut tx, s1).await.unwrap().unwrap().version, 1);
    assert_eq!(
        sprints::open_sprint_of(&mut tx, &[issue]).await.unwrap()[&issue],
        s1
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_ticket_is_in_one_open_sprint_and_its_stays_are_kept() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, "PLAT", by).await;
    let scrum = board(&mut tx, plat, by, "Scrum", "scrum").await;
    let other = board(&mut tx, plat, by, "Other", "scrum").await;
    let issue = ticket(&mut tx, plat, 1).await;
    let s1 = sprint(&mut tx, scrum, by, "PLAT Sprint 1").await;
    let s2 = sprint(&mut tx, scrum, by, "PLAT Sprint 2").await;
    let elsewhere = sprint(&mut tx, other, by, "Other Sprint 1").await;
    let numbers: Vec<i32> = sprints::open_of_board(&mut tx, scrum)
        .await
        .unwrap()
        .iter()
        .map(|s| s.sequence)
        .collect();
    assert_eq!(numbers, [1, 2]);
    assert_eq!(sprints::find(&mut tx, s1).await.unwrap().unwrap().state, "future");

    assert!(sprints::put(&mut tx, issue, s1, by).await.unwrap());
    assert!(
        !sprints::put(&mut tx, issue, s1, by).await.unwrap(),
        "already there"
    );
    assert!(sprints::put(&mut tx, issue, s2, by).await.unwrap());
    assert_eq!(
        sprints::open_sprint_of(&mut tx, &[issue]).await.unwrap()[&issue],
        s2
    );
    assert!(sprints::take_out(&mut tx, issue, by).await.unwrap());
    assert!(!sprints::take_out(&mut tx, issue, by).await.unwrap());
    assert!(sprints::put(&mut tx, issue, s1, by).await.unwrap());
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    // Each stay is kept: left Sprint 1 for Sprint 2, then the backlog, then back.
    let stays = sprints::history(&mut tx, s1).await.unwrap();
    assert_eq!(stays.len(), 2);
    assert!(stays[0].removed_at.is_some() && stays[1].removed_at.is_none());
    let stays = sprints::history(&mut tx, s2).await.unwrap();
    assert!(stays.len() == 1 && stays[0].removed_at.is_some());
    tx.commit().await.unwrap();

    // The database holds the rule too: no second open stay.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let refused = sqlx::query(
        "INSERT INTO sprint_issue (id, tenant_id, sprint_id, issue_id) VALUES ($1, app_current_tenant(), $2, $3)",
    )
    .bind(new_id())
    .bind(elsewhere)
    .bind(issue)
    .execute(&mut *tx)
    .await;
    assert!(refused.is_err(), "{refused:?}");
    tx.rollback().await.unwrap();

    // Deleting a sprint takes its scope with it.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        sprints::delete(&mut tx, s1, Some(&[2])).await.unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    assert_eq!(sprints::delete(&mut tx, s1, Some(&[1])).await.unwrap(), Ok(()));
    assert!(
        sprints::open_sprint_of(&mut tx, &[issue])
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
}

/// The ranked tickets among `issues`, in the board's order.
async fn ranked(conn: &mut PgConnection, board: Uuid, issues: &[Uuid]) -> Vec<Uuid> {
    let mut ranked: Vec<(String, Uuid)> = sprints::ranks_of(conn, board, issues)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, rank)| (rank, id))
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, id)| id).collect()
}

#[tokio::test]
async fn ranks_place_tickets_between_their_neighbours() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, "PLAT", by).await;
    let kanban = board(&mut tx, plat, by, "Flow", "kanban").await;
    let scrum = board(&mut tx, plat, by, "Scrum", "scrum").await;
    let mut issues = Vec::new();
    for n in 1..=5 {
        issues.push(ticket(&mut tx, plat, n).await);
    }
    let (t1, t2, t3, t4, t5) = (issues[0], issues[1], issues[2], issues[3], issues[4]);
    sprints::lock_board(&mut tx, kanban).await.unwrap();
    sprints::rank_last(&mut tx, kanban, &[t1, t2, t3]).await.unwrap();

    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t1, t2, t3]);

    // After t3, before t1, after t1 (its own place left behind), last.
    let at = |after, before| Place { after, before };
    assert!(
        sprints::rank_at(&mut tx, kanban, t4, at(Some(t1), None))
            .await
            .unwrap()
    );
    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t1, t4, t2, t3]);
    assert!(
        sprints::rank_at(&mut tx, kanban, t3, at(None, Some(t1)))
            .await
            .unwrap()
    );
    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t3, t1, t4, t2]);
    assert!(
        sprints::rank_at(&mut tx, kanban, t2, at(Some(t3), None))
            .await
            .unwrap()
    );
    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t3, t2, t1, t4]);
    assert!(
        sprints::rank_at(&mut tx, kanban, t3, at(None, None))
            .await
            .unwrap()
    );
    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t2, t1, t4, t3]);
    // A neighbour without a rank: nothing written.
    assert!(
        !sprints::rank_at(&mut tx, kanban, t1, at(Some(t5), None))
            .await
            .unwrap()
    );
    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t2, t1, t4, t3]);

    // Ranks are per board.
    assert!(
        sprints::ranks_of(&mut tx, scrum, &issues)
            .await
            .unwrap()
            .is_empty()
    );

    // Again and again at the same place: the ranks get rebalanced, in order.
    for _ in 0..300 {
        assert!(
            sprints::rank_at(&mut tx, kanban, t3, at(Some(t2), None))
                .await
                .unwrap()
        );
        assert!(
            sprints::rank_at(&mut tx, kanban, t1, at(Some(t2), None))
                .await
                .unwrap()
        );
    }
    let ranks = sprints::ranks_of(&mut tx, kanban, &issues).await.unwrap();
    assert!(
        ranks.values().all(|r| r.len() <= domain::rank::REBALANCE_LEN),
        "{ranks:?}"
    );
    assert_eq!(ranked(&mut tx, kanban, &issues).await, [t2, t1, t3, t4]);
    tx.commit().await.unwrap();
}

/// The keys `query` finds, through the search's own compiler.
async fn found(conn: &mut PgConnection, query: &str, by: Uuid) -> Vec<String> {
    let context = infra::search::Context {
        account_id: by,
        time_zone: "Europe/Paris",
        visible_projects: None,
        audience: infra::full_text::Audience::Members,
    };
    let parsed = domain::query::parse(query).unwrap();
    let page = infra::search::search(conn, &parsed, &context, None, 50, true)
        .await
        .unwrap_or_else(|e| panic!("{query}: {e:?}"));
    let mut keys: Vec<String> = page.items.into_iter().map(|i| i.key).collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn the_query_language_finds_tickets_by_sprint() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, "PLAT", by).await;
    let scrum = board(&mut tx, plat, by, "Scrum", "scrum").await;
    let (t1, t2, t3) = (
        ticket(&mut tx, plat, 1).await,
        ticket(&mut tx, plat, 2).await,
        ticket(&mut tx, plat, 3).await,
    );
    // PLAT-4, a sub-task of PLAT-2.
    sqlx::query(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, parent_id)
        SELECT $1, tenant_id, project_id, 4, 'PLAT-4', type_id, status_id, 'Sub-task', id FROM issue WHERE id = $2
        "#,
    )
    .bind(new_id())
    .bind(t2)
    .execute(&mut *tx)
    .await
    .unwrap();
    let open = sprint(&mut tx, scrum, by, "PLAT Sprint 2").await;
    let closed = sprint(&mut tx, scrum, by, "PLAT Sprint 1").await;
    sprints::put(&mut tx, t1, closed, by).await.unwrap();
    sqlx::query(
        "UPDATE sprint SET state = 'closed', started_at = now(), closed_at = now(), start_date = '2026-11-02', \
         end_date = '2026-11-13', committed_count = 1, completed_count = 0 WHERE id = $1",
    )
        .bind(closed)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE sprint_issue SET sprint_open = false WHERE sprint_id = $1")
        .bind(closed)
        .execute(&mut *tx)
        .await
        .unwrap();
    sprints::put(&mut tx, t2, open, by).await.unwrap();
    // PLAT-3 came and left: it counts nowhere.
    sprints::put(&mut tx, t3, open, by).await.unwrap();
    sprints::take_out(&mut tx, t3, by).await.unwrap();

    assert_eq!(
        found(&mut tx, "sprint IN openSprints()", by).await,
        ["PLAT-2", "PLAT-4"]
    );
    assert_eq!(found(&mut tx, "sprint = \"plat sprint 1\"", by).await, ["PLAT-1"]);
    assert_eq!(
        found(&mut tx, "sprint IN (\"PLAT Sprint 1\", \"PLAT Sprint 2\")", by).await,
        ["PLAT-1", "PLAT-2", "PLAT-4"]
    );
    assert_eq!(
        found(&mut tx, "sprint NOT IN openSprints()", by).await,
        ["PLAT-1"]
    );
    assert_eq!(
        found(&mut tx, "sprint != \"PLAT Sprint 1\"", by).await,
        ["PLAT-2", "PLAT-4"]
    );
    assert_eq!(found(&mut tx, "sprint IS EMPTY", by).await, ["PLAT-3"]);
    assert_eq!(
        found(&mut tx, "sprint IS NOT EMPTY", by).await,
        ["PLAT-1", "PLAT-2", "PLAT-4"]
    );
    assert_eq!(
        found(&mut tx, "NOT sprint IN openSprints()", by).await,
        ["PLAT-1", "PLAT-3"]
    );
    tx.commit().await.unwrap();
}

/// A stay as kept: ticket, committed, estimate at the start, completed, estimate at the close.
type ScopeLine = (Uuid, bool, Option<f64>, Option<bool>, Option<f64>);

#[tokio::test]
async fn a_sprint_starts_alone_and_closes_keeping_its_scope() {
    use infra::sprints::Estimated;
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let by = admin_of(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, "PLAT", by).await;
    let scrum = board(&mut tx, plat, by, "Scrum", "scrum").await;
    let (t1, t2, t3) = (
        ticket(&mut tx, plat, 1).await,
        ticket(&mut tx, plat, 2).await,
        ticket(&mut tx, plat, 3).await,
    );
    let s1 = sprint(&mut tx, scrum, by, "PLAT Sprint 1").await;
    let s2 = sprint(&mut tx, scrum, by, "PLAT Sprint 2").await;
    sprints::put(&mut tx, t1, s1, by).await.unwrap();
    sprints::put(&mut tx, t2, s1, by).await.unwrap();
    let dates = SprintInput {
        name: "PLAT Sprint 1",
        goal: Some("OIDC"),
        start_date: Some(time::macros::date!(2026 - 11 - 30)),
        end_date: Some(time::macros::date!(2026 - 12 - 11)),
    };
    let estimated = |issue, estimate, done| Estimated {
        issue,
        estimate,
        done,
    };
    let committed = [estimated(t1, Some(8.0), false), estimated(t2, None, false)];
    assert_eq!(
        sprints::start(&mut tx, s1, Some(&[9]), &dates, &committed, (2, 8.0))
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    let started = sprints::start(&mut tx, s1, Some(&[1]), &dates, &committed, (2, 8.0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            started.state.as_str(),
            started.committed_count,
            started.committed_estimate
        ),
        ("active", Some(2), Some(8.0))
    );
    assert!(started.started_at.is_some());
    assert_eq!(
        sprints::active_of_board(&mut tx, scrum)
            .await
            .unwrap()
            .unwrap()
            .id,
        s1
    );
    assert_eq!(sprints::issues_in(&mut tx, s1).await.unwrap(), [t1, t2]);
    // Started already: nothing changes.
    assert_eq!(
        sprints::start(&mut tx, s1, None, &dates, &committed, (2, 8.0))
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    // Added once started: not committed.
    sprints::put(&mut tx, t3, s1, by).await.unwrap();
    tx.commit().await.unwrap();

    // The database holds one active sprint per board.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let refused = sqlx::query(
        "UPDATE sprint SET state = 'active', started_at = now(), start_date = '2026-12-14', end_date = '2026-12-25', \
         committed_count = 0 WHERE id = $1",
    )
    .bind(s2)
    .execute(&mut *tx)
    .await;
    assert!(refused.is_err(), "{refused:?}");
    tx.rollback().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let stays = [
        estimated(t1, Some(8.0), true),
        estimated(t2, Some(3.0), false),
        estimated(t3, None, false),
    ];
    let closed = sprints::close(&mut tx, s1, Some(&[2]), &stays, (1, 8.0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            closed.state.as_str(),
            closed.completed_count,
            closed.completed_estimate
        ),
        ("closed", Some(1), Some(8.0))
    );
    assert!(closed.closed_at.is_some());
    // No longer open: the tickets may go elsewhere, the stays stay.
    assert!(
        sprints::open_sprint_of(&mut tx, &[t1, t2, t3])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(sprints::put(&mut tx, t2, s2, by).await.unwrap());
    assert_eq!(sprints::active_of_board(&mut tx, scrum).await.unwrap(), None);
    let closed_ones: Vec<Uuid> = sprints::closed_of_board(&mut tx, scrum)
        .await
        .unwrap()
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(closed_ones, [s1]);
    let scope = sprints::scope(&mut tx, s1).await.unwrap();
    let rows: Vec<ScopeLine> = scope
        .iter()
        .map(|r| {
            (
                r.issue_id,
                r.committed,
                r.estimate_at_start,
                r.completed,
                r.estimate_at_close,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (t1, true, Some(8.0), Some(true), Some(8.0)),
            (t2, true, None, Some(false), Some(3.0)),
            (t3, false, None, Some(false), None),
        ]
    );
    assert_eq!(
        sprints::close(&mut tx, s1, None, &stays, (1, 8.0)).await.unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    tx.commit().await.unwrap();
}
