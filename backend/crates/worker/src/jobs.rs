//! Runs the jobs of the queue (`infra::jobs`).
//!
//! Each claimed job runs in a transaction of its tenant (row-level security
//! applies, as in the API). On success the transaction commits and the job
//! is removed; on failure the transaction rolls back and the job is retried
//! with an exponential backoff (`domain::jobs`), or fails for good.
//!
//! A job runs at least once: a worker that dies after a job's side effect
//! (an email sent) but before recording it lets another worker run it again
//! once the lease expires. Handlers keep that in mind.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use domain::jobs::{AfterFailure, after_failure};
use infra::email::{EMAIL_JOB, EmailJob, MailError, Mailer, render};
use infra::erasure::{ERASURE_JOB, ErasedMember};
use infra::exports::{EXPORT_JOB, ExportJob, ExportStore};
use infra::jobs::ClaimedJob;
use infra::platform::PURGE_TENANT_JOB;
use infra::sqlx::postgres::{PgConnection, PgPool};

use crate::Batch;

/// Why a job failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// Worth another attempt later (server unreachable…).
    Retry(String),
    /// Will never succeed (unknown kind, unreadable payload): no retry.
    Permanent(String),
}

/// What a handler returns.
pub type JobFuture<'a> = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send + 'a>>;

/// Does the work of the jobs. `conn` is in the transaction of the job's
/// tenant.
pub trait JobHandler: Send + Sync {
    fn handle<'a>(&'a self, job: &'a ClaimedJob, conn: &'a mut PgConnection) -> JobFuture<'a>;
}

/// The jobs of the product.
#[derive(Debug, Clone)]
pub struct Jobs {
    pub mailer: Mailer,
    /// Exports of personal data, in Valkey.
    pub exports: ExportStore,
}

fn payload<T: serde::de::DeserializeOwned>(job: &ClaimedJob) -> Result<T, JobError> {
    serde_json::from_value(job.payload.clone())
        .map_err(|e| JobError::Permanent(format!("unreadable payload: {e}")))
}

impl JobHandler for Jobs {
    fn handle<'a>(&'a self, job: &'a ClaimedJob, conn: &'a mut PgConnection) -> JobFuture<'a> {
        Box::pin(async move {
            match job.kind.as_str() {
                EMAIL_JOB => {
                    let email: EmailJob = serde_json::from_value(job.payload.clone())
                        .map_err(|e| JobError::Permanent(format!("unreadable payload: {e}")))?;
                    let rendered = render(&email.message, &email.locale);
                    self.mailer.send(&email.to, &rendered).await.map_err(|e| match e {
                        // The SMTP server may come back.
                        MailError::Send(_) => JobError::Retry(e.to_string()),
                        MailError::Url(_) | MailError::Address(_) | MailError::Build(_) => {
                            JobError::Permanent(e.to_string())
                        }
                    })
                }
                PURGE_TENANT_JOB => {
                    let removed = infra::platform::purge(conn, job.tenant_id)
                        .await
                        .map_err(|e| JobError::Retry(e.to_string()))?;
                    tracing::info!(removed, "tenant purged");
                    Ok(())
                }
                EXPORT_JOB => {
                    let export: ExportJob = payload(job)?;
                    // The data of that account only (`app_my_personal_data`).
                    infra::db::set_account(conn, export.account_id)
                        .await
                        .map_err(|e| JobError::Retry(e.to_string()))?;
                    let Some(content) = infra::exports::collect(conn)
                        .await
                        .map_err(|e| JobError::Retry(e.to_string()))?
                    else {
                        return Err(JobError::Permanent("the account has no data (erased)".to_owned()));
                    };
                    let stored = self
                        .exports
                        .complete(&export.export_id, content)
                        .await
                        .map_err(|e| JobError::Retry(e.to_string()))?;
                    tracing::info!(target: "audit", account = %export.account_id, stored, "personal data exported");
                    Ok(())
                }
                ERASURE_JOB => {
                    let member: ErasedMember = payload(job)?;
                    let done = infra::erasure::anonymize(conn, &member, &infra::erasure::steps())
                        .await
                        .map_err(|e| JobError::Retry(e.to_string()))?;
                    for (step, changed) in done {
                        tracing::info!(
                            target: "audit",
                            account = %member.account_id,
                            pseudonym = member.pseudonym_number,
                            step,
                            changed,
                            "erased member's content anonymized"
                        );
                    }
                    Ok(())
                }
                other => Err(JobError::Permanent(format!("unknown job kind {other}"))),
            }
        })
    }
}

