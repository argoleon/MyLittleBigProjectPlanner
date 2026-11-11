//! Integration tests on a real PostgreSQL (and Valkey for exports): erasure
//! of a global account or of a tenant's member, the anonymization steps,
//! the export of personal data (L02-US09).

#![allow(clippy::unwrap_used)]

use chrono::{Duration, Utc};
use domain::members::{Access, GroupInput};
use infra::accounts::{self, AccountType, NewMember, TenantRole};
use infra::db::{begin_tenant_tx, set_account};
use infra::erasure::{self, AccountErasure, ErasedMember, ErasureStep, StepFuture};
use infra::exports::ExportStore;
use infra::id::new_id;
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb, TestMember};

/// A transaction of `tenant` on behalf of `account` (`app.account_id`), as
/// the API opens them.
async fn tx_as(
    db: &TestDb,
    tenant: &SeedTenant,
    account: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    set_account(&mut tx, account).await.unwrap();
    tx
}

async fn count(db: &TestDb, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

/// Makes the account of `email` a member of `tenant` too.
async fn join(db: &TestDb, tenant: &SeedTenant, email: &str) {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    accounts::add_member(
        &mut tx,
        &NewMember {
            email,
            display_name: "Test Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// A member of alpha and beta, in a group of each, with a password reset
/// pending, an invitation to their email elsewhere and an invitation they
/// sent; each tenant has another admin.
async fn member_with_data(db: &TestDb) -> TestMember {
    for tenant in [&db.tenant_a, &db.tenant_b] {
        db.add_member(tenant, TenantRole::TenantAdmin).await;
    }
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;
    join(db, &db.tenant_b, &member.email).await;
    for tenant in [&db.tenant_a, &db.tenant_b] {
        let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
        let group = infra::groups::insert(
            &mut tx,
            new_id(),
            &GroupInput {
                name: "Platform".to_owned(),
                description: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        infra::groups::add_member(&mut tx, group.id, member.account_id)
            .await
            .unwrap();
        accounts::create_password_reset(
            &mut tx,
            member.account_id,
            &[1; 32],
            Utc::now() + Duration::hours(1),
        )
        .await
        .unwrap();
        infra::invitations::insert(
            &mut tx,
            &infra::invitations::NewInvitation {
                id: new_id(),
                email: &format!("guest-{}@example.test", new_id().simple()),
                access: Access {
                    account_type: AccountType::Internal,
                    tenant_role: TenantRole::Member,
                },
                token_hash: &new_id().into_bytes(),
                expires_at: Utc::now() + Duration::days(7),
                invited_by: member.account_id,
            },
        )
        .await
        .unwrap()
        .unwrap();
        tx.commit().await.unwrap();
    }
    member
}

#[tokio::test]
async fn erasing_the_global_account_leaves_no_personal_data_and_a_pseudonym_in_every_tenant() {
    let db = TestDb::new().await;
    let member = member_with_data(&db).await;
    let id = member.account_id;

    let mut tx = tx_as(&db, &db.tenant_a, id).await;
    let erased = erasure::erase_account(&mut tx, id, id, true).await.unwrap();
    tx.commit().await.unwrap();
    let AccountErasure::Erased(number) = erased else {
        panic!("{erased:?}")
    };

    // The account row stays for the references to it, anonymized.
    let row: (String, String, Option<String>, Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT email, display_name, locale, time_zone, password_hash, pseudonym_number FROM account WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&db.owner_pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            format!("erased-{number}@erased.invalid"),
            domain::erasure::pseudonym(number),
            None,
            None,
            None,
            Some(number)
        )
    );
    for (table, column) in [
        ("membership", "account_id"),
        ("group_member", "account_id"),
        ("password_reset", "account_id"),
        ("account_mfa", "account_id"),
        ("account_backup_code", "account_id"),
        ("invitation", "invited_by"),
    ] {
        let left = count(
            &db,
            &format!("SELECT count(*) FROM {table} WHERE {column} = '{id}'"),
        )
        .await;
        // Invitations it sent are left to the jobs' step.
        let expected = i64::from(table == "invitation") * 2;
        assert_eq!(left, expected, "{table}");
    }
    assert_eq!(
        count(
            &db,
            &format!("SELECT count(*) FROM account WHERE email = '{}'", member.email)
        )
        .await,
        0,
        "the email is gone"
    );
    // Same pseudonym in both tenants, and an anonymization job in each.
    let pseudonyms = count(
        &db,
        &format!("SELECT count(*) FROM erased_member WHERE account_id = '{id}' AND pseudonym_number = {number} AND scope = 'account' AND comments_purged"),
    )
    .await;
    assert_eq!(pseudonyms, 2);
    let jobs: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT tenant_id, payload FROM job WHERE kind = 'gdpr.anonymize_member' ORDER BY tenant_id",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    let mut tenants: Vec<Uuid> = jobs.iter().map(|(tenant, _)| *tenant).collect();
    tenants.sort();
    let mut expected = vec![db.tenant_a.id, db.tenant_b.id];
    expected.sort();
    assert_eq!(tenants, expected);
    let payload: ErasedMember = serde_json::from_value(jobs[0].1.clone()).unwrap();
    assert_eq!(
        payload,
        ErasedMember {
            account_id: id,
            pseudonym_number: number,
            purge_comments: true
        }
    );

    // Erased once: nothing left to erase.
    let mut tx = tx_as(&db, &db.tenant_a, id).await;
    assert_eq!(
        erasure::erase_account(&mut tx, id, id, false).await.unwrap(),
        AccountErasure::NotFound
    );
}

#[tokio::test]
async fn a_tenant_erasure_leaves_the_global_account_and_the_other_tenants_alone() {
    let db = TestDb::new().await;
    let member = member_with_data(&db).await;
    let admin = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    let id = member.account_id;

    let mut tx = tx_as(&db, &db.tenant_a, admin.account_id).await;
    let number = erasure::erase_in_tenant(&mut tx, admin.account_id, id, &member.email, false)
        .await
        .unwrap();
    infra::members::remove(&mut tx, id, None).await.unwrap().unwrap();
    assert_eq!(erasure::pseudonym_of(&mut tx, id).await.unwrap(), Some(number));
    tx.commit().await.unwrap();

    let (email, name): (String, String) =
        sqlx::query_as("SELECT email, display_name FROM account WHERE id = $1")
            .bind(id)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    assert_eq!(
        (email, name),
        (member.email.clone(), "Test Member".to_owned()),
        "global account untouched"
    );
    let in_tenant = |tenant: Uuid, table: &str| {
        format!("SELECT count(*) FROM {table} WHERE tenant_id = '{tenant}' AND account_id = '{id}'")
    };
    assert_eq!(count(&db, &in_tenant(db.tenant_a.id, "membership")).await, 0);
    assert_eq!(count(&db, &in_tenant(db.tenant_a.id, "group_member")).await, 0);
    assert_eq!(
        count(&db, &in_tenant(db.tenant_b.id, "membership")).await,
        1,
        "beta keeps its member"
    );
    assert_eq!(count(&db, &in_tenant(db.tenant_b.id, "group_member")).await, 1);
    assert_eq!(count(&db, &in_tenant(db.tenant_b.id, "erased_member")).await, 0);
    let jobs = |tenant: Uuid| {
        format!("SELECT count(*) FROM job WHERE kind = 'gdpr.anonymize_member' AND tenant_id = '{tenant}'")
    };
    assert_eq!(
        (
            count(&db, &jobs(db.tenant_a.id)).await,
            count(&db, &jobs(db.tenant_b.id)).await
        ),
        (1, 0)
    );

    // Beta knows no pseudonym for its member.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(erasure::pseudonym_of(&mut tx, id).await.unwrap(), None);
}

#[tokio::test]
async fn only_the_account_itself_or_a_super_admin_erases_it_and_never_the_last_admin() {
    let db = TestDb::new().await;
    let member = member_with_data(&db).await;
    let other = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;

    // A tenant admin is no platform super admin; nor can the actor be
    // another account than the signed-in one.
    let mut tx = tx_as(&db, &db.tenant_a, other.account_id).await;
    assert!(
        erasure::erase_account(&mut tx, other.account_id, member.account_id, false)
            .await
            .is_err()
    );
    let mut tx = tx_as(&db, &db.tenant_a, other.account_id).await;
    assert!(
        erasure::erase_account(&mut tx, member.account_id, member.account_id, false)
            .await
            .is_err()
    );

    // The only admin of a tenant stays.
    let lonely = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    accounts::add_member(
        &mut tx,
        &NewMember {
            email: &lonely.email,
            display_name: "Test Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::TenantAdmin,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query(
        "DELETE FROM membership WHERE tenant_id = $1 AND account_id <> $2 AND tenant_role = 'tenant_admin'",
    )
    .bind(db.tenant_b.id)
    .bind(lonely.account_id)
    .execute(&db.owner_pool)
    .await
    .unwrap();
    let mut tx = tx_as(&db, &db.tenant_a, lonely.account_id).await;
    assert_eq!(
        erasure::erase_account(&mut tx, lonely.account_id, lonely.account_id, false)
            .await
            .unwrap(),
        AccountErasure::LastAdmin
    );

    // A platform super admin: another one removes the role first.
    db.make_platform_admin(other.account_id).await;
    let mut tx = tx_as(&db, &db.tenant_a, other.account_id).await;
    assert_eq!(
        erasure::erase_account(&mut tx, other.account_id, other.account_id, false)
            .await
            .unwrap(),
        AccountErasure::PlatformAdmin
    );
    // It erases others.
    let mut tx = tx_as(&db, &db.tenant_a, other.account_id).await;
    assert!(matches!(
        erasure::erase_account(&mut tx, other.account_id, member.account_id, false)
            .await
            .unwrap(),
        AccountErasure::Erased(_)
    ));
}

/// Comments of a later work package, for the step below.
struct TestComments;

impl ErasureStep for TestComments {
    fn name(&self) -> &'static str {
        "test_comment"
    }

    fn run<'a>(&'a self, conn: &'a mut PgConnection, member: &'a ErasedMember) -> StepFuture<'a> {
        Box::pin(async move {
            let done = if member.purge_comments {
                sqlx::query("DELETE FROM test_comment WHERE author_id = $1")
                    .bind(member.account_id)
                    .execute(conn)
                    .await?
            } else {
                sqlx::query("UPDATE test_comment SET signature = NULL WHERE author_id = $1")
                    .bind(member.account_id)
                    .execute(conn)
                    .await?
            };
            Ok(done.rows_affected())
        })
    }
}

#[tokio::test]
async fn steps_anonymize_or_purge_the_content_of_the_tenant_only() {
    let db = TestDb::new().await;
    db.exec_as_owner(
        "CREATE TABLE test_comment (id uuid PRIMARY KEY, tenant_id uuid NOT NULL, author_id uuid NOT NULL,
                                    body text NOT NULL, signature text);
         ALTER TABLE test_comment ENABLE ROW LEVEL SECURITY;
         ALTER TABLE test_comment FORCE ROW LEVEL SECURITY;
         CREATE POLICY tenant_isolation ON test_comment
             USING (tenant_id = app_current_tenant()) WITH CHECK (tenant_id = app_current_tenant());
         CREATE POLICY test_comment_platform ON test_comment TO mlbpp_owner USING (true) WITH CHECK (true);
         GRANT SELECT, INSERT, UPDATE, DELETE ON test_comment TO mlbpp_app;",
    )
    .await;
    let member = member_with_data(&db).await;
    let id = member.account_id;
    for tenant in [db.tenant_a.id, db.tenant_b.id] {
        for _ in 0..2 {
            sqlx::query("INSERT INTO test_comment VALUES ($1, $2, $3, 'Looks good', 'Lucas')")
                .bind(new_id())
                .bind(tenant)
                .bind(id)
                .execute(&db.owner_pool)
                .await
                .unwrap();
        }
    }
    let steps: Vec<Box<dyn ErasureStep>> = vec![Box::new(TestComments), erasure::steps().remove(0)];
    let comments = |tenant: Uuid, condition: &str| {
        format!("SELECT count(*) FROM test_comment WHERE tenant_id = '{tenant}' AND {condition}")
    };

    // Kept, anonymized, in alpha only; the invitation alpha sent no longer names the member.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let kept = ErasedMember {
        account_id: id,
        pseudonym_number: 7,
        purge_comments: false,
    };
    let done = erasure::anonymize(&mut tx, &kept, &steps).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(done, [("test_comment", 2), ("invitations_sent", 1)]);
    assert_eq!(
        count(&db, &comments(db.tenant_a.id, "signature IS NULL")).await,
        2
    );
    assert_eq!(
        count(&db, &comments(db.tenant_b.id, "signature IS NOT NULL")).await,
        2,
        "beta untouched"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT count(*) FROM invitation WHERE invited_by = '{id}'")
        )
        .await,
        1,
        "beta's invitation untouched"
    );

    // Purged, in beta; a second run changes nothing.
    let purged = ErasedMember {
        purge_comments: true,
        ..kept
    };
    for expected in [2, 0] {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
        let done = erasure::anonymize(&mut tx, &purged, &steps).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(done[0], ("test_comment", expected));
    }
    assert_eq!(count(&db, &comments(db.tenant_b.id, "true")).await, 0);
    assert_eq!(
        count(&db, &comments(db.tenant_a.id, "true")).await,
        2,
        "alpha keeps its comments"
    );
}

