//! Integration tests: the job queue and its runner, against PostgreSQL (with
//! row-level security) and Mailpit.

#![allow(clippy::unwrap_used)]

use std::sync::Mutex;
use std::time::Duration;

use infra::email::{EMAIL_JOB, EmailJob, EmailMessage, Mailer};
use infra::jobs::ClaimedJob;
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::TestDb;
use worker::Batch;
use worker::jobs::{JobError, JobFuture, JobHandler, JobRunner, Jobs};

/// Records the jobs it runs, with the tenant of its transaction.
struct Recorder {
    runs: Mutex<Vec<(Uuid, Uuid)>>,
    outcome: Result<(), JobError>,
    /// Enqueues a follow-up job inside the job's transaction first.
    enqueue_follow_up: bool,
}

impl Recorder {
    fn new(outcome: Result<(), JobError>) -> Self {
        Self {
            runs: Mutex::new(Vec::new()),
            outcome,
            enqueue_follow_up: false,
        }
    }

    fn runs(&self) -> Vec<(Uuid, Uuid)> {
        self.runs.lock().unwrap().clone()
    }
}

impl JobHandler for Recorder {
    fn handle<'a>(&'a self, job: &'a ClaimedJob, conn: &'a mut PgConnection) -> JobFuture<'a> {
        Box::pin(async move {
            let tenant: Uuid = sqlx::query_scalar("SELECT app_current_tenant()")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            if self.enqueue_follow_up {
                infra::jobs::enqueue(conn, "test.follow_up", &serde_json::json!({}))
                    .await
                    .unwrap();
            }
            self.runs.lock().unwrap().push((job.id, tenant));
            self.outcome.clone()
        })
    }
}

async fn enqueue(db: &TestDb, tenant: Uuid, kind: &str, payload: serde_json::Value) -> Uuid {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let id = infra::jobs::enqueue(&mut tx, kind, &payload).await.unwrap();
    tx.commit().await.unwrap();
    id
}

/// A job row as the owner sees it: (status, attempts, last_error, due now).
async fn job_row(db: &TestDb, id: Uuid) -> Option<(String, i32, Option<String>, bool)> {
    sqlx::query_as("SELECT status, attempts, last_error, run_at <= now() FROM job WHERE id = $1")
        .bind(id)
        .fetch_optional(&db.owner_pool)
        .await
        .unwrap()
}

async fn job_count(db: &TestDb) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM job")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_job_exists_only_if_its_transaction_commits() {
    let db = TestDb::new().await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    infra::jobs::enqueue(&mut tx, "test.job", &serde_json::json!({}))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(job_count(&db).await, 0);

    let recorder = Recorder::new(Ok(()));
    assert_eq!(
        JobRunner::new(db.app_pool.clone()).next_batch(&recorder).await,
        Batch::Idle
    );
    assert!(recorder.runs().is_empty());
}

#[tokio::test]
async fn each_job_runs_in_its_own_tenant_and_is_removed_when_done() {
    let db = TestDb::new().await;
    let job_a = enqueue(&db, db.tenant_a.id, "test.job", serde_json::json!({})).await;
    let job_b = enqueue(&db, db.tenant_b.id, "test.job", serde_json::json!({})).await;

    let recorder = Recorder::new(Ok(()));
    let runner = JobRunner::new(db.app_pool.clone());
    assert_eq!(runner.next_batch(&recorder).await, Batch::Processed);

    let mut runs = recorder.runs();
    runs.sort();
    let mut expected = vec![(job_a, db.tenant_a.id), (job_b, db.tenant_b.id)];
    expected.sort();
    assert_eq!(runs, expected);
    assert_eq!(job_count(&db).await, 0);
    assert_eq!(runner.next_batch(&recorder).await, Batch::Idle);
}

#[tokio::test]
async fn the_application_role_sees_the_jobs_of_its_tenant_only() {
    let db = TestDb::new().await;
    enqueue(&db, db.tenant_a.id, "test.job", serde_json::json!({})).await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_b.id)
        .await
        .unwrap();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM job")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 0);
}

