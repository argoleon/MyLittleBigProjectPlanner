//! Two-step verification (specification §14): TOTP secrets encrypted at
//! rest, codes checked against them, single-use backup codes.
//!
//! Rules (time window, replay, code formats) are in `domain::mfa`; this
//! module computes the codes and stores the state, always in a tenant
//! transaction (row-level security shows the current tenant's members only).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use domain::mfa::{BACKUP_CODE_ALPHABET, BACKUP_CODE_CHARS, BACKUP_CODE_COUNT, TOTP_DIGITS, TOTP_STEP_SECS};
use sqlx::postgres::PgConnection;
use totp_rs::{Algorithm, Builder, Totp};
use uuid::Uuid;

use crate::config::Secret;
use crate::credentials::{constant_time_eq, token_hash};
use crate::id::new_id;

/// Name shown by authenticator apps next to the account.
pub const ISSUER: &str = domain::PRODUCT_NAME;

/// Bytes of a TOTP secret (160 bits, as RFC 4226 recommends).
const SECRET_BYTES: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum MfaError {
    #[error("MFA_ENCRYPTION_KEY must be 32 bytes encoded in base64")]
    Key,
    #[error("cannot encrypt or decrypt a TOTP secret")]
    Cipher,
    #[error("invalid TOTP secret: {0}")]
    Totp(String),
    #[error("cannot draw the QR code: {0}")]
    Qr(String),
}

/// Encrypts TOTP secrets with AES-256-GCM. The account id is the associated
/// data: a ciphertext copied to another account does not decrypt.
#[derive(Clone)]
pub struct MfaCipher {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for MfaCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MfaCipher").finish_non_exhaustive()
    }
}

impl MfaCipher {
    /// `key`: 32 random bytes in base64 (`MFA_ENCRYPTION_KEY`).
    pub fn new(key: &Secret) -> Result<Self, MfaError> {
        let bytes = STANDARD.decode(key.expose().trim()).map_err(|_| MfaError::Key)?;
        let cipher = Aes256Gcm::new_from_slice(&bytes).map_err(|_| MfaError::Key)?;
        Ok(Self { cipher })
    }

    /// `(ciphertext, nonce)` of `secret`.
    pub fn encrypt(&self, account_id: Uuid, secret: &[u8]) -> Result<(Vec<u8>, Vec<u8>), MfaError> {
        let nonce: [u8; 12] = rand::random();
        let ciphertext = self
            .cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: secret,
                    aad: account_id.as_bytes(),
                },
            )
            .map_err(|_| MfaError::Cipher)?;
        Ok((ciphertext, nonce.to_vec()))
    }

    pub fn decrypt(&self, account_id: Uuid, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, MfaError> {
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| MfaError::Cipher)?;
        self.cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: account_id.as_bytes(),
                },
            )
            .map_err(|_| MfaError::Cipher)
    }
}

/// A new random TOTP secret.
pub fn generate_secret() -> Vec<u8> {
    let bytes: [u8; SECRET_BYTES] = rand::random();
    bytes.to_vec()
}

fn totp(secret: &[u8], email: &str) -> Result<Totp, MfaError> {
    // The window (skew) is applied by `domain::mfa::candidate_steps`.
    Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(TOTP_DIGITS)
        .with_step_duration(TOTP_STEP_SECS)
        .with_secret(secret)
        .with_issuer(Some(ISSUER))
        .with_account_name(email)
        .build()
        .map_err(|e| MfaError::Totp(e.to_string()))
}

/// What an authenticator app needs: the `otpauth://` URI (in the QR code)
/// and the secret in base32, to type when the camera cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrollment {
    pub otpauth_uri: String,
    pub secret_base32: String,
    /// The URI as a QR code, an SVG image drawn here: nothing leaves the server.
    pub qr_code_svg: String,
}

pub fn enrollment(secret: &[u8], email: &str) -> Result<Enrollment, MfaError> {
    let totp = totp(secret, email)?;
    let otpauth_uri = totp.to_url().map_err(|e| MfaError::Totp(e.to_string()))?;
    let qr_code_svg = qrcode::QrCode::new(otpauth_uri.as_bytes())
        .map_err(|e| MfaError::Qr(e.to_string()))?
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build();
    Ok(Enrollment {
        otpauth_uri,
        secret_base32: totp.secret().to_base32(),
        qr_code_svg,
    })
}

/// The TOTP code of `secret` at `step` (for tests and the end-to-end flow).
pub fn code_at(secret: &[u8], step: u64) -> Result<String, MfaError> {
    Ok(totp(secret, "x@example.org")?
        .generate(step * TOTP_STEP_SECS)
        .to_string())
}

/// The TOTP code of the base32 `secret` (as shown at enrollment) at
/// `unix_secs`: what an authenticator app displays. For tests.
pub fn code_for_base32(secret: &str, unix_secs: u64) -> Result<String, MfaError> {
    let secret = totp_rs::Secret::try_from_base32(secret).map_err(|e| MfaError::Totp(e.to_string()))?;
    code_at(secret.as_ref(), domain::mfa::time_step(unix_secs))
}