#[tokio::test]
async fn the_export_holds_the_accounts_data_only_and_expires() {
    let db = TestDb::new().await;
    let member = member_with_data(&db).await;
    let (valkey, prefix) = testkit::valkey().await;
    let store = ExportStore::new(valkey, prefix);

    let (id, opened) = store.open(member.account_id).await.unwrap();
    assert_eq!(opened.content, None);
    let left = opened.expires_at - Utc::now();
    assert!(
        left > Duration::minutes(14) && left <= Duration::minutes(15),
        "{left}"
    );

    // Collected for the account set in the transaction.
    let mut tx = tx_as(&db, &db.tenant_a, member.account_id).await;
    let data = infra::exports::collect(&mut tx).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(data["format"], "mlbpp.personal-data.v1");
    assert_eq!(data["account"]["email"], member.email.as_str());
    assert_eq!(data["account"]["hasPassword"], true);
    assert_eq!(data["twoStepVerification"]["enabled"], false);
    let memberships = data["memberships"].as_array().unwrap();
    assert_eq!(memberships.len(), 2, "every tenant");
    for membership in memberships {
        assert_eq!(membership["groups"], serde_json::json!(["Platform"]));
    }
    assert!(!data.to_string().contains("password_hash"));

    assert!(store.complete(&id, data.clone()).await.unwrap());
    let ready = store.get(&id).await.unwrap().unwrap();
    assert_eq!((ready.account_id, ready.content), (member.account_id, Some(data)));
    assert_eq!(ready.expires_at, opened.expires_at);
    assert!(store.get("another-id").await.unwrap().is_none());
    assert!(!store.complete("another-id", serde_json::json!({})).await.unwrap());

    // Without account set, nothing.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(infra::exports::collect(&mut tx).await.unwrap().is_none());
}

