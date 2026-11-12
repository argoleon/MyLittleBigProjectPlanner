//! Password hashing (Argon2id) and random tokens (sessions, password resets).
//!
//! Hashing is CPU-heavy on purpose: call [`hash_password`] and
//! [`verify_password`] through `tokio::task::spawn_blocking` from async code.

use std::sync::OnceLock;

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Argon2id parameters recommended by OWASP: 19 MiB of memory, 2 passes,
/// 1 lane. Stored in each hash, so they can be raised later without
/// invalidating existing passwords.
const MEMORY_KIB: u32 = 19 * 1024;
const PASSES: u32 = 2;
const LANES: u32 = 1;

fn argon2() -> Argon2<'static> {
    // The constants above are valid parameters.
    let params = Params::new(MEMORY_KIB, PASSES, LANES, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Why a password could not be hashed (never expected with valid parameters).
#[derive(Debug, thiserror::Error)]
#[error("cannot hash the password: {0}")]
pub struct HashError(String);

/// Argon2id hash of `password`, as a PHC string with a random salt.
pub fn hash_password(password: &str) -> Result<String, HashError> {
    argon2()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| HashError(e.to_string()))
}

/// Whether `password` matches `hash`. A malformed hash never matches.
pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => argon2().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Spends the time of a password verification without any account: makes
/// "unknown email" as slow as "wrong password", so timing reveals nothing.
pub fn verify_against_nothing(password: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("dummy password, never matches").unwrap_or_default());
    let _ = verify_password(password, hash);
}

/// A random token of 256 bits, URL-safe (43 characters).
pub fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 of a token: what the database or Valkey stores instead of the
/// token itself.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// [`token_hash`] in hexadecimal, for Valkey keys.
pub fn token_hash_hex(token: &str) -> String {
    token_hash(token).iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison of two secrets (CSRF tokens).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_argon2id_with_the_owasp_parameters_and_a_random_salt() {
        let first = hash_password("correct horse battery").unwrap();
        let second = hash_password("correct horse battery").unwrap();
        assert!(first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{first}");
        assert_ne!(first, second, "each hash has its own salt");
    }

    #[test]
    fn verification_accepts_the_password_only() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("correct horse batterY", &hash));
        assert!(!verify_password("correct horse battery", "not a hash"));
        verify_against_nothing("anything");
    }

    #[test]
    fn tokens_are_random_and_url_safe() {
        let a = random_token();
        assert_eq!(a.len(), 43);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(a, random_token());
        assert_eq!(token_hash(&a).len(), 32);
        assert_eq!(token_hash_hex(&a).len(), 64);
        assert_eq!(token_hash(&a), token_hash(&a));
    }

    #[test]
    fn comparison_is_exact() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(constant_time_eq("", ""));
    }
}
