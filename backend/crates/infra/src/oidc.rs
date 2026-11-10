//! Sign-in through an OIDC provider: authorization code flow with PKCE,
//! `state` and `nonce` (specification §14).
//!
//! 1. [`start`]: the provider's metadata (discovery), then the URL to send
//!    the browser to, with a fresh state, nonce and PKCE verifier. The caller
//!    keeps them ([`LoginStore`]) until the browser comes back.
//! 2. [`finish`]: the code is exchanged for tokens (with the verifier and
//!    the client secret), and the ID token checked: signature against the
//!    provider's keys (asymmetric algorithms only), issuer, audience, expiry
//!    and nonce. Its claims are returned.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fred::prelude::KeysInterface;
use fred::types::Expiration;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::credentials::{constant_time_eq, random_token, token_hash_hex};
use crate::http_client::{HttpClient, HttpError};
use crate::valkey::{Valkey, ValkeyError};

/// Signature algorithms accepted for ID tokens: never `none`, never HMAC
/// (it would rely on the client secret).
const ALGORITHMS: [Algorithm; 7] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::ES256,
    Algorithm::ES384,
];

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("the provider cannot be reached or answered badly: {0}")]
    Http(#[from] HttpError),
    #[error("the provider's metadata announce issuer {found}, not {expected}")]
    IssuerMismatch { expected: String, found: String },
    #[error("the token answer holds no ID token")]
    NoIdToken,
    #[error("invalid ID token: {0}")]
    InvalidToken(String),
}

/// What `/.well-known/openid-configuration` tells.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Metadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The metadata of the provider `issuer`; they must name that issuer.
pub async fn discover(http: &HttpClient, issuer: &str) -> Result<Metadata, OidcError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let metadata: Metadata = http.get_json(&url).await?;
    if !same_issuer(&metadata.issuer, issuer) {
        return Err(OidcError::IssuerMismatch {
            expected: issuer.to_owned(),
            found: metadata.issuer,
        });
    }
    Ok(metadata)
}

/// A sign-in sent to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// Where to send the browser.
    pub url: String,
    /// What to keep until it comes back.
    pub state: String,
    pub nonce: String,
    pub code_verifier: String,
}

/// The PKCE challenge of `verifier` (S256).
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The authorization URL for `client_id`, coming back to `redirect_uri`.
pub fn start(metadata: &Metadata, client_id: &str, redirect_uri: &str) -> Result<Start, OidcError> {
    let (state, nonce, code_verifier) = (random_token(), random_token(), random_token());
    let mut url = url::Url::parse(&metadata.authorization_endpoint)
        .map_err(|_| HttpError::Url(metadata.authorization_endpoint.clone()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "openid email profile")
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &pkce_challenge(&code_verifier))
        .append_pair("code_challenge_method", "S256");
    Ok(Start {
        url: url.into(),
        state,
        nonce,
        code_verifier,
    })
}

#[derive(Deserialize)]
struct TokenAnswer {
    id_token: Option<String>,
}

/// The claims of a checked ID token.
pub type Claims = serde_json::Map<String, serde_json::Value>;

/// The client credentials of a provider.
#[derive(Debug, Clone, Copy)]
pub struct ClientCredentials<'a> {
    pub client_id: &'a str,
    pub client_secret: &'a str,
}

/// Exchanges `code` for tokens and checks the ID token (see the module).
pub async fn finish(
    http: &HttpClient,
    metadata: &Metadata,
    client: ClientCredentials<'_>,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
    nonce: &str,
) -> Result<Claims, OidcError> {
    let answer: TokenAnswer = http
        .post_form(
            &metadata.token_endpoint,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", code_verifier),
                ("client_id", client.client_id),
                ("client_secret", client.client_secret),
            ],
        )
        .await?;
    let id_token = answer.id_token.ok_or(OidcError::NoIdToken)?;
    let keys: JwkSet = http.get_json(&metadata.jwks_uri).await?;
    verify_id_token(&id_token, &keys, &metadata.issuer, client.client_id, nonce)
}

