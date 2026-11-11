//! The signed-in member's own account (L02-US08, specification §3, §14):
//! preferences, password and sessions. Two-step verification is in
//! `crate::mfa`.
//!
//! The account is global: its preferences and password apply in every
//! tenant, and its sessions are listed and revoked in every tenant.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use domain::account::{
    MAX_FAILED_SIGN_INS_PER_EMAIL, Preferences, SIGN_IN_WINDOW, validate_password, validate_preferences,
};
use domain::validation::FieldError;
use infra::accounts;
use infra::credentials;
use infra::sessions::{ListedSession, SessionStore};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::extract::{ApiJson, ApiPath};
use crate::me::TAG;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// The account's preferences; `null` means the tenant's default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AccountPreferences)]
pub struct PreferencesBody {
    /// `fr` or `en`; the tenant's default language when `null`.
    #[serde(default)]
    #[schema(example = "en")]
    locale: Option<String>,
    /// IANA time zone in which instants are shown; the tenant's when `null`.
    #[serde(default)]
    #[schema(example = "Europe/Paris")]
    time_zone: Option<String>,
}

/// `POST /me/password` body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordChange {
    current_password: String,
    /// 12 to 128 characters, different from the email.
    new_password: String,
}

/// A session of the signed-in account.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AccountSession)]
pub struct SessionView {
    /// To revoke it.
    #[schema(example = "3f2a9c41d07b5e68a1c4f9e2b7d05a13")]
    id: String,
    /// The session of this request.
    current: bool,
    tenant_id: Uuid,
    /// `null` when the account left that tenant since.
    #[schema(example = "Acme")]
    tenant_name: Option<String>,
    /// Guessed from the `User-Agent`; `null` when unknown.
    #[schema(example = "Firefox")]
    browser: Option<String>,
    #[schema(example = "Linux")]
    os: Option<String>,
    /// Sign-in.
    created_at: DateTime<Utc>,
    /// Latest request, to the minute.
    last_seen_at: DateTime<Utc>,
    /// The session ends at this time at the latest.
    expires_at: DateTime<Utc>,
}

/// The list of sessions: the current one first, then by latest activity.
fn session_views(
    sessions: Vec<ListedSession>,
    current_id: &str,
    tenant_names: &HashMap<Uuid, String>,
) -> Vec<SessionView> {
    let mut views: Vec<SessionView> = sessions
        .into_iter()
        .map(|listed| {
            let session = listed.session;
            let device = session
                .user_agent
                .as_deref()
                .map(domain::devices::device)
                .unwrap_or_default();
            SessionView {
                current: listed.id == current_id,
                id: listed.id,
                tenant_id: session.tenant_id,
                tenant_name: tenant_names.get(&session.tenant_id).cloned(),
                browser: device.browser.map(str::to_owned),
                os: device.os.map(str::to_owned),
                created_at: session.created_at,
                last_seen_at: session.last_seen_at.unwrap_or(session.created_at),
                expires_at: session.expires_at,
            }
        })
        .collect();
    views.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then(b.last_seen_at.cmp(&a.last_seen_at))
            .then(a.id.cmp(&b.id))
    });
    views
}

