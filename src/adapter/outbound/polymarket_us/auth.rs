//! Ed25519 authentication. Secrets are never included in Debug or errors.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use reqwest::header::{HeaderMap, HeaderValue};
use zeroize::Zeroizing;

use super::error::UsError;

/// Credentials loaded only when authenticated access is requested.
pub struct Credentials {
    key_id: HeaderValue,
    signing_key: SigningKey,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials([REDACTED])")
    }
}

impl Credentials {
    /// Read the US API credentials, independently of Polygon wallet credentials.
    pub fn from_env() -> Result<Self, UsError> {
        let key_id = std::env::var("POLYMARKET_US_KEY_ID")
            .map_err(|_| UsError::Credentials("POLYMARKET_US_KEY_ID is not set"))?;
        let secret = Zeroizing::new(
            std::env::var("POLYMARKET_US_SECRET_KEY")
                .map_err(|_| UsError::Credentials("POLYMARKET_US_SECRET_KEY is not set"))?,
        );
        Self::new(&key_id, &secret)
    }

    /// Accept a base64 Ed25519 seed or keypair, as documented by Polymarket US.
    pub fn new(key_id: &str, secret: &str) -> Result<Self, UsError> {
        if key_id.trim().is_empty() {
            return Err(UsError::Credentials("key ID is empty"));
        }
        let mut key_id = HeaderValue::from_str(key_id)
            .map_err(|_| UsError::Credentials("key ID is not a valid header"))?;
        key_id.set_sensitive(true);
        let decoded = Zeroizing::new(
            STANDARD
                .decode(secret.trim())
                .map_err(|_| UsError::Credentials("secret is not base64"))?,
        );
        if !matches!(decoded.len(), 32 | 64) {
            return Err(UsError::Credentials("secret must contain 32 or 64 bytes"));
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&decoded[..32]);
        let signing_key = SigningKey::from_bytes(&seed);
        Ok(Self {
            key_id,
            signing_key,
        })
    }

    /// Sign the HTTP path without its query string, using a millisecond timestamp.
    pub fn headers(
        &self,
        method: &str,
        path: &str,
        timestamp_ms: i64,
    ) -> Result<HeaderMap, UsError> {
        if timestamp_ms <= 0 || !path.starts_with("/v1/") || path.contains(['?', '#']) {
            return Err(UsError::Protocol("invalid signing path or timestamp"));
        }
        let payload = format!("{timestamp_ms}{}{path}", method.to_ascii_uppercase());
        let signature = STANDARD.encode(self.signing_key.sign(payload.as_bytes()).to_bytes());
        let mut signature = HeaderValue::from_str(&signature)
            .map_err(|_| UsError::Protocol("invalid signature header"))?;
        signature.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("X-PM-Access-Key", self.key_id.clone());
        headers.insert(
            "X-PM-Timestamp",
            HeaderValue::from_str(&timestamp_ms.to_string())
                .map_err(|_| UsError::Protocol("invalid timestamp header"))?,
        );
        headers.insert("X-PM-Signature", signature);
        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    #[test]
    fn signatures_verify_and_bind_method_path_and_timestamp() {
        let seed = [7u8; 32];
        let credentials = Credentials::new("key-id", &STANDARD.encode(seed)).unwrap();
        let headers = credentials
            .headers("get", "/v1/account/balances", 123456789)
            .unwrap();
        let raw = STANDARD
            .decode(headers["X-PM-Signature"].as_bytes())
            .unwrap();
        let signature = Signature::from_slice(&raw).unwrap();
        let public = SigningKey::from_bytes(&seed).verifying_key();
        public
            .verify(b"123456789GET/v1/account/balances", &signature)
            .unwrap();
        assert!(public
            .verify(b"123456789POST/v1/account/balances", &signature)
            .is_err());
        assert!(headers["X-PM-Signature"].is_sensitive());
        assert!(headers["X-PM-Access-Key"].is_sensitive());
        assert_eq!(format!("{credentials:?}"), "Credentials([REDACTED])");
    }

    #[test]
    fn keypair_and_seed_sign_identically_and_invalid_input_is_rejected() {
        let seed = [7u8; 32];
        let pair = SigningKey::from_bytes(&seed).to_keypair_bytes();
        let a = Credentials::new("id", &STANDARD.encode(seed)).unwrap();
        let b = Credentials::new("id", &STANDARD.encode(pair)).unwrap();
        assert_eq!(
            a.headers("GET", "/v1/orders/open", 1).unwrap(),
            b.headers("GET", "/v1/orders/open", 1).unwrap()
        );
        for size in [0, 31, 33, 63, 65] {
            assert!(Credentials::new("id", &STANDARD.encode(vec![0; size])).is_err());
        }
        assert!(Credentials::new("", &STANDARD.encode(seed)).is_err());
        assert!(Credentials::new("id\nsecret", &STANDARD.encode(seed)).is_err());
        assert!(a.headers("GET", "/v1/orders?secret=x", 1).is_err());
    }
}