/// Checks `id_token` against `keys`, `issuer`, `audience` and `nonce`.
pub fn verify_id_token(
    id_token: &str,
    keys: &JwkSet,
    issuer: &str,
    audience: &str,
    nonce: &str,
) -> Result<Claims, OidcError> {
    let invalid = |reason: &str| OidcError::InvalidToken(reason.to_owned());
    let header = jsonwebtoken::decode_header(id_token).map_err(|e| OidcError::InvalidToken(e.to_string()))?;
    if !ALGORITHMS.contains(&header.alg) {
        return Err(invalid("signature algorithm not accepted"));
    }
    let key = match &header.kid {
        Some(kid) => keys.find(kid),
        // Without a key id, the only key of the set.
        None if keys.keys.len() == 1 => keys.keys.first(),
        None => None,
    }
    .ok_or_else(|| invalid("unknown signing key"))?;
    let decoding = DecodingKey::from_jwk(key).map_err(|e| OidcError::InvalidToken(e.to_string()))?;
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.leeway = 60;
    let claims = jsonwebtoken::decode::<Claims>(id_token, &decoding, &validation)
        .map_err(|e| OidcError::InvalidToken(e.to_string()))?
        .claims;
    let sent = claims.get("nonce").and_then(|v| v.as_str()).unwrap_or_default();
    if !constant_time_eq(sent, nonce) {
        return Err(invalid("nonce mismatch"));
    }
    Ok(claims)
}

// --- Sign-ins in progress ------------------------------------------------------------

/// A sign-in waiting for the provider's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingLogin {
    pub tenant_id: Uuid,
    pub provider_id: Uuid,
    pub nonce: String,
    pub code_verifier: String,
    /// Page of the application to show afterwards.
    pub return_url: Option<String>,
}

/// Sign-ins in progress, in Valkey under the hash of their `state`, for
/// [`domain::oidc::LOGIN_LIFETIME`]. Each one is taken once.
#[derive(Debug, Clone)]
pub struct LoginStore {
    valkey: Valkey,
    prefix: String,
}

impl LoginStore {
    pub fn new(valkey: Valkey, prefix: impl Into<String>) -> Self {
        Self {
            valkey,
            prefix: prefix.into(),
        }
    }

    fn key(&self, state: &str) -> String {
        format!("{}oidc-login:{}", self.prefix, token_hash_hex(state))
    }

    pub async fn save(&self, state: &str, login: &PendingLogin) -> Result<(), ValkeyError> {
        let json = serde_json::to_string(login)
            .map_err(|e| ValkeyError::new(fred::error::ErrorKind::Parse, e.to_string()))?;
        let seconds = i64::try_from(domain::oidc::LOGIN_LIFETIME.as_secs()).unwrap_or(600);
        self.valkey
            .set(self.key(state), json, Some(Expiration::EX(seconds)), None, false)
            .await
    }

    /// The sign-in of `state`, removed: a state works once.
    pub async fn take(&self, state: &str) -> Result<Option<PendingLogin>, ValkeyError> {
        let json: Option<String> = self.valkey.getdel(self.key(state)).await?;
        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Metadata {
        Metadata {
            issuer: "https://idp.example.org/realms/acme".to_owned(),
            authorization_endpoint: "https://idp.example.org/realms/acme/auth".to_owned(),
            token_endpoint: "https://idp.example.org/realms/acme/token".to_owned(),
            jwks_uri: "https://idp.example.org/realms/acme/certs".to_owned(),
        }
    }

    #[test]
    fn the_pkce_challenge_follows_rfc_7636() {
        // RFC 7636, appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_authorization_url_carries_state_nonce_and_pkce() {
        let start = start(
            &metadata(),
            "mlbpp",
            "https://acme.example.org/api/v1/auth/oidc/callback",
        )
        .unwrap();
        let url = url::Url::parse(&start.url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.path(), "/realms/acme/auth");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "mlbpp");
        assert_eq!(
            query["redirect_uri"],
            "https://acme.example.org/api/v1/auth/oidc/callback"
        );
        assert_eq!(query["scope"], "openid email profile");
        assert_eq!(query["state"], start.state);
        assert_eq!(query["nonce"], start.nonce);
        assert_eq!(query["code_challenge"], pkce_challenge(&start.code_verifier));
        assert_eq!(query["code_challenge_method"], "S256");
        let again = super::start(&metadata(), "mlbpp", "https://x").unwrap();
        assert_ne!((again.state, again.nonce), (start.state, start.nonce));
    }

