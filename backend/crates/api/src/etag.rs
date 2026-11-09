//! `ETag`, `If-Match` and `If-None-Match` based on the resource `version`
//! (specification §15: lost-update protection).
//!
//! The ETag of a resource is its version in quotes: `"3"`. To modify or
//! delete, a client sends back the ETag it read in `If-Match`; if the
//! resource changed in the meantime, the server answers 412 instead of
//! overwriting someone else's change.

use axum::http::{HeaderMap, HeaderValue};

use crate::problem::{ErrorCode, Problem};

/// ETag header value of a version.
pub fn etag(version: i64) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{version}\"")).unwrap_or_else(|_| HeaderValue::from_static("\"0\""))
}

/// What a write request accepts to overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IfMatch {
    /// `If-Match: *`: any current version.
    Any,
    /// Only these versions.
    Versions(Vec<i64>),
}

impl IfMatch {
    /// Versions for a conditional SQL write: `None` means any.
    pub fn versions(&self) -> Option<&[i64]> {
        match self {
            Self::Any => None,
            Self::Versions(v) => Some(v),
        }
    }
}

/// Reads `If-Match`, which every update and delete must carry (428 otherwise).
/// Weak tags (`W/"3"`) and tags that are not versions never match (RFC 9110
/// strong comparison), which yields 412 if nothing else matches.
pub fn require_if_match(headers: &HeaderMap) -> Result<IfMatch, Problem> {
    let values: Vec<&str> = headers
        .get_all("if-match")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    if values.is_empty() {
        return Err(Problem::new(ErrorCode::PreconditionRequired));
    }
    let tags: Vec<&str> = values.iter().flat_map(|v| v.split(',')).map(str::trim).collect();
    if tags.contains(&"*") {
        return Ok(IfMatch::Any);
    }
    Ok(IfMatch::Versions(
        tags.iter().filter_map(|t| parse_strong(t)).collect(),
    ))
}

/// Whether `If-None-Match` matches `version` (the client's copy is current).
pub fn if_none_match(headers: &HeaderMap, version: i64) -> bool {
    headers
        .get_all("if-none-match")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|tag| tag == "*" || parse_weak_or_strong(tag) == Some(version))
}

fn parse_strong(tag: &str) -> Option<i64> {
    tag.strip_prefix('"')?.strip_suffix('"')?.parse().ok()
}

fn parse_weak_or_strong(tag: &str) -> Option<i64> {
    parse_strong(tag.strip_prefix("W/").unwrap_or(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(name: &'static str, value: &'static str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_static(value));
        h
    }

    #[test]
    fn etag_is_the_quoted_version() {
        assert_eq!(etag(3), "\"3\"");
    }

    #[test]
    fn missing_if_match_is_precondition_required() {
        assert_eq!(
            require_if_match(&HeaderMap::new()),
            Err(Problem::new(ErrorCode::PreconditionRequired))
        );
    }

    #[test]
    fn if_match_parses_lists_and_star() {
        assert_eq!(
            require_if_match(&headers("if-match", "\"2\", \"3\"")),
            Ok(IfMatch::Versions(vec![2, 3]))
        );
        assert_eq!(require_if_match(&headers("if-match", "*")), Ok(IfMatch::Any));
        assert_eq!(IfMatch::Any.versions(), None);
    }

    #[test]
    fn weak_or_foreign_tags_never_match_if_match() {
        assert_eq!(
            require_if_match(&headers("if-match", "W/\"3\", \"abc\", 3")),
            Ok(IfMatch::Versions(vec![]))
        );
    }

    #[test]
    fn if_none_match_uses_weak_comparison() {
        assert!(if_none_match(&headers("if-none-match", "\"3\""), 3));
        assert!(if_none_match(&headers("if-none-match", "W/\"3\""), 3));
        assert!(if_none_match(&headers("if-none-match", "*"), 3));
        assert!(!if_none_match(&headers("if-none-match", "\"2\""), 3));
        assert!(!if_none_match(&HeaderMap::new(), 3));
    }
}
