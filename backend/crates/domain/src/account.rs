//! Rules of accounts and local authentication (specification §3, §14).

use std::time::Duration;

use crate::validation::{FieldError, Violations};

/// Shortest password accepted (NIST SP 800-63B recommends length over
/// composition rules).
pub const PASSWORD_MIN_CHARS: usize = 12;

/// Longest password accepted: bounds the hashing work an attacker can cause.
pub const PASSWORD_MAX_CHARS: usize = 128;

/// Lifetime of a password reset link.
pub const PASSWORD_RESET_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Failed sign-ins on one email (in one tenant) before sign-in is refused for
/// [`SIGN_IN_WINDOW`].
pub const MAX_FAILED_SIGN_INS_PER_EMAIL: u32 = 5;

/// Sign-in attempts from one address in [`SIGN_IN_WINDOW`] before it is refused.
pub const MAX_SIGN_INS_PER_IP: u32 = 30;

/// Password reset emails sent for one email (in one tenant) in
/// [`SIGN_IN_WINDOW`]; further requests are accepted but send nothing.
pub const MAX_PASSWORD_RESETS_PER_EMAIL: u32 = 3;

/// Window of the sign-in rate limits.
pub const SIGN_IN_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Normalized form of an email: trimmed and lower case. `None` when it is
/// not plausibly an email (exactly one `@`, a non-empty local part, a domain
/// with a dot, no space, 254 characters at most).
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    if email.len() > 254 || email.chars().any(char::is_whitespace) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    let domain_ok = !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..");
    if local.is_empty() || !domain_ok {
        return None;
    }
    Some(email)
}

/// Checks a new password. Field errors are reported on `field`.
pub fn validate_password(field: &str, password: &str, email: &str) -> Result<(), Vec<FieldError>> {
    let mut violations = Violations::new();
    let chars = password.chars().count();
    if chars < PASSWORD_MIN_CHARS {
        violations.add(field, "too_short");
    } else if chars > PASSWORD_MAX_CHARS {
        violations.add(field, "too_long");
    } else if password.trim().to_lowercase() == email.trim().to_lowercase() {
        violations.add(field, "same_as_email");
    }
    violations.into_result()
}

/// Checks a display name: 1 to 200 characters once trimmed.
pub fn validate_display_name(field: &str, name: &str) -> Result<(), Vec<FieldError>> {
    let mut violations = Violations::new();
    let chars = name.trim().chars().count();
    if chars == 0 {
        violations.add(field, "required");
    } else if chars > 200 {
        violations.add(field, "too_long");
    }
    violations.into_result()
}

/// Kind of membership of an account in a tenant (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountType {
    /// Uses the internal application.
    Internal,
    /// Uses the request portal only: never reaches the internal application.
    External,
}

impl AccountType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "internal" => Some(Self::Internal),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

/// Role of a member in its tenant (§3, administration levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantRole {
    Member,
    /// Workflows, types and fields, without member management.
    ConfigurationAdmin,
    /// Members, roles, configurations, portals, authentication policy.
    TenantAdmin,
}

impl TenantRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::ConfigurationAdmin => "configuration_admin",
            Self::TenantAdmin => "tenant_admin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "member" => Some(Self::Member),
            "configuration_admin" => Some(Self::ConfigurationAdmin),
            "tenant_admin" => Some(Self::TenantAdmin),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_trimmed_and_lower_cased() {
        assert_eq!(
            normalize_email("  Thomas.Leroy@Prestataire.FR ").as_deref(),
            Some("thomas.leroy@prestataire.fr")
        );
        assert_eq!(normalize_email("a+tag@x.io").as_deref(), Some("a+tag@x.io"));
    }

    #[test]
    fn implausible_emails_are_rejected() {
        for bad in [
            "",
            "no-at-sign",
            "@example.org",
            "a@b@example.org",
            "a@localhost",
            "a@.example.org",
            "a@example.org.",
            "a@example..org",
            "a b@example.org",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        let long = format!("{}@example.org", "a".repeat(250));
        assert_eq!(normalize_email(&long), None);
    }

    #[test]
    fn passwords_need_a_length_and_differ_from_the_email() {
        assert!(validate_password("password", "correct horse battery", "a@b.io").is_ok());
        let errors = |p: &str| {
            validate_password("password", p, "lucas@acme.io")
                .unwrap_err()
                .into_iter()
                .map(|e| e.code)
                .collect::<Vec<_>>()
        };
        assert_eq!(errors("short"), ["too_short"]);
        assert_eq!(errors(&"x".repeat(129)), ["too_long"]);
        assert_eq!(errors("Lucas@Acme.io"), ["same_as_email"]);
        // Length counts characters, not bytes.
        assert!(validate_password("password", "éééééééééééé", "a@b.io").is_ok());
    }

    #[test]
    fn display_names_are_required_and_bounded() {
        assert!(validate_display_name("displayName", "Lucas Bernard").is_ok());
        assert_eq!(
            validate_display_name("displayName", "  ").unwrap_err()[0].code,
            "required"
        );
        assert_eq!(
            validate_display_name("displayName", &"x".repeat(201)).unwrap_err()[0].code,
            "too_long"
        );
    }

    #[test]
    fn types_and_roles_round_trip() {
        for t in [AccountType::Internal, AccountType::External] {
            assert_eq!(AccountType::parse(t.as_str()), Some(t));
        }
        for r in [
            TenantRole::Member,
            TenantRole::ConfigurationAdmin,
            TenantRole::TenantAdmin,
        ] {
            assert_eq!(TenantRole::parse(r.as_str()), Some(r));
        }
        assert_eq!(AccountType::parse("guest"), None);
        assert_eq!(TenantRole::parse("owner"), None);
    }
}
