//! Encryption of secrets stored in the database (TOTP secrets, OIDC client
//! secrets): AES-256-GCM with the key `DATA_ENCRYPTION_KEY`.
//!
//! The caller binds each ciphertext to its owner with associated data (the
//! row's id): copied to another row, it does not decrypt.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::config::Secret;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("DATA_ENCRYPTION_KEY must be 32 bytes encoded in base64")]
    Key,
    #[error("cannot encrypt or decrypt a stored secret")]
    Cipher,
}

/// AES-256-GCM with a random 96-bit nonce per encryption.
#[derive(Clone)]
pub struct SecretCipher {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for SecretCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretCipher").finish_non_exhaustive()
    }
}

impl SecretCipher {
    /// `key`: 32 random bytes in base64 (`DATA_ENCRYPTION_KEY`).
    pub fn new(key: &Secret) -> Result<Self, CryptoError> {
        let bytes = STANDARD
            .decode(key.expose().trim())
            .map_err(|_| CryptoError::Key)?;
        let cipher = Aes256Gcm::new_from_slice(&bytes).map_err(|_| CryptoError::Key)?;
        Ok(Self { cipher })
    }

    /// `(ciphertext, nonce)` of `plaintext`, bound to `owner`.
    pub fn encrypt(&self, owner: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
        let nonce: [u8; 12] = rand::random();
        let ciphertext = self
            .cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: owner,
                },
            )
            .map_err(|_| CryptoError::Cipher)?;
        Ok((ciphertext, nonce.to_vec()))
    }

    /// The plaintext, if `ciphertext` was encrypted for `owner` with this key.
    pub fn decrypt(&self, owner: &[u8], ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| CryptoError::Cipher)?;
        self.cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: owner,
                },
            )
            .map_err(|_| CryptoError::Cipher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> SecretCipher {
        SecretCipher::new(&Secret::new(STANDARD.encode([byte; 32]))).unwrap()
    }

    #[test]
    fn secrets_round_trip_and_are_bound_to_their_owner() {
        let (ciphertext, nonce) = cipher(7).encrypt(b"owner-1", b"secret").unwrap();
        assert_ne!(ciphertext, b"secret");
        assert_eq!(
            cipher(7).decrypt(b"owner-1", &ciphertext, &nonce).unwrap(),
            b"secret"
        );
        assert!(
            cipher(7).decrypt(b"owner-2", &ciphertext, &nonce).is_err(),
            "not for another owner"
        );
        assert!(
            cipher(8).decrypt(b"owner-1", &ciphertext, &nonce).is_err(),
            "not with another key"
        );
        let (again, nonce_again) = cipher(7).encrypt(b"owner-1", b"secret").unwrap();
        assert_ne!(
            (again, nonce_again),
            (ciphertext, nonce),
            "a fresh nonce each time"
        );
    }

    #[test]
    fn keys_must_be_32_bytes_of_base64() {
        assert!(SecretCipher::new(&Secret::new(STANDARD.encode([1_u8; 16]))).is_err());
        assert!(SecretCipher::new(&Secret::new("not base64!")).is_err());
    }
}
