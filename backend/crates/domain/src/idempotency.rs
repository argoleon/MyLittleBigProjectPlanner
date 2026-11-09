//! Rules of the `Idempotency-Key` header (specification §15).

use std::time::Duration;

/// How long a key protects against duplicates.
pub const KEY_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

pub const KEY_MAX_LEN: usize = 255;

/// A key is 1 to 255 visible ASCII characters (a UUID is typical).
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= KEY_MAX_LEN && key.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether a key stored with `expires_at` no longer protects anything at
/// `now` (both in seconds since the Unix epoch); an expired key can be reused.
pub fn is_expired(expires_at_secs: i64, now_secs: i64) -> bool {
    now_secs >= expires_at_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format() {
        assert!(is_valid_key("8b4f1c1e-6a0c-4a0e-9f55-0d5c2b8f9e11"));
        assert!(is_valid_key(&"k".repeat(KEY_MAX_LEN)));
        assert!(!is_valid_key(""));
        assert!(!is_valid_key(&"k".repeat(KEY_MAX_LEN + 1)));
        assert!(!is_valid_key("has space"));
        assert!(!is_valid_key("clé"));
    }

    #[test]
    fn expiry_boundary() {
        assert!(!is_expired(1_000, 999));
        assert!(is_expired(1_000, 1_000));
        assert!(is_expired(1_000, 5_000));
    }

    #[test]
    fn lifetime_is_one_day() {
        assert_eq!(KEY_LIFETIME.as_secs(), 86_400);
    }
}