    #[test]
    fn tokens_signed_with_a_shared_secret_or_none_are_refused() {
        let keys = JwkSet { keys: vec![] };
        // {"alg":"HS256"} . {"sub":"x"} . signature
        let hs256 = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln";
        let err = verify_id_token(hs256, &keys, "https://idp", "mlbpp", "n").unwrap_err();
        assert!(err.to_string().contains("not accepted"), "{err}");
        // {"alg":"none"}
        let none = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ4In0.";
        assert!(verify_id_token(none, &keys, "https://idp", "mlbpp", "n").is_err());
        // {"alg":"RS256","kid":"k"}: no such key.
        let unknown = "eyJhbGciOiJSUzI1NiIsImtpZCI6ImsifQ.eyJzdWIiOiJ4In0.c2ln";
        let err = verify_id_token(unknown, &keys, "https://idp", "mlbpp", "n").unwrap_err();
        assert!(err.to_string().contains("unknown signing key"), "{err}");
    }

    /// An ES256 key pair: its PKCS#8 document and its public JWK set.
    fn key_pair(kid: &str) -> (Vec<u8>, JwkSet) {
        use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let point = pair.public_key().as_ref();
        let jwks: JwkSet = serde_json::from_value(serde_json::json!({"keys": [{
            "kty": "EC", "crv": "P-256", "kid": kid, "use": "sig", "alg": "ES256",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        }]}))
        .unwrap();
        (pkcs8.as_ref().to_vec(), jwks)
    }

    fn token(pkcs8: &[u8], kid: &str, claims: &serde_json::Value) -> String {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_owned());
        jsonwebtoken::encode(&header, claims, &jsonwebtoken::EncodingKey::from_ec_der(pkcs8)).unwrap()
    }

    fn claims(changes: &serde_json::Value) -> serde_json::Value {
        let now = chrono::Utc::now().timestamp();
        let mut claims = serde_json::json!({
            "iss": "https://idp.example.org/realms/acme", "aud": "mlbpp", "sub": "user-1",
            "exp": now + 300, "iat": now, "nonce": "the-nonce",
            "email": "lucas@acme.example", "email_verified": true,
        });
        for (key, value) in changes.as_object().unwrap() {
            if value.is_null() {
                claims.as_object_mut().unwrap().remove(key);
            } else {
                claims[key] = value.clone();
            }
        }
        claims
    }

    fn check(token: &str, keys: &JwkSet) -> Result<Claims, OidcError> {
        verify_id_token(
            token,
            keys,
            "https://idp.example.org/realms/acme",
            "mlbpp",
            "the-nonce",
        )
    }

    #[test]
    fn a_valid_id_token_gives_its_claims() {
        let (pkcs8, keys) = key_pair("k1");
        let claims = check(&token(&pkcs8, "k1", &claims(&serde_json::json!({}))), &keys).unwrap();
        assert_eq!(claims["sub"], "user-1");
        assert_eq!(claims["email"], "lucas@acme.example");
    }

    #[test]
    fn id_tokens_are_checked_claim_by_claim() {
        let (pkcs8, keys) = key_pair("k1");
        let past = chrono::Utc::now().timestamp() - 3600;
        for (case, changes) in [
            ("expired", serde_json::json!({"exp": past})),
            ("other audience", serde_json::json!({"aud": "another-client"})),
            (
                "other issuer",
                serde_json::json!({"iss": "https://evil.example.org"}),
            ),
            ("other nonce", serde_json::json!({"nonce": "replayed"})),
            ("no nonce", serde_json::json!({"nonce": null})),
            ("no subject", serde_json::json!({"sub": null})),
        ] {
            let result = check(&token(&pkcs8, "k1", &claims(&changes)), &keys);
            assert!(result.is_err(), "{case} must be refused");
        }
    }

    #[test]
    fn a_token_signed_by_another_key_is_refused() {
        let (_, keys) = key_pair("k1");
        let (other_pkcs8, _) = key_pair("k1");
        let forged = token(&other_pkcs8, "k1", &claims(&serde_json::json!({})));
        assert!(check(&forged, &keys).is_err());
    }

    #[test]
    fn issuers_compare_without_trailing_slash() {
        assert!(same_issuer("https://idp/realms/a/", "https://idp/realms/a"));
        assert!(!same_issuer("https://idp/realms/a", "https://idp/realms/b"));
    }
}
