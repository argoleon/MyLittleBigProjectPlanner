//! Erasure by anonymization (specification §14, L02-US09).
//!
//! - [`erase_account`] erases the global account, in every tenant
//!   (`app_erase_account`): the account itself or a platform super admin.
//! - [`erase_in_tenant`] records the erasure of a member of the current
//!   tenant, whose membership the caller removes in the same transaction.
//!
//! Both leave an `erased_member` row in each tenant concerned, with the
//! pseudonym "Deleted user #N", and queue an [`ERASURE_JOB`] there. The
//! worker runs the [`steps`] in the tenant's transaction: each table holding
//! a member's content (comments, history…) registers its own step, which
//! anonymizes it, or purges the comments when asked.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::id::new_id;

/// Job anonymizing an erased member's content in one tenant.
pub const ERASURE_JOB: &str = "gdpr.anonymize_member";

/// Payload of [`ERASURE_JOB`]: who was erased, under which pseudonym.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErasedMember {
    pub account_id: Uuid,
    pub pseudonym_number: i64,
    /// Delete the member's comments rather than keep them anonymized.
    pub purge_comments: bool,
}

/// What came of [`erase_account`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountErasure {
    /// Erased under this pseudonym number.
    Erased(i64),
    /// No such account, or erased already.
    NotFound,
    /// The account is the last active tenant admin of a tenant.
    LastAdmin,
    /// The account is a platform super admin: another one removes that
    /// role first.
    PlatformAdmin,
}

/// Erases the global account `account_id` for `actor`, the signed-in account
/// (`app.account_id`): itself, or a platform super admin. Refused by the
/// database otherwise (`insufficient_privilege`).
pub async fn erase_account(
    conn: &mut PgConnection,
    actor: Uuid,
    account_id: Uuid,
    purge_comments: bool,
) -> Result<AccountErasure, sqlx::Error> {
    let erased = sqlx::query_scalar!(
        r#"SELECT app_erase_account($1, $2, $3) AS "number""#,
        actor,
        account_id,
        purge_comments
    )
    .fetch_one(conn)
    .await;
    match erased {
        Ok(Some(number)) => Ok(AccountErasure::Erased(number)),
        Ok(None) => Ok(AccountErasure::NotFound),
        Err(sqlx::Error::Database(err)) if err.message() == "member.last_admin" => {
            Ok(AccountErasure::LastAdmin)
        }
        Err(sqlx::Error::Database(err)) if err.message() == "account.platform_admin" => {
            Ok(AccountErasure::PlatformAdmin)
        }
        Err(err) => Err(err),
    }
}

/// Records that `actor`, a tenant admin, erases the member `account_id`
/// (whose email is `email`) from the current tenant, and queues the
/// anonymization of their content there. Returns the pseudonym number. The
/// caller removes the membership in the same transaction.
pub async fn erase_in_tenant(
    conn: &mut PgConnection,
    actor: Uuid,
    account_id: Uuid,
    email: &str,
    purge_comments: bool,
) -> Result<i64, sqlx::Error> {
    let number = sqlx::query_scalar!(
        r#"
        INSERT INTO erased_member (id, tenant_id, account_id, pseudonym_number, scope, comments_purged, erased_by)
        VALUES ($1, app_current_tenant(), $2, nextval('pseudonym_number_seq'), 'tenant', $3, $4)
        ON CONFLICT (tenant_id, account_id) DO UPDATE
            SET comments_purged = erased_member.comments_purged OR EXCLUDED.comments_purged,
                erased_by = EXCLUDED.erased_by, erased_at = now(), version = erased_member.version + 1
        RETURNING pseudonym_number
        "#,
        new_id(),
        account_id,
        purge_comments,
        actor
    )
    .fetch_one(&mut *conn)
    .await?;
    // Personal data of the member held by this tenant.
    sqlx::query!("DELETE FROM invitation WHERE email = $1", email)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "DELETE FROM job WHERE kind = $1 AND status = 'queued' AND lower(payload ->> 'to') = $2",
        crate::email::EMAIL_JOB,
        email
    )
    .execute(&mut *conn)
    .await?;
    let member = ErasedMember {
        account_id,
        pseudonym_number: number,
        purge_comments,
    };
    let payload = serde_json::to_value(&member).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    crate::jobs::enqueue(conn, ERASURE_JOB, &payload).await?;
    Ok(number)
}

/// The pseudonym number of `account_id` in the current tenant, if it was
/// erased there: content shows "Deleted user #N" instead of its author.
pub async fn pseudonym_of(conn: &mut PgConnection, account_id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT pseudonym_number FROM erased_member WHERE account_id = $1",
        account_id
    )
    .fetch_optional(conn)
    .await
}

/// What a step returns: how many rows it changed.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<u64, sqlx::Error>> + Send + 'a>>;

/// Anonymization of one table's content of an erased member, in the
/// tenant's transaction. Steps run at least once: they must be idempotent.
pub trait ErasureStep: Send + Sync {
    /// Name in the logs.
    fn name(&self) -> &'static str;
    fn run<'a>(&'a self, conn: &'a mut PgConnection, member: &'a ErasedMember) -> StepFuture<'a>;
}

/// Invitations the member sent no longer name them.
struct InvitationsSent;

impl ErasureStep for InvitationsSent {
    fn name(&self) -> &'static str {
        "invitations_sent"
    }

    fn run<'a>(&'a self, conn: &'a mut PgConnection, member: &'a ErasedMember) -> StepFuture<'a> {
        Box::pin(async move {
            let done = sqlx::query!(
                "UPDATE invitation SET invited_by = NULL WHERE invited_by = $1",
                member.account_id
            )
            .execute(conn)
            .await?;
            Ok(done.rows_affected())
        })
    }
}

/// The steps of the product. Tables holding a member's content add theirs
/// here: comments (purged when `purge_comments`, else kept under the
/// pseudonym), history, attachments' uploader…
pub fn steps() -> Vec<Box<dyn ErasureStep>> {
    vec![Box::new(InvitationsSent)]
}

/// Runs `steps` for `member` in the current tenant: each step's name and the
/// rows it changed.
pub async fn anonymize(
    conn: &mut PgConnection,
    member: &ErasedMember,
    steps: &[Box<dyn ErasureStep>],
) -> Result<Vec<(&'static str, u64)>, sqlx::Error> {
    let mut done = Vec::with_capacity(steps.len());
    for step in steps {
        let changed = step.run(conn, member).await?;
        done.push((step.name(), changed));
    }
    Ok(done)
}