#[tokio::test]
async fn purging_a_tenant_keeps_an_account_another_tenant_shows_under_a_pseudonym() {
    let db = TestDb::new().await;
    let super_admin = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    db.make_platform_admin(super_admin.account_id).await;
    db.add_member(&db.tenant_b, TenantRole::TenantAdmin).await;
    let member = db.add_member(&db.tenant_b, TenantRole::Member).await;
    join(&db, &db.tenant_a, &member.email).await;

    // Alpha erases it; then beta, its only tenant left, is purged.
    let mut tx = tx_as(&db, &db.tenant_a, super_admin.account_id).await;
    erasure::erase_in_tenant(
        &mut tx,
        super_admin.account_id,
        member.account_id,
        &member.email,
        false,
    )
    .await
    .unwrap();
    infra::members::remove(&mut tx, member.account_id, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut conn = db.app_pool.acquire().await.unwrap();
    infra::platform::delete(&mut conn, super_admin.account_id, db.tenant_b.id, None, new_id())
        .await
        .unwrap()
        .unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    infra::platform::purge(&mut tx, db.tenant_b.id).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count(
            &db,
            &format!("SELECT count(*) FROM account WHERE id = '{}'", member.account_id)
        )
        .await,
        1,
        "alpha still shows it as Deleted user #N"
    );
}