#[tokio::test]
async fn two_workers_run_every_job_exactly_once() {
    let db = TestDb::new().await;
    let mut ids = Vec::new();
    for i in 0..40 {
        let tenant = if i % 2 == 0 {
            db.tenant_a.id
        } else {
            db.tenant_b.id
        };
        ids.push(enqueue(&db, tenant, "test.job", serde_json::json!({ "n": i })).await);
    }

    let recorder = Recorder::new(Ok(()));
    let (first, second) = (
        JobRunner::new(db.app_pool.clone()),
        JobRunner::new(db.app_pool.clone()),
    );
    let drain = |runner: JobRunner| {
        let recorder = &recorder;
        async move { while runner.next_batch(recorder).await == Batch::Processed {} }
    };
    tokio::join!(drain(first), drain(second));

    let mut ran: Vec<Uuid> = recorder.runs().into_iter().map(|(id, _)| id).collect();
    ran.sort();
    ids.sort();
    assert_eq!(ran, ids, "every job ran, none twice");
    assert_eq!(job_count(&db).await, 0);
}

#[tokio::test]
async fn a_failed_job_is_rolled_back_and_retried_later() {
    let db = TestDb::new().await;
    let id = enqueue(&db, db.tenant_a.id, "test.job", serde_json::json!({})).await;

    let mut recorder = Recorder::new(Err(JobError::Retry("server unreachable".to_owned())));
    recorder.enqueue_follow_up = true;
    let runner = JobRunner::new(db.app_pool.clone());
    assert_eq!(runner.next_batch(&recorder).await, Batch::Processed);

    let (status, attempts, error, due) = job_row(&db, id).await.unwrap();
    assert_eq!(status, "queued");
    assert_eq!(attempts, 1);
    assert_eq!(error.as_deref(), Some("server unreachable"));
    assert!(!due, "retried after a backoff, not immediately");
    assert_eq!(job_count(&db).await, 1, "the follow-up job was rolled back");
    assert_eq!(runner.next_batch(&recorder).await, Batch::Idle);
}

#[tokio::test]
async fn a_job_fails_for_good_on_a_permanent_error_or_its_last_attempt() {
    let db = TestDb::new().await;
    let permanent = enqueue(&db, db.tenant_a.id, "test.job", serde_json::json!({})).await;
    let recorder = Recorder::new(Err(JobError::Permanent("bad payload".to_owned())));
    let runner = JobRunner::new(db.app_pool.clone());
    runner.next_batch(&recorder).await;
    let (status, attempts, error, _) = job_row(&db, permanent).await.unwrap();
    assert_eq!((status.as_str(), attempts), ("failed", 1));
    assert_eq!(error.as_deref(), Some("bad payload"));

    let last = enqueue(&db, db.tenant_a.id, "test.job", serde_json::json!({})).await;
    db.exec_as_owner(&format!("UPDATE job SET max_attempts = 1 WHERE id = '{last}'"))
        .await;
    let recorder = Recorder::new(Err(JobError::Retry("still down".to_owned())));
    runner.next_batch(&recorder).await;
    let (status, attempts, _, _) = job_row(&db, last).await.unwrap();
    assert_eq!((status.as_str(), attempts), ("failed", 1));

    assert_eq!(
        runner.next_batch(&recorder).await,
        Batch::Idle,
        "failed jobs stay failed"
    );
}