/// `PUT /api/v1/me/preferences`.
#[utoipa::path(
    put,
    path = "/me/preferences",
    tag = TAG,
    operation_id = "updateMyPreferences",
    summary = "Set the language and time zone of the signed-in account",
    description = "Both apply in every tenant of the account. `null` (or absent) means the \
                   tenant's default.",
    request_body = PreferencesBody,
    responses(
        (status = 200, description = "The preferences saved", body = PreferencesBody),
        (status = 400, description = "`validation_failed`: `unsupported_locale`, `unknown_time_zone`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: an external account, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_preferences(
    mut tx: TenantTx,
    ApiJson(body): ApiJson<PreferencesBody>,
) -> Result<ApiJson<PreferencesBody>, Problem> {
    let preferences = Preferences {
        locale: body.locale.clone(),
        time_zone: body.time_zone.clone(),
    };
    let mut errors = validate_preferences(&preferences).err().unwrap_or_default();
    if let Some(time_zone) = &preferences.time_zone
        && !errors.iter().any(|e| e.field == "timeZone")
        && !accounts::time_zone_exists(&mut tx, time_zone).await?
    {
        errors.push(FieldError::new("timeZone", "unknown_time_zone"));
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let account = tx.account_id();
    accounts::set_preferences(&mut tx, account, &preferences).await?;
    tx.commit().await?;
    Ok(ApiJson(body))
}

/// `POST /api/v1/me/password`.
#[utoipa::path(
    post,
    path = "/me/password",
    tag = TAG,
    operation_id = "changeMyPassword",
    summary = "Change the password of the signed-in account",
    description = "Needs the current password. Ends every other session of the account, in every \
                   tenant; this one stays open. Unusable by an account without a local password \
                   (SSO only).",
    request_body = PasswordChange,
    responses(
        (status = 204, description = "Changed; the other sessions are ended"),
        (status = 400, description = "`validation_failed`: `currentPassword` `incorrect`, or \
                                      `newPassword` `too_short`, `too_long`, `same_as_email`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`account.no_password`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`: too many incorrect passwords, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn change_password(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiJson(body): ApiJson<PasswordChange>,
) -> Result<StatusCode, Problem> {
    let auth = &state.auth;
    let account = tx.account_id();
    let failures_key = format!("password-change:{account}");
    if auth.limits.count(&failures_key).await? >= MAX_FAILED_SIGN_INS_PER_EMAIL {
        return Err(auth.too_many(&failures_key).await);
    }
    let hash = accounts::password_hash(&mut tx, account)
        .await?
        .ok_or(Problem::new(ErrorCode::AccountNoPassword))?;
    let current = body.current_password;
    let matches = tokio::task::spawn_blocking(move || credentials::verify_password(&current, &hash)).await?;
    if !matches {
        auth.limits.hit(&failures_key, SIGN_IN_WINDOW).await?;
        return Err(Problem::validation(vec![FieldError::new(
            "currentPassword",
            "incorrect",
        )]));
    }
    validate_password("newPassword", &body.new_password, &tx.member().email).map_err(Problem::validation)?;

    let password = body.new_password;
    let new_hash = tokio::task::spawn_blocking(move || credentials::hash_password(&password))
        .await?
        .map_err(|e| Problem::internal(&e))?;
    accounts::set_password(&mut tx, account, &new_hash).await?;
    accounts::revoke_password_resets(&mut tx, account).await?;
    // Before the commit: if it fails, the other sessions merely ended.
    let ended = auth.sessions.delete_others(account, tx.session_token()).await?;
    tx.commit().await?;
    auth.limits.clear(&failures_key).await?;
    tracing::info!(account = %account, sessions_ended = ended, "password changed");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/me/sessions`.
#[utoipa::path(
    get,
    path = "/me/sessions",
    tag = TAG,
    operation_id = "listMySessions",
    summary = "The open sessions of the signed-in account",
    description = "In every tenant of the account: the current one first, then by latest activity.",
    responses(
        (status = 200, description = "The sessions", body = Vec<SessionView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: an external account", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn sessions(
    State(state): State<AppState>,
    mut tx: TenantTx,
) -> Result<ApiJson<Vec<SessionView>>, Problem> {
    let tenant_names: HashMap<Uuid, String> = infra::memberships::mine(&mut tx)
        .await?
        .into_iter()
        .map(|m| (m.tenant_id, m.name))
        .collect();
    let current = SessionStore::id_of(tx.session_token());
    let listed = state.auth.sessions.list(tx.account_id()).await?;
    tx.commit().await?;
    Ok(ApiJson(session_views(listed, &current, &tenant_names)))
}

/// `DELETE /api/v1/me/sessions/{id}`.
#[utoipa::path(
    delete,
    path = "/me/sessions/{id}",
    tag = TAG,
    operation_id = "revokeMySession",
    summary = "End a session of the signed-in account",
    description = "The session is refused from its next request on. Not the session of this \
                   request: sign out instead.",
    params(("id" = String, Path, description = "`id` from the list of sessions")),
    responses(
        (status = 204, description = "Ended"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such session of this account", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`session.current`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn revoke_session(
    State(state): State<AppState>,
    tx: TenantTx,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, Problem> {
    if id == SessionStore::id_of(tx.session_token()) {
        return Err(Problem::new(ErrorCode::SessionCurrent));
    }
    let account = tx.account_id();
    if !state.auth.sessions.delete_by_id(account, &id).await? {
        return Err(Problem::new(ErrorCode::NotFound));
    }
    tx.commit().await?;
    tracing::info!(account = %account, "session revoked");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use infra::sessions::{Session, SessionStage};

    use super::*;

    fn listed(id: &str, tenant: Uuid, user_agent: Option<&str>, minutes_ago: Option<i64>) -> ListedSession {
        let created_at = DateTime::parse_from_rfc3339("2026-11-11T08:00:00Z")
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_default();
        let now = created_at + Duration::hours(2);
        ListedSession {
            id: id.to_owned(),
            session: Session {
                account_id: Uuid::nil(),
                tenant_id: tenant,
                csrf_token: "csrf".to_owned(),
                created_at,
                expires_at: created_at + Duration::days(7),
                stage: SessionStage::Complete,
                second_factor: false,
                sso: false,
                user_agent: user_agent.map(str::to_owned),
                last_seen_at: minutes_ago.map(|m| now - Duration::minutes(m)),
            },
        }
    }

    #[test]
    fn the_current_session_comes_first_then_the_latest_used() {
        let (acme, services, gone) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let names = HashMap::from([(acme, "Acme".to_owned()), (services, "Acme Services".to_owned())]);
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";
        let views = session_views(
            vec![
                listed("old", services, None, None),
                listed(
                    "phone",
                    acme,
                    Some("Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) Safari/604.1"),
                    Some(5),
                ),
                listed("this", acme, Some(firefox), Some(30)),
                listed("left", gone, Some("curl/8.11.0"), Some(1)),
            ],
            "this",
            &names,
        );
        let order: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(order, ["this", "left", "phone", "old"]);
        let this = &views[0];
        assert!(this.current);
        assert_eq!(this.tenant_name.as_deref(), Some("Acme"));
        assert_eq!(
            (this.browser.as_deref(), this.os.as_deref()),
            (Some("Firefox"), Some("Linux"))
        );
        assert!(views[1..].iter().all(|v| !v.current));
        // Left that tenant since; device unknown.
        assert_eq!(
            (views[1].tenant_name.as_deref(), views[1].browser.as_deref()),
            (None, None)
        );
        assert_eq!(
            (views[2].browser.as_deref(), views[2].os.as_deref()),
            (Some("Safari"), Some("iOS"))
        );
        // Stored before activity was recorded: its sign-in stands for it.
        assert_eq!(views[3].last_seen_at, views[3].created_at);
    }
}