/// The step of `candidates` whose code is `code`, if any.
pub fn matching_step(secret: &[u8], code: &str, candidates: &[u64]) -> Result<Option<u64>, MfaError> {
    let totp = totp(secret, "x@example.org")?;
    Ok(candidates
        .iter()
        .copied()
        .find(|step| constant_time_eq(&totp.generate(step * TOTP_STEP_SECS).to_string(), code)))
}

/// New backup codes, normalized (see `domain::mfa::parse_code`).
pub fn generate_backup_codes() -> Vec<String> {
    (0..BACKUP_CODE_COUNT)
        .map(|_| {
            (0..BACKUP_CODE_CHARS)
                .map(|_| {
                    let index = rand::random_range(0..BACKUP_CODE_ALPHABET.len());
                    char::from(BACKUP_CODE_ALPHABET[index])
                })
                .collect()
        })
        .collect()
}

// --- Storage ----------------------------------------------------------------------

/// Two-step verification of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMfa {
    pub secret_ciphertext: Vec<u8>,
    pub secret_nonce: Vec<u8>,
    /// `false` while enrollment waits for its first code.
    pub enabled: bool,
    pub last_used_step: Option<i64>,
}

/// The stored state of `account_id`, if enrollment started.
pub async fn find(conn: &mut PgConnection, account_id: Uuid) -> Result<Option<StoredMfa>, sqlx::Error> {
    sqlx::query_as!(
        StoredMfa,
        r#"
        SELECT secret_ciphertext, secret_nonce, enabled_at IS NOT NULL AS "enabled!", last_used_step
        FROM account_mfa WHERE account_id = $1
        "#,
        account_id
    )
    .fetch_optional(conn)
    .await
}

/// Whether two-step verification is enabled for `account_id`.
pub async fn is_enabled(conn: &mut PgConnection, account_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(find(conn, account_id).await?.is_some_and(|m| m.enabled))
}

/// Unused backup codes of `account_id`.
pub async fn backup_codes_left(conn: &mut PgConnection, account_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "left!" FROM account_backup_code WHERE account_id = $1 AND used_at IS NULL"#,
        account_id
    )
    .fetch_one(conn)
    .await
}