/// Claims and runs batches of jobs.
#[derive(Debug, Clone)]
pub struct JobRunner {
    pool: PgPool,
    worker_id: String,
    batch_size: u32,
    lease: Duration,
}

impl JobRunner {
    /// A runner with a unique worker id, claiming 10 jobs at a time with a
    /// 5-minute lease.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            worker_id: format!("worker-{}", infra::id::new_id().simple()),
            batch_size: 10,
            lease: Duration::from_secs(5 * 60),
        }
    }

    #[must_use]
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// Claims due jobs and runs them with `handler`. `Idle` when there was
    /// nothing to do, or the database is unreachable (logged).
    pub async fn next_batch(&self, handler: &dyn JobHandler) -> Batch {
        let jobs = match infra::jobs::claim(&self.pool, &self.worker_id, self.batch_size, self.lease).await {
            Ok(jobs) => jobs,
            Err(err) => {
                tracing::warn!(error = %err, "cannot claim jobs");
                return Batch::Idle;
            }
        };
        if jobs.is_empty() {
            return Batch::Idle;
        }
        for job in jobs {
            self.run(handler, job).await;
        }
        Batch::Processed
    }

    async fn run(&self, handler: &dyn JobHandler, job: ClaimedJob) {
        let span = tracing::info_span!("job", id = %job.id, kind = %job.kind, tenant = %job.tenant_id, attempt = job.attempt);
        let _entered = span.enter();
        let result = match infra::db::begin_tenant_tx(&self.pool, job.tenant_id).await {
            Ok(mut tx) => match handler.handle(&job, &mut tx).await {
                Ok(()) => tx.commit().await.map_err(|e| JobError::Retry(e.to_string())),
                Err(err) => Err(err),
            },
            Err(err) => Err(JobError::Retry(err.to_string())),
        };
        let recorded = match &result {
            Ok(()) => {
                tracing::info!("job done");
                infra::jobs::complete(&self.pool, job.id, &self.worker_id).await
            }
            Err(JobError::Permanent(reason)) => {
                tracing::error!(%reason, "job failed for good");
                infra::jobs::fail(&self.pool, job.id, &self.worker_id, reason, None).await
            }
            Err(JobError::Retry(reason)) => match after_failure(job.attempt, job.max_attempts) {
                AfterFailure::RetryIn(delay) => {
                    tracing::warn!(%reason, retry_in_secs = delay.as_secs(), "job failed, will retry");
                    let retry_at =
                        Utc::now() + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::hours(1));
                    infra::jobs::fail(&self.pool, job.id, &self.worker_id, reason, Some(retry_at)).await
                }
                AfterFailure::GiveUp => {
                    tracing::error!(%reason, "job failed on its last attempt");
                    infra::jobs::fail(&self.pool, job.id, &self.worker_id, reason, None).await
                }
            },
        };
        match recorded {
            Ok(true) => {}
            // The purge removed the tenant, its jobs included.
            Ok(false) if result.is_ok() && job.kind == PURGE_TENANT_JOB => {}
            Ok(false) => tracing::warn!("lease lost: another worker may run the job again"),
            Err(err) => tracing::warn!(error = %err, "cannot record the job's outcome"),
        }
    }
}
