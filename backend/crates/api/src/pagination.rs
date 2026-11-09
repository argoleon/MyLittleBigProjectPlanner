//! Cursor pagination of list endpoints (specification §15).
//!
//! `GET /api/v1/things?limit=50` answers `{"items": [...], "nextCursor": "…"}`;
//! the next page is `GET /api/v1/things?limit=50&cursor=…`. `nextCursor` is
//! `null` on the last page. Cursors are opaque to clients: their content may
//! change between versions.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::DateTime;
use infra::pagination::Keyset;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};

use crate::problem::{ErrorCode, Problem};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;

/// `limit` and `cursor` query parameters of a list endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PageParams {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

impl PageParams {
    /// Page size and starting position, validated.
    pub fn resolve(&self) -> Result<(usize, Option<Keyset>), Problem> {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(Problem::new(ErrorCode::InvalidQuery)
                .with_detail(format!("limit must be between 1 and {MAX_LIMIT}")));
        }
        let after = match self.cursor.as_deref() {
            None | Some("") => None,
            Some(cursor) => Some(decode_cursor(cursor).ok_or_else(|| {
                Problem::new(ErrorCode::InvalidQuery).with_detail("cursor is invalid or expired")
            })?),
        };
        Ok((limit, after))
    }
}

/// A page as returned to clients.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageBody<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

const VERSION: &str = "v1";

pub fn encode_cursor(keyset: Keyset) -> String {
    let raw = format!(
        "{VERSION}:{}:{}",
        keyset.created_at.timestamp_micros(),
        keyset.id.simple()
    );
    URL_SAFE_NO_PAD.encode(raw)
}

pub fn decode_cursor(cursor: &str) -> Option<Keyset> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let raw = String::from_utf8(bytes).ok()?;
    let mut parts = raw.split(':');
    if parts.next()? != VERSION {
        return None;
    }
    let micros: i64 = parts.next()?.parse().ok()?;
    let id = Uuid::try_parse(parts.next()?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Keyset {
        created_at: DateTime::from_timestamp_micros(micros)?,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyset() -> Keyset {
        Keyset {
            created_at: DateTime::from_timestamp_micros(1_762_646_400_123_456).unwrap(),
            id: Uuid::from_u128(0x0194_0000_0000_7000_8000_0000_0000_0001),
        }
    }

    #[test]
    fn cursor_round_trips_with_microsecond_precision() {
        let cursor = encode_cursor(keyset());
        assert!(
            cursor
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(decode_cursor(&cursor), Some(keyset()));
    }

    #[test]
    fn garbage_and_tampered_cursors_are_rejected() {
        for bad in [
            "not base64!",
            "",
            &URL_SAFE_NO_PAD.encode("v2:1:00000000000000000000000000000001"),
            &URL_SAFE_NO_PAD.encode("v1:abc:00000000000000000000000000000001"),
            &URL_SAFE_NO_PAD.encode("v1:1:not-a-uuid"),
            &URL_SAFE_NO_PAD.encode("v1:1:00000000000000000000000000000001:extra"),
        ] {
            assert_eq!(decode_cursor(bad), None, "{bad}");
        }
    }

    #[test]
    fn limit_defaults_and_bounds() {
        assert_eq!(PageParams::default().resolve().unwrap().0, DEFAULT_LIMIT);
        for limit in [0, MAX_LIMIT + 1] {
            let params = PageParams {
                limit: Some(limit),
                cursor: None,
            };
            assert_eq!(params.resolve().unwrap_err().code, ErrorCode::InvalidQuery);
        }
    }

    #[test]
    fn invalid_cursor_is_an_invalid_query() {
        let params = PageParams {
            limit: None,
            cursor: Some("bogus".to_owned()),
        };
        assert_eq!(params.resolve().unwrap_err().code, ErrorCode::InvalidQuery);
    }
}