/// Starts (or restarts) an enrollment with a new secret. False when two-step
/// verification is already enabled: nothing changes then.
pub async fn start_enrollment(
    conn: &mut PgConnection,
    account_id: Uuid,
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        INSERT INTO account_mfa (account_id, secret_ciphertext, secret_nonce)
        VALUES ($1, $2, $3)
        ON CONFLICT (account_id) DO UPDATE
            SET secret_ciphertext = EXCLUDED.secret_ciphertext,
                secret_nonce = EXCLUDED.secret_nonce,
                last_used_step = NULL,
                version = account_mfa.version + 1
            WHERE account_mfa.enabled_at IS NULL
        "#,
        account_id,
        ciphertext,
        nonce
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Records that the code of `step` was accepted, unless a code of this step
/// or a later one already was (two requests racing with the same code).
pub async fn record_step(conn: &mut PgConnection, account_id: Uuid, step: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE account_mfa SET last_used_step = $2, version = version + 1
        WHERE account_id = $1 AND (last_used_step IS NULL OR last_used_step < $2)
        "#,
        account_id,
        step
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Enables two-step verification (enrollment confirmed by a code of `step`)
/// with new backup codes, replacing any former ones.
pub async fn enable(
    conn: &mut PgConnection,
    account_id: Uuid,
    step: i64,
    backup_codes: &[String],
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE account_mfa SET enabled_at = now(), last_used_step = $2, version = version + 1
        WHERE account_id = $1 AND enabled_at IS NULL
        "#,
        account_id,
        step
    )
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() != 1 {
        return Ok(false);
    }
    sqlx::query!(
        "DELETE FROM account_backup_code WHERE account_id = $1",
        account_id
    )
    .execute(&mut *conn)
    .await?;
    for code in backup_codes {
        sqlx::query!(
            "INSERT INTO account_backup_code (id, account_id, code_hash) VALUES ($1, $2, $3)",
            new_id(),
            account_id,
            token_hash(code)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(true)
}

/// Uses the backup code `code` (normalized). False if unknown or already used.
pub async fn use_backup_code(
    conn: &mut PgConnection,
    account_id: Uuid,
    code: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE account_backup_code SET used_at = now(), version = version + 1
        WHERE account_id = $1 AND code_hash = $2 AND used_at IS NULL
        "#,
        account_id,
        token_hash(code)
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Turns two-step verification off: secret and backup codes are deleted.
pub async fn disable(conn: &mut PgConnection, account_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM account_backup_code WHERE account_id = $1",
        account_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM account_mfa WHERE account_id = $1", account_id)
        .execute(conn)
        .await?;
    Ok(())
}

// --- Authentication policy of the tenant -------------------------------------------

/// Authentication policy of the current tenant (§3), set by its tenant admin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthPolicy {
    /// Members signing in with a password must use two-step verification.
    pub local_accounts_require_mfa: bool,
    pub version: i64,
}

/// The policy of the current tenant.
pub async fn auth_policy(conn: &mut PgConnection) -> Result<AuthPolicy, sqlx::Error> {
    sqlx::query_as!(
        AuthPolicy,
        "SELECT local_accounts_require_mfa, version FROM tenant_auth_policy"
    )
    .fetch_one(conn)
    .await
}

/// Changes the policy if its version is one of `expected_versions` (`None`:
/// any). `None` when the version did not match.
pub async fn set_auth_policy(
    conn: &mut PgConnection,
    local_accounts_require_mfa: bool,
    expected_versions: Option<&[i64]>,
) -> Result<Option<AuthPolicy>, sqlx::Error> {
    sqlx::query_as!(
        AuthPolicy,
        r#"
        UPDATE tenant_auth_policy
        SET local_accounts_require_mfa = $1, version = version + 1
        WHERE $2::bigint[] IS NULL OR version = ANY($2)
        RETURNING local_accounts_require_mfa, version
        "#,
        local_accounts_require_mfa,
        expected_versions
    )
    .fetch_optional(conn)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> MfaCipher {
        MfaCipher::new(&Secret::new(STANDARD.encode([7_u8; 32]))).unwrap()
    }

    #[test]
    fn secrets_round_trip_and_are_bound_to_their_account() {
        let (account, other) = (new_id(), new_id());
        let secret = generate_secret();
        assert_eq!(secret.len(), 20);
        let (ciphertext, nonce) = cipher().encrypt(account, &secret).unwrap();
        assert_ne!(ciphertext, secret);
        assert_eq!(cipher().decrypt(account, &ciphertext, &nonce).unwrap(), secret);
        assert!(
            cipher().decrypt(other, &ciphertext, &nonce).is_err(),
            "not for another account"
        );
        let wrong_key = MfaCipher::new(&Secret::new(STANDARD.encode([8_u8; 32]))).unwrap();
        assert!(wrong_key.decrypt(account, &ciphertext, &nonce).is_err());
        let (again, nonce_again) = cipher().encrypt(account, &secret).unwrap();
        assert_ne!(
            (again, nonce_again),
            (ciphertext, nonce),
            "a fresh nonce each time"
        );
    }

    #[test]
    fn keys_must_be_32_bytes_of_base64() {
        assert!(MfaCipher::new(&Secret::new(STANDARD.encode([1_u8; 16]))).is_err());
        assert!(MfaCipher::new(&Secret::new("not base64!")).is_err());
    }

    #[test]
    fn codes_match_the_rfc_6238_test_vector() {
        // RFC 6238, appendix B: SHA-1, secret "12345678901234567890", T = 59 s.
        let secret = b"12345678901234567890";
        assert_eq!(code_at(secret, 1).unwrap(), "287082");
        assert_eq!(matching_step(secret, "287082", &[0, 1, 2]).unwrap(), Some(1));
        assert_eq!(matching_step(secret, "287082", &[2, 3]).unwrap(), None);
    }

    #[test]
    fn the_base32_secret_gives_the_codes_of_the_app() {
        let secret = generate_secret();
        let shown = enrollment(&secret, "a@b.io").unwrap().secret_base32;
        assert_eq!(code_for_base32(&shown, 59).unwrap(), code_at(&secret, 1).unwrap());
    }

    #[test]
    fn enrollment_gives_an_otpauth_uri_a_typable_secret_and_a_qr_code() {
        let secret = generate_secret();
        let enrollment = enrollment(&secret, "lucas@acme.example").unwrap();
        assert!(
            enrollment.otpauth_uri.starts_with("otpauth://totp/"),
            "{}",
            enrollment.otpauth_uri
        );
        assert!(
            enrollment.otpauth_uri.contains("lucas%40acme.example"),
            "{}",
            enrollment.otpauth_uri
        );
        assert!(
            enrollment
                .otpauth_uri
                .contains(&format!("secret={}", enrollment.secret_base32))
        );
        assert!(
            enrollment
                .otpauth_uri
                .contains("issuer=My%20Little%20Big%20Project%20Planner")
        );
        assert_eq!(enrollment.secret_base32.len(), 32);
        assert!(enrollment.qr_code_svg.contains("<svg"));
    }

    #[test]
    fn backup_codes_are_random_and_parseable() {
        let codes = generate_backup_codes();
        assert_eq!(codes.len(), 8);
        for code in &codes {
            assert_eq!(
                domain::mfa::parse_code(code),
                Some(domain::mfa::Code::Backup(code.clone()))
            );
        }
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), 8);
    }
}
