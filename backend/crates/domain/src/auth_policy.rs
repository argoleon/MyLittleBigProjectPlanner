//! The authentication policy of a tenant (specification §3): which way each
//! member signs in. Set by the tenant admin.

/// The policy, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Policy {
    /// Password sign-ins need two-step verification.
    pub local_accounts_require_mfa: bool,
    /// Internal members sign in through an OIDC provider of the tenant,
    /// except those marked as local accounts.
    pub internal_accounts_require_sso: bool,
}

/// The member trying to sign in with a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordSignIn {
    pub internal: bool,
    /// Marked by the tenant admin as outside the organization's directory.
    pub local_account: bool,
    /// The tenant has at least one enabled OIDC provider.
    pub tenant_has_sso: bool,
}

/// Whether a password sign-in is allowed. SSO is only required when the
/// tenant has a provider to sign in with: a policy set before any provider
/// locks nobody out.
pub fn password_allowed(policy: Policy, who: PasswordSignIn) -> bool {
    !(policy.internal_accounts_require_sso && who.internal && !who.local_account && who.tenant_has_sso)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERNAL: PasswordSignIn = PasswordSignIn {
        internal: true,
        local_account: false,
        tenant_has_sso: true,
    };

    #[test]
    fn without_the_sso_rule_every_member_may_use_a_password() {
        let policy = Policy::default();
        assert!(password_allowed(policy, INTERNAL));
    }

    #[test]
    fn the_sso_rule_sends_internal_members_to_the_provider() {
        let policy = Policy {
            internal_accounts_require_sso: true,
            ..Policy::default()
        };
        assert!(!password_allowed(policy, INTERNAL));
        // A local account (a contractor) keeps its password.
        assert!(password_allowed(
            policy,
            PasswordSignIn {
                local_account: true,
                ..INTERNAL
            }
        ));
        // External members use the portal, whose methods the portal enforces.
        assert!(password_allowed(
            policy,
            PasswordSignIn {
                internal: false,
                ..INTERNAL
            }
        ));
        // No provider to sign in with: nobody is locked out.
        assert!(password_allowed(
            policy,
            PasswordSignIn {
                tenant_has_sso: false,
                ..INTERNAL
            }
        ));
    }
}
