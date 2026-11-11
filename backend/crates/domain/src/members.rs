//! Rules of a tenant's members, invitations and groups (specification §3,
//! §14, L02-US06).

use std::time::Duration;

use crate::account::{AccountType, TenantRole, normalize_email};
use crate::validation::{FieldError, Violations};

/// Lifetime of an invitation link.
pub const INVITATION_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Longest group name, in characters.
pub const GROUP_NAME_MAX_CHARS: usize = 100;

/// Longest group description, in characters.
pub const GROUP_DESCRIPTION_MAX_CHARS: usize = 1000;

/// Whether a member with `role` manages the tenant's members, invitations
/// and groups: tenant admins only, not configuration admins (§3).
pub const fn manages_members(role: TenantRole) -> bool {
    matches!(role, TenantRole::TenantAdmin)
}

/// What an admin sets on a member: account type and tenant role. An
/// external account is never an administrator (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub account_type: AccountType,
    pub tenant_role: TenantRole,
}

/// Checks `access`; field errors on `tenantRole`.
pub fn validate_access(access: Access) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if access.account_type == AccountType::External && access.tenant_role != TenantRole::Member {
        v.add("tenantRole", "external_member_only");
    }
    v.into_result()
}

/// An invitation to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationInput {
    pub email: String,
    pub access: Access,
}

/// Checks `input`; the normalized email when it is valid.
pub fn validate_invitation(input: &InvitationInput) -> Result<String, Vec<FieldError>> {
    let mut v = Violations::new();
    let email = normalize_email(&input.email);
    if input.email.trim().is_empty() {
        v.add("email", "required");
    } else if email.is_none() {
        v.add("email", "email");
    }
    if let Err(errors) = validate_access(input.access) {
        for e in errors {
            v.add(e.field, e.code);
        }
    }
    v.into_result()?;
    // Checked above: no violation means a valid email.
    email.ok_or_else(|| vec![FieldError::new("email", "email")])
}

/// Where an invitation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitationStatus {
    /// Sent, waiting for the invitee.
    Pending,
    /// Not accepted in time: the admin can send it again.
    Expired,
    Accepted,
    Revoked,
}

impl InvitationStatus {
    /// The status of an invitation stored as `stored` (`pending`, `accepted`
    /// or `revoked`), expiring at `expires_at`, at `now` (Unix seconds).
    pub fn at(stored: &str, expires_at: i64, now: i64) -> Self {
        match stored {
            "accepted" => Self::Accepted,
            "revoked" => Self::Revoked,
            _ if now >= expires_at => Self::Expired,
            _ => Self::Pending,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Expired => "expired",
            Self::Accepted => "accepted",
            Self::Revoked => "revoked",
        }
    }
}

/// A group of members, to grant roles to several people at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInput {
    pub name: String,
    pub description: Option<String>,
}

/// Checks `input`; the name is compared trimmed.
pub fn validate_group(input: &GroupInput) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let name = input.name.trim().chars().count();
    if name == 0 {
        v.add("name", "required");
    } else if name > GROUP_NAME_MAX_CHARS {
        v.add("name", "too_long");
    }
    if input
        .description
        .as_deref()
        .is_some_and(|d| d.chars().count() > GROUP_DESCRIPTION_MAX_CHARS)
    {
        v.add("description", "too_long");
    }
    v.into_result()
}

