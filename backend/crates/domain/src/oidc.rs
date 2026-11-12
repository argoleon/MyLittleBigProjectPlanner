//! Rules of sign-in through an OIDC provider (specification §3, §14): the
//! configuration of a provider, and which email of an ID token identifies a
//! member.

use std::time::Duration;

use crate::account::normalize_email;
use crate::validation::{FieldError, Violations};

/// Time to come back from the provider after leaving for it.
pub const LOGIN_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// What a tenant admin sends to configure a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInput {
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    /// Required on creation; `None` on update keeps the stored one.
    pub client_secret: Option<String>,
    pub email_claim: String,
    pub enabled: bool,
}

/// Checks `input`. `allow_http`: plain-HTTP issuers, for development and
/// tests only; production issuers are HTTPS.
pub fn validate_provider(
    input: &ProviderInput,
    creating: bool,
    allow_http: bool,
) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let name = input.display_name.trim().chars().count();
    if name == 0 {
        v.add("displayName", "required");
    } else if name > 100 {
        v.add("displayName", "too_long");
    }
    if input.issuer.trim().is_empty() {
        v.add("issuer", "required");
    } else if !is_issuer_url(input.issuer.trim(), allow_http) {
        v.add("issuer", "format");
    }
    let client_id = input.client_id.trim().chars().count();
    if client_id == 0 {
        v.add("clientId", "required");
    } else if client_id > 500 {
        v.add("clientId", "too_long");
    }
    match &input.client_secret {
        None if creating => v.add("clientSecret", "required"),
        Some(secret) if secret.is_empty() => v.add("clientSecret", "required"),
        Some(secret) if secret.chars().count() > 2000 => v.add("clientSecret", "too_long"),
        _ => {}
    }
    let claim = &input.email_claim;
    if claim.is_empty()
        || claim.len() > 100
        || !claim
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'))
    {
        v.add("emailClaim", "format");
    }
    v.into_result()
}

/// An issuer: an absolute `https://` URL (or `http://` when allowed), with a
/// host, without query, fragment or credentials.
fn is_issuer_url(issuer: &str, allow_http: bool) -> bool {
    let rest = if let Some(rest) = issuer.strip_prefix("https://") {
        rest
    } else if let Some(rest) = issuer.strip_prefix("http://").filter(|_| allow_http) {
        rest
    } else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    issuer.len() <= 2000 && !host.is_empty() && !host.contains('@') && !issuer.contains(['?', '#', ' '])
}

/// Why an ID token identifies no member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailRefusal {
    /// The claim is missing, or not an email.
    Missing,
    /// The provider does not vouch for the email (`email_verified` is not true).
    Unverified,
}

/// The email of an ID token to match against the tenant's members.
///
/// With the standard `email` claim, the provider must vouch for it
/// (`email_verified: true`). Another claim (`upn`, `preferred_username` of a
/// corporate directory) is trusted as is: the tenant admin chose it because
/// the directory controls it.
pub fn matching_email(
    email_claim: &str,
    value: Option<&str>,
    email_verified: Option<bool>,
) -> Result<String, EmailRefusal> {
    let email = value.and_then(normalize_email).ok_or(EmailRefusal::Missing)?;
    if email_claim == "email" && email_verified != Some(true) {
        return Err(EmailRefusal::Unverified);
    }
    Ok(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ProviderInput {
        ProviderInput {
            display_name: "Entra ID".to_owned(),
            issuer: "https://login.microsoftonline.com/0000/v2.0".to_owned(),
            client_id: "client".to_owned(),
            client_secret: Some("secret".to_owned()),
            email_claim: "email".to_owned(),
            enabled: true,
        }
    }

    fn errors(result: Result<(), Vec<FieldError>>) -> Vec<(String, &'static str)> {
        result
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn a_provider_needs_a_name_an_https_issuer_a_client_and_its_secret() {
        assert_eq!(validate_provider(&input(), true, false), Ok(()));
        let bad = ProviderInput {
            display_name: " ".to_owned(),
            issuer: "http://idp.example.org".to_owned(),
            client_id: String::new(),
            client_secret: None,
            email_claim: "e mail".to_owned(),
            enabled: true,
        };
        assert_eq!(
            errors(validate_provider(&bad, true, false)),
            [
                ("displayName".to_owned(), "required"),
                ("issuer".to_owned(), "format"),
                ("clientId".to_owned(), "required"),
                ("clientSecret".to_owned(), "required"),
                ("emailClaim".to_owned(), "format"),
            ]
        );
    }

    #[test]
    fn the_secret_may_be_omitted_on_update_only() {
        let keep = ProviderInput {
            client_secret: None,
            ..input()
        };
        assert_eq!(validate_provider(&keep, false, false), Ok(()));
        let emptied = ProviderInput {
            client_secret: Some(String::new()),
            ..input()
        };
        assert_eq!(
            errors(validate_provider(&emptied, false, false)),
            [("clientSecret".to_owned(), "required")]
        );
    }

    #[test]
    fn plain_http_issuers_are_for_development_only() {
        let local = ProviderInput {
            issuer: "http://keycloak:8080/realms/mlbpp".to_owned(),
            ..input()
        };
        assert!(validate_provider(&local, true, false).is_err());
        assert_eq!(validate_provider(&local, true, true), Ok(()));
        for bad in [
            "https://",
            "ftp://idp.example.org",
            "https://user:pw@idp.example.org",
            "https://idp.example.org/?tenant=1",
            "https://idp.example.org/#x",
            "idp.example.org",
        ] {
            let issuer = ProviderInput {
                issuer: bad.to_owned(),
                ..input()
            };
            assert!(validate_provider(&issuer, true, true).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_email_claim_must_be_verified() {
        assert_eq!(
            matching_email("email", Some("Lucas@Acme.example"), Some(true)),
            Ok("lucas@acme.example".to_owned())
        );
        assert_eq!(
            matching_email("email", Some("lucas@acme.example"), Some(false)),
            Err(EmailRefusal::Unverified)
        );
        assert_eq!(
            matching_email("email", Some("lucas@acme.example"), None),
            Err(EmailRefusal::Unverified)
        );
        assert_eq!(
            matching_email("email", None, Some(true)),
            Err(EmailRefusal::Missing)
        );
        assert_eq!(
            matching_email("email", Some("not an email"), Some(true)),
            Err(EmailRefusal::Missing)
        );
    }

    #[test]
    fn a_directory_claim_chosen_by_the_admin_is_trusted() {
        assert_eq!(
            matching_email("upn", Some("lucas@acme.example"), None),
            Ok("lucas@acme.example".to_owned())
        );
    }
}
