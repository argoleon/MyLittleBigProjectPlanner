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

/// What the session being left proved about its member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Proof {
    /// A TOTP or backup code was checked (two-step verification).
    pub second_factor: bool,
    /// The member signed in through an identity provider.
    pub sso: bool,
}

/// The member switching to another tenant, as that tenant sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    /// The target tenant's policy.
    pub policy: Policy,
    pub local_account: bool,
    /// The target tenant has an enabled identity provider.
    pub tenant_has_sso: bool,
    /// The account has two-step verification (global to the account).
    pub account_mfa_enabled: bool,
    pub proof: Proof,
}

/// How a switch ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// The target requires its own identity provider: sign in there.
    SignInThroughProvider,
    /// A session at this stage opens in the target tenant.
    Session(crate::mfa::SessionStage),
}

/// The target tenant's policy applied to a member coming from another
/// tenant's session (only internal members switch).
pub fn after_switch(switch: Switch) -> SwitchOutcome {
    use crate::mfa::SessionStage;
    let password_proof = PasswordSignIn {
        internal: true,
        local_account: switch.local_account,
        tenant_has_sso: switch.tenant_has_sso,
    };
    if !password_allowed(switch.policy, password_proof) {
        // Even after another tenant's SSO: this tenant trusts its own provider.
        return SwitchOutcome::SignInThroughProvider;
    }
    let stage = if switch.account_mfa_enabled && !switch.proof.second_factor {
        SessionStage::MfaChallenge
    } else if !switch.account_mfa_enabled && switch.policy.local_accounts_require_mfa {
        SessionStage::MfaEnrollment
    } else {
        SessionStage::Complete
    };
    SwitchOutcome::Session(stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mfa::SessionStage;

    fn switch() -> Switch {
        Switch {
            policy: Policy::default(),
            local_account: false,
            tenant_has_sso: false,
            account_mfa_enabled: false,
            proof: Proof::default(),
        }
    }

    #[test]
    fn a_switch_without_requirements_opens_a_complete_session() {
        assert_eq!(
            after_switch(switch()),
            SwitchOutcome::Session(SessionStage::Complete)
        );
    }

    #[test]
    fn a_target_requiring_its_sso_asks_to_sign_in_there() {
        let policy = Policy {
            internal_accounts_require_sso: true,
            ..Policy::default()
        };
        let target = Switch {
            policy,
            tenant_has_sso: true,
            proof: Proof {
                second_factor: true,
                sso: true,
            },
            ..switch()
        };
        assert_eq!(
            after_switch(target),
            SwitchOutcome::SignInThroughProvider,
            "another tenant's SSO is not enough"
        );
        assert_eq!(
            after_switch(Switch {
                local_account: true,
                ..target
            }),
            SwitchOutcome::Session(SessionStage::Complete),
            "a local account keeps its way in"
        );
        assert_eq!(
            after_switch(Switch {
                tenant_has_sso: false,
                ..target
            }),
            SwitchOutcome::Session(SessionStage::Complete),
            "no provider: nobody is locked out"
        );
    }

    #[test]
    fn two_step_verification_is_asked_unless_the_session_proved_it() {
        let with_mfa = Switch {
            account_mfa_enabled: true,
            ..switch()
        };
        assert_eq!(
            after_switch(with_mfa),
            SwitchOutcome::Session(SessionStage::MfaChallenge)
        );
        assert_eq!(
            after_switch(Switch {
                proof: Proof {
                    second_factor: true,
                    sso: false
                },
                ..with_mfa
            }),
            SwitchOutcome::Session(SessionStage::Complete)
        );
    }

    #[test]
    fn a_target_requiring_mfa_makes_members_without_it_enroll() {
        let target = Switch {
            policy: Policy {
                local_accounts_require_mfa: true,
                ..Policy::default()
            },
            ..switch()
        };
        assert_eq!(
            after_switch(target),
            SwitchOutcome::Session(SessionStage::MfaEnrollment)
        );
    }

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
