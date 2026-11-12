//! GDPR erasure by anonymization (specification §14, L02-US09).
//!
//! An erased member appears as "Deleted user #N" wherever they appear; their
//! content stays, unless their comments are purged too. Who may erase what:
//!
//! - a tenant admin erases a member of their tenant ([`Scope::Tenant`]): the
//!   global account and the other tenants are untouched;
//! - the account itself, or a platform super admin, erases the global
//!   account ([`Scope::Account`]), in every tenant.

use std::time::Duration;

/// What an erasure covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// One tenant's membership.
    Tenant,
    /// The global account, in every tenant.
    Account,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tenant => "tenant",
            Self::Account => "account",
        }
    }
}

/// Who asks for an erasure, as far as erasure is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The account to erase, signed in.
    Itself,
    /// A tenant admin of the tenant (members management).
    TenantAdmin,
    /// A platform super admin.
    PlatformAdmin,
}

/// Whether `actor` may erase with `scope`.
pub const fn may_erase(actor: Actor, scope: Scope) -> bool {
    matches!(
        (actor, scope),
        (Actor::TenantAdmin, Scope::Tenant) | (Actor::Itself | Actor::PlatformAdmin, Scope::Account)
    )
}

/// The name an erased member appears under.
pub fn pseudonym(number: i64) -> String {
    format!("Deleted user #{number}")
}

/// How recent a sign-in through the organization's identity provider must be
/// to erase an account without a password.
pub const RECENT_SSO_SIGN_IN: Duration = Duration::from_secs(10 * 60);

/// How the account proves, again, that it is itself before erasing itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reauthentication {
    /// It types its password.
    Password,
    /// Its session comes from a sign-in through the identity provider within
    /// [`RECENT_SSO_SIGN_IN`]: enough.
    RecentSso,
    /// No password, and no recent sign-in through the identity provider: it
    /// signs in again first.
    SignInAgain,
}

/// What an account with (or without) a password, whose session was opened
/// through SSO (or not) `session_age` ago, must do to erase itself.
pub fn reauthentication(has_password: bool, sso_session: bool, session_age: Duration) -> Reauthentication {
    if has_password {
        Reauthentication::Password
    } else if sso_session && session_age <= RECENT_SSO_SIGN_IN {
        Reauthentication::RecentSso
    } else {
        Reauthentication::SignInAgain
    }
}

/// Whether `typed` confirms the erasure of the account of `email`: the
/// email, whatever the case and the surrounding spaces.
pub fn confirms(typed: &str, email: &str) -> bool {
    !typed.trim().is_empty() && typed.trim().to_lowercase() == email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_actor_erases_its_own_scope_only() {
        assert!(may_erase(Actor::TenantAdmin, Scope::Tenant));
        assert!(
            !may_erase(Actor::TenantAdmin, Scope::Account),
            "the global account is not the tenant's"
        );
        assert!(may_erase(Actor::Itself, Scope::Account));
        assert!(
            !may_erase(Actor::Itself, Scope::Tenant),
            "a member leaves through their account"
        );
        assert!(may_erase(Actor::PlatformAdmin, Scope::Account));
        assert!(!may_erase(Actor::PlatformAdmin, Scope::Tenant));
        assert_eq!(
            (Scope::Tenant.as_str(), Scope::Account.as_str()),
            ("tenant", "account")
        );
    }

    #[test]
    fn pseudonyms_carry_their_number() {
        assert_eq!(pseudonym(1234), "Deleted user #1234");
        assert_eq!(pseudonym(1), "Deleted user #1");
    }

    #[test]
    fn a_password_is_asked_when_there_is_one_else_a_recent_sso_sign_in() {
        let minutes = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(
            reauthentication(true, true, minutes(1)),
            Reauthentication::Password
        );
        assert_eq!(
            reauthentication(true, false, minutes(500)),
            Reauthentication::Password
        );
        assert_eq!(
            reauthentication(false, true, minutes(9)),
            Reauthentication::RecentSso
        );
        assert_eq!(
            reauthentication(false, true, minutes(10)),
            Reauthentication::RecentSso
        );
        assert_eq!(
            reauthentication(false, true, minutes(11)),
            Reauthentication::SignInAgain
        );
        assert_eq!(
            reauthentication(false, false, minutes(0)),
            Reauthentication::SignInAgain
        );
    }

    #[test]
    fn the_email_confirms_whatever_its_case() {
        assert!(confirms(
            "Lucas.Bernard@acme.example ",
            "lucas.bernard@acme.example"
        ));
        assert!(!confirms(
            "lucas.bernard@acme.exampl",
            "lucas.bernard@acme.example"
        ));
        assert!(!confirms("  ", ""));
    }
}