#[tokio::test]
async fn the_job_of_a_dead_worker_is_claimed_again_when_its_lease_expires() {
    let db = TestDb::new().await;
    let id = enqueue(&db, db.tenant_a.id, "test.job", serde_json::json!({})).await;

    // A worker claims the job, then dies without completing it.
    let claimed = infra::jobs::claim(&db.app_pool, "dead-worker", 10, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let recorder = Recorder::new(Ok(()));
    let runner = JobRunner::new(db.app_pool.clone());
    assert_eq!(runner.next_batch(&recorder).await, Batch::Idle, "leased");

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(runner.next_batch(&recorder).await, Batch::Processed);
    assert_eq!(recorder.runs(), [(id, db.tenant_a.id)]);
    assert_eq!(job_count(&db).await, 0);
    // The dead worker comes back: its lease is gone.
    assert!(
        !infra::jobs::complete(&db.app_pool, id, "dead-worker")
            .await
            .unwrap()
    );
}

async fn jobs() -> Jobs {
    let smtp = infra::config::Secret::new(testkit::smtp_url());
    let (valkey, prefix) = testkit::valkey().await;
    Jobs {
        mailer: Mailer::new(&smtp, "Planner <noreply@example.org>").unwrap(),
        exports: infra::exports::ExportStore::new(valkey, prefix),
        storage: Some(testkit::storage().await),
        clamav: Some(testkit::clamav_addr()),
        public_url: None,
    }
}

#[tokio::test]
async fn an_email_job_sends_the_email_in_the_recipients_language() {
    let db = TestDb::new().await;
    let to = format!("reset-{}@example.org", infra::id::new_id().simple());
    let email = EmailJob {
        to: to.clone(),
        locale: "en".to_owned(),
        message: EmailMessage::PasswordReset {
            display_name: "Lucas Bernard".to_owned(),
            tenant_name: "Alpha Corp".to_owned(),
            link: "https://alpha.example.org/reset-password?token=abc".to_owned(),
            valid_minutes: 60,
        },
    };
    let id = enqueue(
        &db,
        db.tenant_a.id,
        EMAIL_JOB,
        serde_json::to_value(&email).unwrap(),
    )
    .await;

    let runner = JobRunner::new(db.app_pool.clone());
    assert_eq!(runner.next_batch(&jobs().await).await, Batch::Processed);
    assert_eq!(job_row(&db, id).await, None, "done");

    let caught = testkit::caught_email(&to).await;
    assert!(caught.subject.contains("password"), "{}", caught.subject);
    assert!(
        caught
            .text
            .contains("https://alpha.example.org/reset-password?token=abc")
    );
}

#[tokio::test]
async fn unknown_kinds_and_unreadable_payloads_fail_without_retry() {
    let db = TestDb::new().await;
    let unknown = enqueue(&db, db.tenant_a.id, "test.unknown", serde_json::json!({})).await;
    let unreadable = enqueue(&db, db.tenant_a.id, EMAIL_JOB, serde_json::json!({ "to": 3 })).await;

    JobRunner::new(db.app_pool.clone())
        .next_batch(&jobs().await)
        .await;
    for id in [unknown, unreadable] {
        let (status, attempts, error, _) = job_row(&db, id).await.unwrap();
        assert_eq!((status.as_str(), attempts), ("failed", 1), "{error:?}");
    }
}

#[tokio::test]
async fn the_purge_job_removes_a_deleted_tenant_and_its_rows() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, infra::accounts::TenantRole::TenantAdmin)
        .await;
    db.make_platform_admin(admin.account_id).await;
    let member = db
        .add_member(&db.tenant_b, infra::accounts::TenantRole::Member)
        .await;
    enqueue(&db, db.tenant_b.id, "test.job", serde_json::json!({})).await;
    // Files of both tenants in the storage.
    let storage = testkit::storage().await;
    let gone = [
        infra::storage::upload_key(db.tenant_b.id, infra::id::new_id()),
        format!("{}/thumbnails/x.png", db.tenant_b.id),
    ];
    let kept = infra::storage::upload_key(db.tenant_a.id, infra::id::new_id());
    for key in gone.iter().chain([&kept]) {
        storage
            .write(key, b"x".to_vec().into(), "text/plain")
            .await
            .unwrap();
    }

    let job_id = infra::id::new_id();
    let mut conn = db.app_pool.acquire().await.unwrap();
    infra::platform::delete(&mut conn, admin.account_id, db.tenant_b.id, None, job_id)
        .await
        .unwrap()
        .unwrap();
    drop(conn);

    // The purge runs whatever the order of the claimed jobs.
    let runner = JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs().await).await == Batch::Processed {}

    let count = |sql: &str| {
        let sql = sql.to_owned();
        let pool = db.owner_pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        count(&format!(
            "SELECT count(*) FROM tenant WHERE id = '{}'",
            db.tenant_b.id
        ))
        .await,
        0
    );
    assert_eq!(
        count(&format!(
            "SELECT count(*) FROM account WHERE id = '{}'",
            member.account_id
        ))
        .await,
        0
    );
    assert_eq!(
        count("SELECT count(*) FROM job").await,
        0,
        "no job left, none failed"
    );
    assert_eq!(
        count(&format!(
            "SELECT count(*) FROM tenant WHERE id = '{}'",
            db.tenant_a.id
        ))
        .await,
        1
    );
    assert!(
        storage
            .keys(&format!("{}/", db.tenant_b.id))
            .await
            .unwrap()
            .is_empty(),
        "the deleted tenant's files left the storage"
    );
    assert_eq!(
        storage.size_of(&kept).await.unwrap(),
        Some(1),
        "the other tenant's stay"
    );
    storage.delete(&kept).await.unwrap();
}