/// Whether a tenant still has an active tenant admin once a member who is
/// one (`was_admin`) stops being one (`stays_admin` false: demoted, made
/// external or removed). `other_admins`: the other active tenant admins.
/// Nobody could administer the tenant any more otherwise.
pub const fn keeps_an_admin(was_admin: bool, stays_admin: bool, other_admins: u64) -> bool {
    !was_admin || stays_admin || other_admins > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERNAL_ADMIN: Access = Access {
        account_type: AccountType::Internal,
        tenant_role: TenantRole::TenantAdmin,
    };

    #[test]
    fn only_tenant_admins_manage_members() {
        assert!(manages_members(TenantRole::TenantAdmin));
        assert!(!manages_members(TenantRole::ConfigurationAdmin));
        assert!(!manages_members(TenantRole::Member));
    }

    #[test]
    fn an_external_account_is_a_plain_member() {
        for role in [
            TenantRole::Member,
            TenantRole::ConfigurationAdmin,
            TenantRole::TenantAdmin,
        ] {
            let internal = Access {
                account_type: AccountType::Internal,
                tenant_role: role,
            };
            assert_eq!(validate_access(internal), Ok(()));
            let external = Access {
                account_type: AccountType::External,
                tenant_role: role,
            };
            assert_eq!(
                validate_access(external).is_ok(),
                role == TenantRole::Member,
                "{role:?}"
            );
        }
        assert_eq!(
            validate_access(Access {
                account_type: AccountType::External,
                tenant_role: TenantRole::TenantAdmin,
            }),
            Err(vec![FieldError::new("tenantRole", "external_member_only")])
        );
    }

    #[test]
    fn an_invitation_needs_a_valid_email_and_returns_it_normalized() {
        let input = |email: &str, access| InvitationInput {
            email: email.to_owned(),
            access,
        };
        assert_eq!(
            validate_invitation(&input("  P.Durand@Client.Example ", INTERNAL_ADMIN)),
            Ok("p.durand@client.example".to_owned())
        );
        assert_eq!(
            validate_invitation(&input(" ", INTERNAL_ADMIN)),
            Err(vec![FieldError::new("email", "required")])
        );
        let external_admin = Access {
            account_type: AccountType::External,
            tenant_role: TenantRole::TenantAdmin,
        };
        assert_eq!(
            validate_invitation(&input("not an email", external_admin)),
            Err(vec![
                FieldError::new("email", "email"),
                FieldError::new("tenantRole", "external_member_only"),
            ])
        );
    }

    #[test]
    fn an_invitation_expires_at_its_deadline() {
        let deadline = 1_700_000_000;
        assert_eq!(
            InvitationStatus::at("pending", deadline, deadline - 1),
            InvitationStatus::Pending
        );
        assert_eq!(
            InvitationStatus::at("pending", deadline, deadline),
            InvitationStatus::Expired
        );
        assert_eq!(
            InvitationStatus::at("pending", deadline, deadline + 1),
            InvitationStatus::Expired
        );
        // Accepted or revoked whatever the time.
        assert_eq!(
            InvitationStatus::at("accepted", deadline, deadline + 1),
            InvitationStatus::Accepted
        );
        assert_eq!(
            InvitationStatus::at("revoked", deadline, deadline - 1),
            InvitationStatus::Revoked
        );
        assert_eq!(INVITATION_LIFETIME.as_secs(), 7 * 86_400);
        assert_eq!(InvitationStatus::Expired.as_str(), "expired");
    }

    #[test]
    fn a_group_has_a_name_and_an_optional_short_description() {
        let group = |name: &str, description: Option<String>| GroupInput {
            name: name.to_owned(),
            description,
        };
        assert_eq!(validate_group(&group("Platform", None)), Ok(()));
        assert_eq!(validate_group(&group("Platform", Some("x".repeat(1000)))), Ok(()));
        assert_eq!(
            validate_group(&group("  ", Some("x".repeat(1001)))),
            Err(vec![
                FieldError::new("name", "required"),
                FieldError::new("description", "too_long"),
            ])
        );
        assert_eq!(
            validate_group(&group(&"é".repeat(101), None)),
            Err(vec![FieldError::new("name", "too_long")])
        );
    }

    #[test]
    fn the_last_tenant_admin_stays_one() {
        // Demoting or removing the only admin.
        assert!(!keeps_an_admin(true, false, 0));
        // Another admin remains, or the member stays admin, or never was one.
        assert!(keeps_an_admin(true, false, 1));
        assert!(keeps_an_admin(true, true, 0));
        assert!(keeps_an_admin(false, false, 0));
    }
}
