//! Authentication policy of the current tenant (specification §3), set by
//! its tenant admin. One row per tenant, created with it. Rules in
//! `domain::auth_policy`.

use sqlx::postgres::PgConnection;

/// The policy of the current tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthPolicy {
    pub settings: PolicySettings,
    pub version: i64,
}

/// What the tenant admin sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PolicySettings {
    /// Members signing in with a password must use two-step verification.
    pub local_accounts_require_mfa: bool,
    /// Internal members sign in through an OIDC provider, except local accounts.
    pub internal_accounts_require_sso: bool,
    /// Sign-in methods of the request portal.
    pub portal_password_enabled: bool,
    pub portal_magic_link_enabled: bool,
}

impl AuthPolicy {
    /// The rules' view of it.
    pub fn rules(&self) -> domain::auth_policy::Policy {
        domain::auth_policy::Policy {
            local_accounts_require_mfa: self.settings.local_accounts_require_mfa,
            internal_accounts_require_sso: self.settings.internal_accounts_require_sso,
        }
    }
}

struct Row {
    local_accounts_require_mfa: bool,
    internal_accounts_require_sso: bool,
    portal_password_enabled: bool,
    portal_magic_link_enabled: bool,
    version: i64,
}

impl From<Row> for AuthPolicy {
    fn from(row: Row) -> Self {
        Self {
            settings: PolicySettings {
                local_accounts_require_mfa: row.local_accounts_require_mfa,
                internal_accounts_require_sso: row.internal_accounts_require_sso,
                portal_password_enabled: row.portal_password_enabled,
                portal_magic_link_enabled: row.portal_magic_link_enabled,
            },
            version: row.version,
        }
    }
}

/// The policy of the current tenant.
pub async fn get(conn: &mut PgConnection) -> Result<AuthPolicy, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT local_accounts_require_mfa, internal_accounts_require_sso,
               portal_password_enabled, portal_magic_link_enabled, version
        FROM tenant_auth_policy
        "#
    )
    .fetch_one(conn)
    .await?;
    Ok(row.into())
}

/// Replaces the policy if its version is one of `expected_versions` (`None`:
/// any). `None` when the version did not match.
pub async fn set(
    conn: &mut PgConnection,
    settings: &PolicySettings,
    expected_versions: Option<&[i64]>,
) -> Result<Option<AuthPolicy>, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        r#"
        UPDATE tenant_auth_policy
        SET local_accounts_require_mfa = $1, internal_accounts_require_sso = $2,
            portal_password_enabled = $3, portal_magic_link_enabled = $4, version = version + 1
        WHERE $5::bigint[] IS NULL OR version = ANY($5)
        RETURNING local_accounts_require_mfa, internal_accounts_require_sso,
                  portal_password_enabled, portal_magic_link_enabled, version
        "#,
        settings.local_accounts_require_mfa,
        settings.internal_accounts_require_sso,
        settings.portal_password_enabled,
        settings.portal_magic_link_enabled,
        expected_versions
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(AuthPolicy::from))
}
