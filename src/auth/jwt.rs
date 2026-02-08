use base64::Engine;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rsa::{RsaPrivateKey, RsaPublicKey};
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, DecodePublicKey, LineEnding};
use rsa::traits::PublicKeyParts;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const ACCESS_TOKEN_TTL_SECONDS: u64 = 3600; // 1 hour
const REFRESH_TOKEN_TTL_SECONDS: u64 = 2_592_000; // 30 days

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("Failed to generate RSA key pair: {0}")]
    KeyGeneration(String),

    #[error("Failed to encode token: {0}")]
    Encoding(#[from] jsonwebtoken::errors::Error),

    #[error("Failed to decode token: {0}")]
    Decoding(String),

    #[error("Token has expired")]
    Expired,

    #[error("Invalid token")]
    Invalid,

    #[error("PEM encoding error: {0}")]
    PemEncoding(String),

    #[error("Environment variable error: {0}")]
    EnvVar(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub identity_id: String,
    pub tenant_id: String,
    pub tenant_slug: String,
    pub platform_code: String,
    pub role: String,
    pub local_user_id: Option<String>,
    pub exp: u64, // Expiration time (Unix timestamp)
    pub iat: u64, // Issued at (Unix timestamp)
}

#[derive(Clone)]
pub struct JwtService {
    encoding_key: Arc<EncodingKey>,
    decoding_key: Arc<DecodingKey>,
    public_key_pem: Arc<String>,
}

impl JwtService {
    /// Create a new JwtService by generating a new RSA key pair
    pub fn new_with_generated_keys() -> Result<Self, JwtError> {
        let mut rng = rand::thread_rng();

        // Generate 2048-bit RSA key pair
        let private_key = RsaPrivateKey::new(&mut rng, 2048)
            .map_err(|e| JwtError::KeyGeneration(e.to_string()))?;

        let public_key = RsaPublicKey::from(&private_key);

        // Encode to PEM format
        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| JwtError::PemEncoding(e.to_string()))?;

        let public_pem = public_key
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| JwtError::PemEncoding(e.to_string()))?;

        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes())
            .map_err(|e| JwtError::KeyGeneration(e.to_string()))?;

        let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())
            .map_err(|e| JwtError::KeyGeneration(e.to_string()))?;

        Ok(Self {
            encoding_key: Arc::new(encoding_key),
            decoding_key: Arc::new(decoding_key),
            public_key_pem: Arc::new(public_pem),
        })
    }

    /// Create a new JwtService from PEM-encoded private and public keys
    pub fn new_from_pem(private_pem: &str, public_pem: &str) -> Result<Self, JwtError> {
        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes())
            .map_err(|e| JwtError::KeyGeneration(e.to_string()))?;

        let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())
            .map_err(|e| JwtError::KeyGeneration(e.to_string()))?;

        Ok(Self {
            encoding_key: Arc::new(encoding_key),
            decoding_key: Arc::new(decoding_key),
            public_key_pem: Arc::new(public_pem.to_string()),
        })
    }

    /// Create a new JwtService from environment variables
    /// Expects JWT_PRIVATE_KEY and JWT_PUBLIC_KEY env vars with PEM content
    pub fn new_from_env() -> Result<Self, JwtError> {
        let private_pem = std::env::var("JWT_PRIVATE_KEY")
            .map_err(|e| JwtError::EnvVar(format!("JWT_PRIVATE_KEY not found: {}", e)))?;

        let public_pem = std::env::var("JWT_PUBLIC_KEY")
            .map_err(|e| JwtError::EnvVar(format!("JWT_PUBLIC_KEY not found: {}", e)))?;

        Self::new_from_pem(&private_pem, &public_pem)
    }

    /// Create a new JwtService from PEM files
    pub fn new_from_files(private_key_path: &str, public_key_path: &str) -> Result<Self, JwtError> {
        let private_pem = std::fs::read_to_string(private_key_path)
            .map_err(|e| JwtError::EnvVar(format!("Failed to read private key file: {}", e)))?;

        let public_pem = std::fs::read_to_string(public_key_path)
            .map_err(|e| JwtError::EnvVar(format!("Failed to read public key file: {}", e)))?;

        Self::new_from_pem(&private_pem, &public_pem)
    }

    /// Sign an access token (1 hour expiry)
    pub fn sign_access_token(&self, mut claims: Claims) -> Result<String, JwtError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs();

        claims.iat = now;
        claims.exp = now + ACCESS_TOKEN_TTL_SECONDS;

        let header = Header::new(Algorithm::RS256);
        encode(&header, &claims, &self.encoding_key).map_err(JwtError::from)
    }

    /// Sign a refresh token (30 days expiry)
    pub fn sign_refresh_token(&self, mut claims: Claims) -> Result<String, JwtError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs();

        claims.iat = now;
        claims.exp = now + REFRESH_TOKEN_TTL_SECONDS;

        let header = Header::new(Algorithm::RS256);
        encode(&header, &claims, &self.encoding_key).map_err(JwtError::from)
    }

    /// Verify and decode a token
    pub fn verify_token(&self, token: &str) -> Result<Claims, JwtError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;

        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
                _ => JwtError::Decoding(e.to_string()),
            })
    }

    /// Get the public key in PEM format (for JWKS endpoint)
    pub fn get_public_key_pem(&self) -> String {
        (*self.public_key_pem).clone()
    }

    /// Get the public key in JWK format for JWKS endpoint
    pub fn get_jwks(&self) -> serde_json::Value {
        // Parse the public key to extract modulus and exponent
        let public_key = RsaPublicKey::from_public_key_pem(&self.public_key_pem)
            .expect("Failed to parse public key");

        let n = public_key.n();
        let e = public_key.e();

        // Convert to base64url encoding
        let n_bytes = n.to_bytes_be();
        let e_bytes = e.to_bytes_be();

        let n_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&n_bytes);
        let e_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&e_bytes);

        serde_json::json!({
            "keys": [
                {
                    "kty": "RSA",
                    "use": "sig",
                    "alg": "RS256",
                    "n": n_b64,
                    "e": e_b64
                }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_generation_and_verification() {
        let service = JwtService::new_with_generated_keys().unwrap();

        let claims = Claims {
            identity_id: "user123".to_string(),
            tenant_id: "tenant456".to_string(),
            tenant_slug: "acme-corp".to_string(),
            platform_code: "PLATFORM1".to_string(),
            role: "admin".to_string(),
            local_user_id: Some("local789".to_string()),
            exp: 0, // Will be set by sign_access_token
            iat: 0, // Will be set by sign_access_token
        };

        let token = service.sign_access_token(claims.clone()).unwrap();
        let verified_claims = service.verify_token(&token).unwrap();

        assert_eq!(verified_claims.identity_id, claims.identity_id);
        assert_eq!(verified_claims.tenant_id, claims.tenant_id);
        assert_eq!(verified_claims.role, claims.role);
    }

    #[test]
    fn test_access_and_refresh_token_ttl() {
        let service = JwtService::new_with_generated_keys().unwrap();

        let claims = Claims {
            identity_id: "user123".to_string(),
            tenant_id: "tenant456".to_string(),
            tenant_slug: "acme-corp".to_string(),
            platform_code: "PLATFORM1".to_string(),
            role: "user".to_string(),
            local_user_id: None,
            exp: 0,
            iat: 0,
        };

        let access_token = service.sign_access_token(claims.clone()).unwrap();
        let refresh_token = service.sign_refresh_token(claims.clone()).unwrap();

        let access_claims = service.verify_token(&access_token).unwrap();
        let refresh_claims = service.verify_token(&refresh_token).unwrap();

        // Access token should expire in ~1 hour
        assert_eq!(access_claims.exp - access_claims.iat, ACCESS_TOKEN_TTL_SECONDS);

        // Refresh token should expire in ~30 days
        assert_eq!(refresh_claims.exp - refresh_claims.iat, REFRESH_TOKEN_TTL_SECONDS);
    }

    #[test]
    fn test_jwks_format() {
        let service = JwtService::new_with_generated_keys().unwrap();
        let jwks = service.get_jwks();

        assert!(jwks["keys"].is_array());
        assert_eq!(jwks["keys"].as_array().unwrap().len(), 1);

        let key = &jwks["keys"][0];
        assert_eq!(key["kty"], "RSA");
        assert_eq!(key["use"], "sig");
        assert_eq!(key["alg"], "RS256");
        assert!(key["n"].is_string());
        assert!(key["e"].is_string());
    }
}