#[tokio::test]
async fn an_uploaded_file_is_scanned_by_the_queue_and_its_upload_removed_when_infected() {
    use domain::attachments::Status;
    use infra::attachments::{self, AttachmentJob, NewAttachment};

    let db = TestDb::new().await;
    let tenant = db.tenant_a.id;
    let admin = db
        .add_member(&db.tenant_a, infra::accounts::TenantRole::TenantAdmin)
        .await
        .account_id;
    let storage = testkit::storage().await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let project = node(
        &mut tx,
        domain::hierarchy::NodeType::Project,
        "PLAT",
        Some("PLAT"),
        None,
        admin,
    )
    .await;
    let fields = serde_json::json!({});
    let issue = infra::issues::create(
        &mut tx,
        &infra::issues::NewIssue {
            id: infra::id::new_id(),
            project_id: project,
            project_key: "PLAT",
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: None,
            summary: "Ticket",
            description: None,
            priority: "medium",
            reporter_id: admin,
            assignee_id: None,
            start_date: None,
            due_date: None,
            fields: &fields,
        },
    )
    .await
    .unwrap();
    let mut ids = Vec::new();
    for content in [b"a clean file".to_vec(), testkit::eicar()] {
        let id = infra::id::new_id();
        let key = infra::storage::upload_key(tenant, id);
        attachments::insert(
            &mut tx,
            &NewAttachment {
                id,
                issue_id: issue.id,
                name: "file",
                size: content.len() as i64,
                media_type: "text/plain",
                object_key: &key,
                uploaded_by: admin,
            },
        )
        .await
        .unwrap();
        storage.write(&key, content.into(), "text/plain").await.unwrap();
        attachments::mark_uploaded(&mut tx, id).await.unwrap();
        let payload = serde_json::to_value(AttachmentJob { attachment_id: id }).unwrap();
        infra::jobs::enqueue(&mut tx, attachments::SCAN_JOB, &payload)
            .await
            .unwrap();
        ids.push(id);
    }
    tx.commit().await.unwrap();

    let runner = JobRunner::new(db.app_pool.clone());
    while runner.next_batch(&jobs().await).await == Batch::Processed {}

    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    let clean = attachments::find(&mut tx, ids[0]).await.unwrap().unwrap();
    let infected = attachments::find(&mut tx, ids[1]).await.unwrap().unwrap();
    assert_eq!((clean.status, infected.status), (Status::Clean, Status::Infected));
    assert_eq!(
        storage.keys(&format!("{tenant}/")).await.unwrap(),
        [clean.object_key],
        "the infected upload was removed by its job"
    );
    let left: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT kind, status, last_error FROM job WHERE kind IN ('attachment.scan', 'storage.delete')",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    assert_eq!(left, [], "no scan nor removal left, none failed");
}

/// A node of `node_type` named `name`, under `parent`, in the tenant of `conn`.
async fn node(
    conn: &mut PgConnection,
    node_type: domain::hierarchy::NodeType,
    name: &str,
    code: Option<&str>,
    parent: Option<Uuid>,
    creator: Uuid,
) -> Uuid {
    let id = infra::id::new_id();
    infra::nodes::insert(
        conn,
        &infra::nodes::NewNode {
            id,
            node_type,
            key: code,
            fields: infra::nodes::NodeFields {
                name,
                description: "",
                objectives: "",
                start_date: None,
                end_date: None,
                health: domain::hierarchy::Health::OnTrack,
                owners: &[],
            },
            parent,
            created_by: creator,
        },
    )
    .await
    .unwrap()
    .unwrap();
    id
}

async fn aggregate_of(db: &TestDb, tenant: Uuid, node: Uuid) -> Option<domain::aggregates::Aggregate> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant).await.unwrap();
    infra::aggregates::find(&mut tx, &[node])
        .await
        .unwrap()
        .remove(&node)
}

