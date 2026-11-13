//! Signature checks of JSON Web Tokens with `ring`, the cryptography the
//! rest of the stack already uses (rustls, lettre), for `jsonwebtoken`.
//!
//! `jsonwebtoken` 10 ships two cryptography backends, and both are kept out
//! on purpose: `rust_crypto` pulls the `rsa` crate (RUSTSEC-2023-0071), and
//! `aws_lc_rs` a license outside the project's policy (CLAUDE.md). It lets
//! the application install its own provider instead: this one only verifies
//! signatures, which is all the OIDC sign-in needs. It never signs.

use jsonwebtoken::crypto::{CryptoProvider, JwkUtils, JwtSigner, JwtVerifier};
use jsonwebtoken::errors::{ErrorKind, Result};
use jsonwebtoken::{Algorithm, DecodingKey, DecodingKeyKind, EncodingKey};
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P384_SHA384_FIXED, ED25519, RSA_PKCS1_2048_8192_SHA256,
    RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512, RSA_PSS_2048_8192_SHA256,
    RSA_PSS_2048_8192_SHA384, RSA_PSS_2048_8192_SHA512, RsaParameters, RsaPublicKeyComponents,
    UnparsedPublicKey, VerificationAlgorithm,
};

/// The provider: verification with `ring`, no signing, no JWK computation.
pub static PROVIDER: CryptoProvider = CryptoProvider {
    signer_factory: no_signer,
    verifier_factory: verifier,
    jwk_utils: JwkUtils::new_unimplemented(),
};

/// Makes [`PROVIDER`] the process's provider. Idempotent: later calls, or a
/// provider installed before, leave things as they are.
pub fn install() {
    let _ = PROVIDER.install_default();
}

/// How `ring` checks a signature of `algorithm`: `None` for the algorithms
/// this provider does not verify (shared secrets).
fn ring_algorithm(algorithm: Algorithm) -> Option<Check> {
    Some(match algorithm {
        Algorithm::RS256 => Check::Rsa(&RSA_PKCS1_2048_8192_SHA256),
        Algorithm::RS384 => Check::Rsa(&RSA_PKCS1_2048_8192_SHA384),
        Algorithm::RS512 => Check::Rsa(&RSA_PKCS1_2048_8192_SHA512),
        Algorithm::PS256 => Check::Rsa(&RSA_PSS_2048_8192_SHA256),
        Algorithm::PS384 => Check::Rsa(&RSA_PSS_2048_8192_SHA384),
        Algorithm::PS512 => Check::Rsa(&RSA_PSS_2048_8192_SHA512),
        Algorithm::ES256 => Check::Other(&ECDSA_P256_SHA256_FIXED),
        Algorithm::ES384 => Check::Other(&ECDSA_P384_SHA384_FIXED),
        Algorithm::EdDSA => Check::Other(&ED25519),
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => return None,
    })
}

#[derive(Clone, Copy)]
enum Check {
    Rsa(&'static RsaParameters),
    Other(&'static dyn VerificationAlgorithm),
}

fn no_signer(_: &Algorithm, _: &EncodingKey) -> Result<Box<dyn JwtSigner>> {
    Err(ErrorKind::InvalidAlgorithm.into())
}

fn verifier(algorithm: &Algorithm, key: &DecodingKey) -> Result<Box<dyn JwtVerifier>> {
    let check = ring_algorithm(*algorithm).ok_or(ErrorKind::InvalidAlgorithm)?;
    Ok(Box::new(RingVerifier {
        algorithm: *algorithm,
        check,
        key: key.clone(),
    }))
}

struct RingVerifier {
    algorithm: Algorithm,
    check: Check,
    key: DecodingKey,
}

impl signature::Verifier<Vec<u8>> for RingVerifier {
    fn verify(&self, message: &[u8], signature: &Vec<u8>) -> std::result::Result<(), signature::Error> {
        let verified = match (self.check, self.key.kind()) {
            // A JWK's modulus and exponent.
            (Check::Rsa(parameters), DecodingKeyKind::RsaModulusExponent { n, e }) => {
                RsaPublicKeyComponents { n, e }.verify(parameters, message, signature)
            }
            // A DER RSA public key.
            (Check::Rsa(parameters), DecodingKeyKind::SecretOrDer(der)) => {
                UnparsedPublicKey::new(parameters, der).verify(message, signature)
            }
            // An EC point or an Ed25519 key.
            (Check::Other(algorithm), DecodingKeyKind::SecretOrDer(bytes)) => {
                UnparsedPublicKey::new(algorithm, bytes).verify(message, signature)
            }
            (Check::Other(_), DecodingKeyKind::RsaModulusExponent { .. }) => {
                return Err(signature::Error::new());
            }
        };
        verified.map_err(|_| signature::Error::new())
    }
}

impl JwtVerifier for RingVerifier {
    fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_secrets_are_not_verified() {
        let key = DecodingKey::from_secret(b"secret");
        for algorithm in [Algorithm::HS256, Algorithm::HS384, Algorithm::HS512] {
            assert!(verifier(&algorithm, &key).is_err(), "{algorithm:?}");
        }
    }

    #[test]
    fn nothing_is_signed() {
        let key = EncodingKey::from_secret(b"secret");
        assert!(no_signer(&Algorithm::RS256, &key).is_err());
    }

    #[test]
    fn a_key_of_the_wrong_kind_verifies_nothing() {
        let rsa = DecodingKey::from_rsa_raw_components(&[1; 256], &[1, 0, 1]);
        let verifier = verifier(&Algorithm::ES256, &rsa).unwrap_or_else(|e| panic!("{e}"));
        assert!(signature::Verifier::verify(verifier.as_ref(), b"message", &vec![0; 64]).is_err());
    }
}
