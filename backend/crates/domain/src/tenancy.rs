//! Which tenant a request belongs to (specification §3).
//!
//! In single-tenant mode every request goes to the default tenant. In
//! multi-tenant mode the tenant is the first label of the host name:
//! `acme.example.org` → `acme` when the base domain is `example.org`.

/// Deployment mode, set by configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tenancy {
    /// One tenant, whatever the host name.
    Single { default_slug: String },
    /// One subdomain per tenant under `base_domain` (lowercase, no port).
    Multi { base_domain: String },
}

/// Why no tenant could be derived from the host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantHostError {
    /// No `Host` header (or URI authority) on the request.
    MissingHost,
    /// The host is not `<slug>.<base_domain>` with a valid slug.
    NotATenantHost,
}

/// Slug of the tenant addressed by `host`.
///
/// `host` is the raw `Host` header: it may carry a port, a trailing dot or
/// uppercase letters.
pub fn tenant_slug_from_host(tenancy: &Tenancy, host: Option<&str>) -> Result<String, TenantHostError> {
    match tenancy {
        Tenancy::Single { default_slug } => Ok(default_slug.clone()),
        Tenancy::Multi { base_domain } => {
            let host = normalize_host(host.ok_or(TenantHostError::MissingHost)?);
            let slug = host
                .strip_suffix(base_domain.as_str())
                .and_then(|rest| rest.strip_suffix('.'))
                .ok_or(TenantHostError::NotATenantHost)?;
            if is_valid_slug(slug) {
                Ok(slug.to_owned())
            } else {
                Err(TenantHostError::NotATenantHost)
            }
        }
    }
}

/// A tenant slug is a single DNS label: 1 to 63 characters among `a-z`,
/// `0-9` and `-`, not starting or ending with `-`.
pub fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-')
}

/// Lowercase host name without port or trailing dot.
fn normalize_host(host: &str) -> String {
    let host = host.trim();
    let without_port = if host.starts_with('[') {
        // IPv6 literal: never a tenant host, keep it whole so it fails to match.
        host
    } else {
        host.rsplit_once(':').map_or(host, |(name, _port)| name)
    };
    without_port.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi() -> Tenancy {
        Tenancy::Multi {
            base_domain: "example.org".to_owned(),
        }
    }

    #[test]
    fn single_mode_ignores_the_host() {
        let tenancy = Tenancy::Single {
            default_slug: "main".to_owned(),
        };
        assert_eq!(tenant_slug_from_host(&tenancy, None), Ok("main".to_owned()));
        assert_eq!(
            tenant_slug_from_host(&tenancy, Some("anything.test:8080")),
            Ok("main".to_owned())
        );
    }

    #[test]
    fn multi_mode_takes_the_first_label() {
        assert_eq!(
            tenant_slug_from_host(&multi(), Some("acme.example.org")),
            Ok("acme".to_owned())
        );
    }

    #[test]
    fn multi_mode_normalizes_port_case_and_trailing_dot() {
        for host in [
            "acme.example.org:8443",
            "ACME.Example.ORG",
            "acme.example.org.",
            " acme.example.org ",
        ] {
            assert_eq!(
                tenant_slug_from_host(&multi(), Some(host)),
                Ok("acme".to_owned()),
                "{host}"
            );
        }
    }

    #[test]
    fn multi_mode_rejects_hosts_outside_the_base_domain() {
        for host in [
            "example.org",
            "acme.other.org",
            "acmeexample.org",
            "a.b.example.org",
            "-acme.example.org",
            "ac_me.example.org",
            "[::1]:8080",
            "",
        ] {
            assert_eq!(
                tenant_slug_from_host(&multi(), Some(host)),
                Err(TenantHostError::NotATenantHost),
                "{host}"
            );
        }
    }

    #[test]
    fn multi_mode_requires_a_host() {
        assert_eq!(
            tenant_slug_from_host(&multi(), None),
            Err(TenantHostError::MissingHost)
        );
    }

    #[test]
    fn slug_rules() {
        assert!(is_valid_slug("a"));
        assert!(is_valid_slug("acme-2"));
        assert!(is_valid_slug(&"a".repeat(63)));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug(&"a".repeat(64)));
        assert!(!is_valid_slug("Acme"));
        assert!(!is_valid_slug("acme-"));
        assert!(!is_valid_slug("ac.me"));
    }
}