#[tokio::test]
async fn the_aggregates_of_every_ancestor_follow_a_change_within_seconds() {
    use domain::aggregates::Figures;
    use domain::hierarchy::NodeType;
    use infra::accounts::TenantRole;

    let db = TestDb::new().await;
    let figures = |planned_minutes, todo| Figures {
        planned_minutes,
        todo,
        ..Figures::default()
    };

    // A portfolio, a program and a project in one tenant; a project in
    // another.
    let creator = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    let portfolio = node(&mut tx, NodeType::Portfolio, "Digital", None, None, creator).await;
    let program = node(
        &mut tx,
        NodeType::Program,
        "Compliance",
        None,
        Some(portfolio),
        creator,
    )
    .await;
    let project = node(
        &mut tx,
        NodeType::Project,
        "Archive",
        Some("ARCH"),
        Some(program),
        creator,
    )
    .await;
    infra::aggregates::set_figures(&mut tx, project, &figures(600, 12))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let other = db
        .add_member(&db.tenant_b, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_b.id)
        .await
        .unwrap();
    let elsewhere = node(&mut tx, NodeType::Project, "Elsewhere", Some("ELSE"), None, other).await;
    infra::aggregates::set_figures(&mut tx, elsewhere, &figures(90, 3))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(aggregate_of(&db, db.tenant_a.id, portfolio).await, None);

    // The worker polls its queue, as `worker::run_loop` does.
    let handler = jobs().await;
    let runner = JobRunner::new(db.app_pool.clone());
    let changed = std::time::Instant::now();
    let wait_for = |expected_minutes: u64| {
        let (db, runner, handler) = (&db, &runner, &handler);
        async move {
            loop {
                runner.next_batch(handler).await;
                let found = aggregate_of(db, db.tenant_a.id, portfolio).await;
                if found.is_some_and(|a| a.figures.planned_minutes == expected_minutes) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait_for(600))
        .await
        .expect("the aggregates are computed");
    let elapsed = changed.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "within seconds, not {elapsed:?}"
    );

    let top = aggregate_of(&db, db.tenant_a.id, portfolio).await.unwrap();
    assert_eq!((top.projects, top.figures.open()), (1, 12));
    let middle = aggregate_of(&db, db.tenant_a.id, program).await.unwrap();
    assert_eq!(middle.figures.planned_minutes, 600);
    // Each tenant's job computed its own nodes.
    let other_tenant = async {
        loop {
            runner.next_batch(&handler).await;
            if let Some(found) = aggregate_of(&db, db.tenant_b.id, elsewhere).await {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let apart = tokio::time::timeout(Duration::from_secs(10), other_tenant)
        .await
        .expect("the other tenant's aggregates are computed");
    assert_eq!((apart.projects, apart.figures.planned_minutes), (1, 90));
    assert_eq!(aggregate_of(&db, db.tenant_b.id, portfolio).await, None);
    assert_eq!(aggregate_of(&db, db.tenant_a.id, elsewhere).await, None);

    // A ticket closed in the project, later on: every ancestor follows.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, db.tenant_a.id)
        .await
        .unwrap();
    infra::aggregates::set_figures(&mut tx, project, &figures(540, 11))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let changed = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(10), wait_for(540))
        .await
        .expect("the aggregates follow");
    assert!(changed.elapsed() < Duration::from_secs(5));
    let middle = aggregate_of(&db, db.tenant_a.id, program).await.unwrap();
    assert_eq!((middle.figures.planned_minutes, middle.figures.open()), (540, 11));

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM job")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "no job left, none failed");
}

#[tokio::test]
async fn the_outbox_and_history_partitions_are_kept_ready_until_cancelled() {
    let db = TestDb::new().await;
    for table in ["outbox_event", "audit_log"] {
        db.exec_as_owner(&format!(
            "DO $$ BEGIN EXECUTE format('DROP TABLE {table}_%s', \
             to_char(date_trunc('month', now()) + interval '3 months', 'YYYY_MM')); END $$"
        ))
        .await;
    }
    let token = tokio_util::sync::CancellationToken::new();
    let keeper = tokio::spawn(worker::keep_partitions(
        db.app_pool.clone(),
        token.clone(),
        Duration::from_secs(3600),
    ));
    let mut created = false;
    for _ in 0..100 {
        let found: bool = sqlx::query_scalar(
            "SELECT to_regclass('outbox_event_' || \
             to_char(date_trunc('month', now()) + interval '3 months', 'YYYY_MM')) IS NOT NULL \
             AND to_regclass('audit_log_' || \
             to_char(date_trunc('month', now()) + interval '3 months', 'YYYY_MM')) IS NOT NULL",
        )
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
        if found {
            created = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(created, "the missing months are created at start");
    token.cancel();
    tokio::time::timeout(Duration::from_secs(2), keeper)
        .await
        .expect("stops when cancelled, without waiting for the next check")
        .unwrap();
}
