//! Integration tests on a real PostgreSQL: groups and invitations under
//! row-level security (L02-US06).

#![allow(clippy::unwrap_used)]

use chrono::{Duration, Utc};
use domain::members::{Access, GroupInput, InvitationStatus};
use infra::accounts::{AccountType, TenantRole};
use infra::db::begin_tenant_tx;
use infra::groups::{self, MemberChange, WriteConflict};
use infra::id::new_id;
use infra::invitations::{self, InvitationFilter, NewInvitation};
use testkit::TestDb;

fn group(name: &str) -> GroupInput {
    GroupInput {
        name: name.to_owned(),
        description: None,
    }
}

const MEMBER: Access = Access {
    account_type: AccountType::Internal,
    tenant_role: TenantRole::Member,
};

#[tokio::test]
async fn groups_are_isolated_and_named_once_per_tenant() {
    let db = TestDb::new().await;
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await;

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let platform = groups::insert(&mut tx, new_id(), &group("Platform"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        groups::add_member(&mut tx, platform.id, member.account_id)
            .await
            .unwrap(),
        MemberChange::Done
    );
    tx.commit().await.unwrap();
    // The violation aborts the transaction: the caller answers 409 and rolls back.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        groups::insert(&mut tx, new_id(), &group("platform"))
            .await
            .unwrap(),
        Err(WriteConflict::NameTaken)
    );
    drop(tx);

    // Beta sees neither the group nor the member, and may reuse the name.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(groups::find(&mut tx, platform.id).await.unwrap(), None);
    assert!(groups::list(&mut tx, None, 10).await.unwrap().items.is_empty());
    assert!(groups::members(&mut tx, platform.id).await.unwrap().is_empty());
    assert!(
        groups::of_members(&mut tx, &[member.account_id])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        groups::add_member(&mut tx, platform.id, member.account_id)
            .await
            .unwrap(),
        MemberChange::NoGroup
    );
    let beta_group = groups::insert(&mut tx, new_id(), &group("Platform"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        groups::add_member(&mut tx, beta_group.id, member.account_id)
            .await
            .unwrap(),
        MemberChange::NoMember
    );
    assert_eq!(
        groups::delete(&mut tx, platform.id, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    tx.commit().await.unwrap();

    // Even written directly, a row cannot tie a tenant's group to another's member.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let crossed = infra::sqlx::query(
        "INSERT INTO group_member (id, tenant_id, group_id, account_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(new_id())
    .bind(db.tenant_b.id)
    .bind(beta_group.id)
    .bind(member.account_id)
    .execute(&mut *tx)
    .await;
    assert!(crossed.is_err());
}

#[tokio::test]
async fn invitations_are_isolated_and_open_once_per_email() {
    let db = TestDb::new().await;
    let admin = db.add_member(&db.tenant_a, TenantRole::TenantAdmin).await;
    let hash = infra::credentials::token_hash("token-1");
    let new = |id, email: &'static str, token_hash: &'static [u8]| NewInvitation {
        id,
        email,
        access: MEMBER,
        token_hash,
        expires_at: Utc::now() + Duration::days(7),
        invited_by: admin.account_id,
    };
    let hash: &'static [u8] = Box::leak(hash.into_boxed_slice());
    let other: &'static [u8] = Box::leak(infra::credentials::token_hash("token-2").into_boxed_slice());

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let invitation = invitations::insert(&mut tx, &new(new_id(), "p.durand@client.example", hash))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invitation.status(Utc::now()), InvitationStatus::Pending);
    assert_eq!(invitation.invited_by_name.as_deref(), Some("Test Member"));
    assert_eq!(
        invitations::find_by_token(&mut tx, hash)
            .await
            .unwrap()
            .map(|i| i.id),
        Some(invitation.id)
    );
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(
        invitations::insert(&mut tx, &new(new_id(), "p.durand@client.example", other))
            .await
            .unwrap(),
        Err(invitations::WriteConflict::AlreadyInvited)
    );
    drop(tx);

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(invitations::find(&mut tx, invitation.id).await.unwrap(), None);
    assert_eq!(invitations::find_by_token(&mut tx, hash).await.unwrap(), None);
    assert!(
        invitations::list_open(&mut tx, &InvitationFilter::default(), None, 10)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        invitations::revoke(&mut tx, invitation.id, None).await.unwrap(),
        Err(invitations::WriteConflict::NotFound)
    );
    // The same email can be invited by another tenant.
    invitations::insert(&mut tx, &new(new_id(), "p.durand@client.example", other))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    // Accepted once; then closed.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(invitations::mark_accepted(&mut tx, invitation.id).await.unwrap());
    assert!(!invitations::mark_accepted(&mut tx, invitation.id).await.unwrap());
    assert_eq!(invitations::find_by_token(&mut tx, hash).await.unwrap(), None);
    assert_eq!(
        invitations::revoke(&mut tx, invitation.id, None).await.unwrap(),
        Err(invitations::WriteConflict::Closed)
    );
    let accepted = invitations::find(&mut tx, invitation.id).await.unwrap().unwrap();
    assert_eq!(accepted.status(Utc::now()), InvitationStatus::Accepted);
}
