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

fn jobs() -> Jobs {
    let smtp = infra::config::Secret::new(testkit::smtp_url());
    Jobs {
        mailer: Mailer::new(&smtp, "Planner <noreply@example.org>").unwrap(),
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
    assert_eq!(runner.next_batch(&jobs()).await, Batch::Processed);
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

    JobRunner::new(db.app_pool.clone()).next_batch(&jobs()).await;
    for id in [unknown, unreadable] {
        let (status, attempts, error, _) = job_row(&db, id).await.unwrap();
        assert_eq!((status.as_str(), attempts), ("failed", 1), "{error:?}");
    }
}
