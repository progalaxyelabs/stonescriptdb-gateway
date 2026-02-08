use dashmap::DashMap;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use oauth2::basic::{BasicClient, BasicTokenResponse};
use oauth2::reqwest::async_http_client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::error;

use crate::error::{GatewayError, Result};

/// OAuth state token information
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub platform_code: String,
    pub tenant_slug: Option<String>,
    pub redirect_uri: String,
    pub created_at: SystemTime,
}

/// In-memory state token store (states expire after 10 minutes)
pub struct OAuthStateStore {
    states: Arc<DashMap<String, OAuthState>>,
}

impl OAuthStateStore {
    pub fn new() -> Self {
        Self {
            states: Arc::new(DashMap::new()),
        }
    }

    /// Store a new state token with context
    pub fn store(&self, state: String, context: OAuthState) {
        self.states.insert(state, context);
    }

    /// Retrieve and remove a state token
    pub fn verify_and_remove(&self, state: &str) -> Option<OAuthState> {
        self.states.remove(state).map(|(_, v)| v)
    }

    /// Clean up expired states (older than 10 minutes)
    pub fn cleanup_expired(&self) {
        let now = SystemTime::now();
        self.states.retain(|_, state| {
            now.duration_since(state.created_at)
                .map(|d| d < Duration::from_secs(600))
                .unwrap_or(false)
        });
    }
}

/// Google user info response
#[derive(Debug, Deserialize)]
pub struct GoogleUserInfo {
    pub id: String,
    pub email: String,
    pub verified_email: bool,
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub picture: Option<String>,
}

/// OAuth service for managing OAuth flows
pub struct OAuthService {
    state_store: OAuthStateStore,
}

impl OAuthService {
    pub fn new() -> Self {
        Self {
            state_store: OAuthStateStore::new(),
        }
    }

    /// Generate Google OAuth authorization URL
    pub fn generate_google_auth_url(
        &self,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
        platform_code: String,
        tenant_slug: Option<String>,
    ) -> Result<String> {
        // Create OAuth2 client
        let client = BasicClient::new(
            ClientId::new(client_id.to_string()),
            Some(ClientSecret::new(client_secret.to_string())),
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
                .map_err(|e| {
                    error!("Invalid Google auth URL: {}", e);
                    GatewayError::Internal("OAuth configuration error".to_string())
                })?,
            Some(
                TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).map_err(|e| {
                    error!("Invalid Google token URL: {}", e);
                    GatewayError::Internal("OAuth configuration error".to_string())
                })?,
            ),
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                error!("Invalid redirect URI: {}", e);
                GatewayError::InvalidRequest {
                    message: "Invalid redirect_uri".to_string(),
                }
            })?,
        );

        // Generate authorization URL with CSRF protection
        let (auth_url, csrf_state) = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new("email".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new(
                "https://www.googleapis.com/auth/userinfo.email".to_string(),
            ))
            .add_scope(Scope::new(
                "https://www.googleapis.com/auth/userinfo.profile".to_string(),
            ))
            .url();

        // Store state token with context
        self.state_store.store(
            csrf_state.secret().clone(),
            OAuthState {
                platform_code,
                tenant_slug,
                redirect_uri: redirect_uri.to_string(),
                created_at: SystemTime::now(),
            },
        );

        // Cleanup expired states
        self.state_store.cleanup_expired();

        Ok(auth_url.to_string())
    }

    /// Verify state token and retrieve context
    pub fn verify_state(&self, state: &str) -> Result<OAuthState> {
        self.state_store
            .verify_and_remove(state)
            .ok_or_else(|| GatewayError::InvalidRequest {
                message: "Invalid or expired state token".to_string(),
            })
    }

    /// Exchange authorization code for tokens and get user info
    pub async fn exchange_google_code(
        &self,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
        code: &str,
    ) -> Result<(BasicTokenResponse, GoogleUserInfo)> {
        // Create OAuth2 client
        let client = BasicClient::new(
            ClientId::new(client_id.to_string()),
            Some(ClientSecret::new(client_secret.to_string())),
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
                .map_err(|e| {
                    error!("Invalid Google auth URL: {}", e);
                    GatewayError::Internal("OAuth configuration error".to_string())
                })?,
            Some(
                TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).map_err(|e| {
                    error!("Invalid Google token URL: {}", e);
                    GatewayError::Internal("OAuth configuration error".to_string())
                })?,
            ),
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                error!("Invalid redirect URI: {}", e);
                GatewayError::Internal("OAuth configuration error".to_string())
            })?,
        );

        // Exchange code for token
        let token_response = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .request_async(async_http_client)
            .await
            .map_err(|e| {
                error!("Failed to exchange code for token: {}", e);
                GatewayError::InvalidRequest {
                    message: "Failed to exchange authorization code".to_string(),
                }
            })?;

        // Get user info using access token
        let user_info = self
            .get_google_user_info(token_response.access_token().secret())
            .await?;

        Ok((token_response, user_info))
    }

    /// Get Google user info from access token
    async fn get_google_user_info(&self, access_token: &str) -> Result<GoogleUserInfo> {
        let client = reqwest::Client::new();
        let response = client
            .get("https://www.googleapis.com/oauth2/v2/userinfo")
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| {
                error!("Failed to fetch Google user info: {}", e);
                GatewayError::Internal("Failed to fetch user info from Google".to_string())
            })?;

        if !response.status().is_success() {
            error!("Google user info request failed: {}", response.status());
            return Err(GatewayError::Internal(
                "Failed to fetch user info from Google".to_string(),
            ));
        }

        response.json::<GoogleUserInfo>().await.map_err(|e| {
            error!("Failed to parse Google user info: {}", e);
            GatewayError::Internal("Failed to parse user info from Google".to_string())
        })
    }
}
