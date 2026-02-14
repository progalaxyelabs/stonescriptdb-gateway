use axum::{extract::State, http::{StatusCode, HeaderMap}, response::Json};
use deadpool_postgres::Pool;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Sha256, Digest};
use std::sync::Arc;
use tracing::{error, info};

use crate::auth::jwt::{Claims, JwtService};
use crate::auth::password::{hash_password, verify_password};
use crate::auth::oauth::OAuthService;
use crate::config::Config;
use crate::email::EmailService;
use crate::error::{GatewayError, Result};

/// GET /auth/jwks - Return the JSON Web Key Set (public key)
pub async fn get_jwks(State(jwt_service): State<Arc<JwtService>>) -> Json<Value> {
    Json(jwt_service.get_jwks())
}

/// Request body for POST /auth/register
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub platform_code: Option<String>,
    #[serde(default)]
    pub tenant_slug: Option<String>,
}

/// Response body for POST /auth/register
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub identity_id: String,
    pub email: String,
    pub email_verified: bool,
    pub verification_sent: bool,
}

/// POST /auth/register - Register a new user
pub async fn register(
    State(pool): State<Arc<Pool>>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<RegisterResponse>)> {
    info!("Registration request for email: {}", req.email);

    // 1. Validate email format
    if !is_valid_email(&req.email) {
        return Err(GatewayError::InvalidRequest {
            message: "Invalid email format".to_string(),
        });
    }

    // 3. Validate password complexity
    if let Err(e) = validate_password_complexity(&req.password) {
        return Err(GatewayError::InvalidRequest { message: e });
    }

    // Get database connection
    let client = pool.get().await?;

    // 2. Check email not already registered
    let existing = client
        .query_opt("SELECT id FROM identities WHERE email = $1", &[&req.email])
        .await?;

    if existing.is_some() {
        return Err(GatewayError::InvalidRequest {
            message: "Email already registered".to_string(),
        });
    }

    // 6. If platform_code provided, check if tenant_slug exists and is valid
    if let Some(ref platform_code) = req.platform_code {
        if let Some(ref tenant_slug) = req.tenant_slug {
            let tenant_exists = client
                .query_opt(
                    "SELECT id FROM tenants WHERE platform_code = $1 AND slug = $2",
                    &[platform_code, tenant_slug],
                )
                .await?;

            if tenant_exists.is_none() {
                return Err(GatewayError::InvalidRequest {
                    message: format!(
                        "Tenant '{}' does not exist for platform '{}'",
                        tenant_slug, platform_code
                    ),
                });
            }
        }
    }

    // 4. Hash password with bcrypt
    let password_hash = hash_password(&req.password).map_err(|e| {
        error!("Failed to hash password: {}", e);
        GatewayError::Internal("Failed to hash password".to_string())
    })?;

    // 5. Insert into identities table (status='pending' until verified)
    let row = client
        .query_one(
            "INSERT INTO identities (email, password_hash, status)
             VALUES ($1, $2, 'pending')
             RETURNING id",
            &[&req.email, &password_hash],
        )
        .await?;

    let identity_id: uuid::Uuid = row.get(0);

    info!("Successfully registered user: {} ({})", req.email, identity_id);

    // 7. Return 201 on success
    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse {
            identity_id: identity_id.to_string(),
            email: req.email,
            email_verified: false,
            verification_sent: true,
        }),
    ))
}

/// Validate email format using regex
fn is_valid_email(email: &str) -> bool {
    let email_regex = Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$").unwrap();
    email_regex.is_match(email)
}

/// Validate password complexity
/// Requirements: minimum 8 characters, 1 uppercase, 1 lowercase, 1 number
fn validate_password_complexity(password: &str) -> std::result::Result<(), String> {
    if password.len() < 8 {
        return Err("Password must be at least 8 characters long".to_string());
    }

    if !password.chars().any(|c| c.is_uppercase()) {
        return Err("Password must contain at least one uppercase letter".to_string());
    }

    if !password.chars().any(|c| c.is_lowercase()) {
        return Err("Password must contain at least one lowercase letter".to_string());
    }

    if !password.chars().any(|c| c.is_numeric()) {
        return Err("Password must contain at least one number".to_string());
    }

    Ok(())
}

/// Request body for POST /auth/login
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub platform_code: String,
    #[serde(default)]
    pub tenant_slug: Option<String>,
}

/// Identity info in login response
#[derive(Debug, Serialize)]
pub struct IdentityInfo {
    pub id: String,
    pub email: String,
}

/// Membership info in login response
#[derive(Debug, Clone, Serialize)]
pub struct MembershipInfo {
    pub tenant_id: String,
    pub tenant_slug: String,
    pub tenant_name: String,
    pub role: String,
    pub local_user_id: Option<i32>,
}

/// Response when login successful (single tenant or tenant specified)
#[derive(Debug, Serialize)]
pub struct LoginSuccessResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub identity: IdentityInfo,
    pub membership: MembershipInfo,
}

/// Response when multiple tenants available and none specified
#[derive(Debug, Serialize)]
pub struct LoginTenantSelectionResponse {
    pub requires_tenant_selection: bool,
    pub selection_token: String,
    pub memberships: Vec<MembershipInfo>,
}

/// Union response for login
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum LoginResponse {
    Success(LoginSuccessResponse),
    TenantSelection(LoginTenantSelectionResponse),
}

/// POST /auth/login - Login with multi-tenant support
pub async fn login(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>> {
    info!("Login request for email: {}", req.email);

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // 1. Verify email exists in identities
    let identity_row = txn
        .query_opt(
            "SELECT id, email, password_hash, status, last_login_at FROM identities WHERE email = $1",
            &[&req.email],
        )
        .await?;

    let identity_row = identity_row.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Invalid email or password".to_string(),
    })?;

    let identity_id: uuid::Uuid = identity_row.get(0);
    let email: String = identity_row.get(1);
    let password_hash: Option<String> = identity_row.get(2);
    let status: String = identity_row.get(3);

    // 3. Check account not suspended
    if status == "suspended" {
        return Err(GatewayError::InvalidRequest {
            message: "Account is suspended".to_string(),
        });
    }

    // 2. Verify password with bcrypt
    let password_hash = password_hash.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Invalid email or password".to_string(),
    })?;

    let password_valid = verify_password(&req.password, &password_hash).map_err(|e| {
        error!("Password verification error: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid email or password".to_string(),
        }
    })?;

    if !password_valid {
        // 9. Track failed attempts for lockout
        // TODO: Implement failed login tracking (5 failures = 15 min lockout)
        return Err(GatewayError::InvalidRequest {
            message: "Invalid email or password".to_string(),
        });
    }

    // 4. Query tenant_memberships for platform_code
    let memberships_rows = txn
        .query(
            "SELECT tm.tenant_id, tm.tenant_slug, t.display_name, tm.role, tm.local_user_id
             FROM tenant_memberships tm
             JOIN tenants t ON tm.tenant_id = t.id
             WHERE tm.identity_id = $1
             AND tm.platform_code = $2
             AND tm.status = 'active'
             ORDER BY tm.created_at ASC",
            &[&identity_id, &req.platform_code],
        )
        .await?;

    if memberships_rows.is_empty() {
        return Err(GatewayError::InvalidRequest {
            message: format!("No active memberships found for platform '{}'", req.platform_code),
        });
    }

    // Parse all memberships
    let memberships: Vec<_> = memberships_rows
        .iter()
        .map(|row| {
            let tenant_id: uuid::Uuid = row.get(0);
            let tenant_slug: String = row.get(1);
            let tenant_name: String = row.get(2);
            let role: String = row.get(3);
            let local_user_id: Option<i32> = row.get(4);

            MembershipInfo {
                tenant_id: tenant_id.to_string(),
                tenant_slug,
                tenant_name,
                role,
                local_user_id,
            }
        })
        .collect();

    // 5. If tenant_slug provided, validate membership exists
    let selected_membership = if let Some(ref tenant_slug) = req.tenant_slug {
        memberships
            .iter()
            .find(|m| m.tenant_slug == *tenant_slug)
            .ok_or_else(|| GatewayError::InvalidRequest {
                message: format!("No membership found for tenant '{}'", tenant_slug),
            })?
            .clone()
    } else if memberships.len() == 1 {
        // Single membership - auto-select
        memberships[0].clone()
    } else {
        // 6. If multiple memberships and no tenant_slug, return selection response
        // Create a temporary selection token (short-lived, 5 minutes)
        let selection_claims = Claims {
            identity_id: identity_id.to_string(),
            tenant_id: "selection".to_string(),
            tenant_slug: "selection".to_string(),
            platform_code: req.platform_code.clone(),
            role: "selection".to_string(),
            local_user_id: None,
            exp: 0,
            iat: 0,
        };

        let selection_token = jwt_service
            .sign_access_token(selection_claims)
            .map_err(|e| {
                error!("Failed to create selection token: {}", e);
                GatewayError::Internal("Failed to create selection token".to_string())
            })?;

        txn.commit().await?;

        return Ok(Json(LoginResponse::TenantSelection(
            LoginTenantSelectionResponse {
                requires_tenant_selection: true,
                selection_token,
                memberships,
            },
        )));
    };

    // 7. Issue JWT with full claims
    let claims = Claims {
        identity_id: identity_id.to_string(),
        tenant_id: selected_membership.tenant_id.clone(),
        tenant_slug: selected_membership.tenant_slug.clone(),
        platform_code: req.platform_code.clone(),
        role: selected_membership.role.clone(),
        local_user_id: selected_membership.local_user_id.map(|id| id.to_string()),
        exp: 0,
        iat: 0,
    };

    let access_token = jwt_service
        .sign_access_token(claims.clone())
        .map_err(|e| {
            error!("Failed to create access token: {}", e);
            GatewayError::Internal("Failed to create access token".to_string())
        })?;

    let refresh_token = jwt_service
        .sign_refresh_token(claims)
        .map_err(|e| {
            error!("Failed to create refresh token: {}", e);
            GatewayError::Internal("Failed to create refresh token".to_string())
        })?;

    // 8. Update last_login_at
    txn.execute(
        "UPDATE identities SET last_login_at = NOW() WHERE id = $1",
        &[&identity_id],
    )
    .await?;

    txn.commit().await?;

    info!("Successful login for user: {} ({})", email, identity_id);

    Ok(Json(LoginResponse::Success(LoginSuccessResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        identity: IdentityInfo {
            id: identity_id.to_string(),
            email,
        },
        membership: selected_membership,
    })))
}

/// Request body for POST /auth/refresh
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Response body for POST /auth/refresh
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// POST /auth/refresh - Issue a new access token from a refresh token
pub async fn refresh(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>> {
    info!("Token refresh request");

    // 1. Verify refresh token signature and expiry
    let claims = jwt_service.verify_token(&req.refresh_token).map_err(|e| {
        error!("Invalid refresh token: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid or expired refresh token".to_string(),
        }
    })?;

    // Get database connection
    let client = pool.get().await?;

    // 2. Check token not revoked
    let token_hash = hash_token(&req.refresh_token);
    let revoked = client
        .query_opt(
            "SELECT id FROM revoked_tokens WHERE token_hash = $1",
            &[&token_hash],
        )
        .await?;

    if revoked.is_some() {
        return Err(GatewayError::InvalidRequest {
            message: "Token has been revoked".to_string(),
        });
    }

    // 3. Issue new access token with same claims
    let access_token = jwt_service
        .sign_access_token(claims)
        .map_err(|e| {
            error!("Failed to create access token: {}", e);
            GatewayError::Internal("Failed to create access token".to_string())
        })?;

    info!("Successfully refreshed access token");

    Ok(Json(RefreshResponse {
        access_token,
        expires_in: 3600,
    }))
}

/// Request body for POST /auth/logout
#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

/// Response body for POST /auth/logout
#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub success: bool,
}

/// POST /auth/logout - Revoke a refresh token
pub async fn logout(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    Json(req): Json<LogoutRequest>,
) -> Result<Json<LogoutResponse>> {
    info!("Logout request");

    // 1. Try to decode the token to get expiry (for cleanup purposes)
    // We don't fail if token is invalid/expired - we still try to revoke it
    let expires_at = match jwt_service.verify_token(&req.refresh_token) {
        Ok(claims) => {
            // Convert Unix timestamp to PostgreSQL timestamp
            chrono::DateTime::from_timestamp(claims.exp as i64, 0)
                .map(|dt| dt.naive_utc())
        }
        Err(_) => {
            // If token is invalid/expired, set a default expiry in the past
            // This ensures the record will be cleaned up
            Some(chrono::Utc::now().naive_utc())
        }
    };

    let expires_at = expires_at.ok_or_else(|| {
        error!("Failed to parse token expiry");
        GatewayError::Internal("Failed to process token".to_string())
    })?;

    // Get database connection
    let client = pool.get().await?;

    // 2. Mark refresh token as revoked (store hash in revoked_tokens table)
    let token_hash = hash_token(&req.refresh_token);

    // Extract identity_id from token if possible (optional field)
    let identity_id: Option<uuid::Uuid> = jwt_service
        .verify_token(&req.refresh_token)
        .ok()
        .and_then(|claims| claims.identity_id.parse().ok());

    // Insert into revoked_tokens - use ON CONFLICT to handle duplicate revocations
    client
        .execute(
            "INSERT INTO revoked_tokens (token_hash, identity_id, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (token_hash) DO NOTHING",
            &[&token_hash, &identity_id, &expires_at],
        )
        .await?;

    info!("Successfully revoked refresh token");

    // 3. Return success even if token already invalid
    Ok(Json(LogoutResponse { success: true }))
}

/// Hash a token using SHA-256 for storage in revoked_tokens table
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Request body for POST /auth/select-tenant
#[derive(Debug, Deserialize)]
pub struct SelectTenantRequest {
    pub selection_token: String,
    pub tenant_id: String,
}

/// POST /auth/select-tenant - Complete tenant selection after multi-tenant login
pub async fn select_tenant(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    Json(req): Json<SelectTenantRequest>,
) -> Result<Json<LoginSuccessResponse>> {
    info!("Tenant selection request for tenant: {}", req.tenant_id);

    // 1. Verify selection_token (short-lived, ~5 min)
    let selection_claims = jwt_service.verify_token(&req.selection_token).map_err(|e| {
        error!("Invalid selection token: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid or expired selection token".to_string(),
        }
    })?;

    // Verify this is actually a selection token
    if selection_claims.tenant_id != "selection" {
        return Err(GatewayError::InvalidRequest {
            message: "Invalid selection token".to_string(),
        });
    }

    // Parse identity_id and tenant_id
    let identity_id: uuid::Uuid = selection_claims
        .identity_id
        .parse()
        .map_err(|_| GatewayError::InvalidRequest {
            message: "Invalid identity ID in token".to_string(),
        })?;

    let tenant_id: uuid::Uuid = req.tenant_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid tenant ID format".to_string(),
    })?;

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // 2. Verify identity has membership in selected tenant
    let membership_row = txn
        .query_opt(
            "SELECT tm.tenant_id, tm.tenant_slug, t.display_name, tm.role, tm.local_user_id
             FROM tenant_memberships tm
             JOIN tenants t ON tm.tenant_id = t.id
             WHERE tm.identity_id = $1
             AND tm.tenant_id = $2
             AND tm.platform_code = $3
             AND tm.status = 'active'",
            &[&identity_id, &tenant_id, &selection_claims.platform_code],
        )
        .await?;

    let membership_row = membership_row.ok_or_else(|| GatewayError::InvalidRequest {
        message: "No active membership found for selected tenant".to_string(),
    })?;

    let tenant_id_db: uuid::Uuid = membership_row.get(0);
    let tenant_slug: String = membership_row.get(1);
    let tenant_name: String = membership_row.get(2);
    let role: String = membership_row.get(3);
    let local_user_id: Option<i32> = membership_row.get(4);

    // Get identity info
    let identity_row = txn
        .query_one(
            "SELECT email FROM identities WHERE id = $1",
            &[&identity_id],
        )
        .await?;

    let email: String = identity_row.get(0);

    // 3. Issue full JWT with tenant context
    let claims = Claims {
        identity_id: identity_id.to_string(),
        tenant_id: tenant_id_db.to_string(),
        tenant_slug: tenant_slug.clone(),
        platform_code: selection_claims.platform_code.clone(),
        role: role.clone(),
        local_user_id: local_user_id.map(|id| id.to_string()),
        exp: 0,
        iat: 0,
    };

    let access_token = jwt_service
        .sign_access_token(claims.clone())
        .map_err(|e| {
            error!("Failed to create access token: {}", e);
            GatewayError::Internal("Failed to create access token".to_string())
        })?;

    let refresh_token = jwt_service
        .sign_refresh_token(claims)
        .map_err(|e| {
            error!("Failed to create refresh token: {}", e);
            GatewayError::Internal("Failed to create refresh token".to_string())
        })?;

    // Update last_login_at
    txn.execute(
        "UPDATE identities SET last_login_at = NOW() WHERE id = $1",
        &[&identity_id],
    )
    .await?;

    txn.commit().await?;

    info!(
        "Tenant selection completed for identity: {} tenant: {}",
        identity_id, tenant_id
    );

    Ok(Json(LoginSuccessResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        identity: IdentityInfo {
            id: identity_id.to_string(),
            email,
        },
        membership: MembershipInfo {
            tenant_id: tenant_id_db.to_string(),
            tenant_slug,
            tenant_name,
            role,
            local_user_id,
        },
    }))
}

/// Request body for POST /auth/switch-tenant
#[derive(Debug, Deserialize)]
pub struct SwitchTenantRequest {
    pub tenant_id: String,
}

/// POST /auth/switch-tenant - Switch to a different tenant while staying on same platform
pub async fn switch_tenant(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    headers: HeaderMap,
    Json(req): Json<SwitchTenantRequest>,
) -> Result<Json<LoginSuccessResponse>> {
    info!("Tenant switch request to tenant: {}", req.tenant_id);

    // Extract and validate bearer token
    let auth_header = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "Missing Authorization header".to_string(),
        })?;

    if !auth_header.starts_with("Bearer ") {
        return Err(GatewayError::InvalidRequest {
            message: "Invalid Authorization header format".to_string(),
        });
    }

    let token = &auth_header[7..]; // Skip "Bearer "

    // 1. Verify current JWT
    let current_claims = jwt_service.verify_token(token).map_err(|e| {
        error!("Invalid access token: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid or expired access token".to_string(),
        }
    })?;

    // Parse identity_id and new tenant_id
    let identity_id: uuid::Uuid = current_claims
        .identity_id
        .parse()
        .map_err(|_| GatewayError::InvalidRequest {
            message: "Invalid identity ID in token".to_string(),
        })?;

    let new_tenant_id: uuid::Uuid = req.tenant_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid tenant ID format".to_string(),
    })?;

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // 2. Verify identity has membership in target tenant (same platform)
    let membership_row = txn
        .query_opt(
            "SELECT tm.tenant_id, tm.tenant_slug, t.display_name, tm.role, tm.local_user_id
             FROM tenant_memberships tm
             JOIN tenants t ON tm.tenant_id = t.id
             WHERE tm.identity_id = $1
             AND tm.tenant_id = $2
             AND tm.platform_code = $3
             AND tm.status = 'active'",
            &[&identity_id, &new_tenant_id, &current_claims.platform_code],
        )
        .await?;

    let membership_row = membership_row.ok_or_else(|| GatewayError::InvalidRequest {
        message: "No active membership found for target tenant on this platform".to_string(),
    })?;

    let tenant_id_db: uuid::Uuid = membership_row.get(0);
    let tenant_slug: String = membership_row.get(1);
    let tenant_name: String = membership_row.get(2);
    let role: String = membership_row.get(3);
    let local_user_id: Option<i32> = membership_row.get(4);

    // Get identity info
    let identity_row = txn
        .query_one(
            "SELECT email FROM identities WHERE id = $1",
            &[&identity_id],
        )
        .await?;

    let email: String = identity_row.get(0);

    // 3. Issue new JWT with updated tenant context
    let claims = Claims {
        identity_id: identity_id.to_string(),
        tenant_id: tenant_id_db.to_string(),
        tenant_slug: tenant_slug.clone(),
        platform_code: current_claims.platform_code.clone(),
        role: role.clone(),
        local_user_id: local_user_id.map(|id| id.to_string()),
        exp: 0,
        iat: 0,
    };

    let access_token = jwt_service
        .sign_access_token(claims.clone())
        .map_err(|e| {
            error!("Failed to create access token: {}", e);
            GatewayError::Internal("Failed to create access token".to_string())
        })?;

    let refresh_token = jwt_service
        .sign_refresh_token(claims)
        .map_err(|e| {
            error!("Failed to create refresh token: {}", e);
            GatewayError::Internal("Failed to create refresh token".to_string())
        })?;

    txn.commit().await?;

    info!(
        "Tenant switch completed for identity: {} to tenant: {}",
        identity_id, new_tenant_id
    );

    Ok(Json(LoginSuccessResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        identity: IdentityInfo {
            id: identity_id.to_string(),
            email,
        },
        membership: MembershipInfo {
            tenant_id: tenant_id_db.to_string(),
            tenant_slug,
            tenant_name,
            role,
            local_user_id,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_email() {
        assert!(is_valid_email("user@example.com"));
        assert!(is_valid_email("test.user+tag@example.co.uk"));
        assert!(!is_valid_email("invalid"));
        assert!(!is_valid_email("@example.com"));
        assert!(!is_valid_email("user@"));
        assert!(!is_valid_email("user@example"));
    }

    #[test]
    fn test_validate_password_complexity() {
        // Valid passwords
        assert!(validate_password_complexity("Password123").is_ok());
        assert!(validate_password_complexity("MyP@ss1234").is_ok());

        // Too short
        assert!(validate_password_complexity("Pass1").is_err());

        // Missing uppercase
        assert!(validate_password_complexity("password123").is_err());

        // Missing lowercase
        assert!(validate_password_complexity("PASSWORD123").is_err());

        // Missing number
        assert!(validate_password_complexity("Password").is_err());
    }
}

// ============================================================================
// Account Management Endpoints
// ============================================================================

/// Request body for POST /account/password-reset/request
#[derive(Debug, Deserialize)]
pub struct PasswordResetRequestRequest {
    pub email: String,
}

/// Response body for POST /account/password-reset/request
#[derive(Debug, Serialize)]
pub struct PasswordResetRequestResponse {
    pub message: String,
    pub expires_in_minutes: u32,
}

/// POST /account/password-reset/request - Request a password reset
pub async fn password_reset_request(
    State((pool, config, email_service)): State<(Arc<Pool>, Arc<Config>, Arc<EmailService>)>,
    Json(req): Json<PasswordResetRequestRequest>,
) -> Result<Json<PasswordResetRequestResponse>> {
    info!("Password reset request for email: {}", req.email);

    // Get database connection
    let client = pool.get().await?;

    // Check if identity exists
    let identity_row = client
        .query_opt(
            "SELECT id FROM identities WHERE email = $1",
            &[&req.email],
        )
        .await?;

    // If account exists, generate token and send email
    if let Some(row) = identity_row {
        let identity_id: uuid::Uuid = row.get(0);

        // Generate secure random token (32 bytes = 64 hex chars)
        // Scoped so ThreadRng (!Send) doesn't live across .await
        let token = {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let token_bytes: [u8; 32] = rng.gen();
            hex::encode(token_bytes)
        };

        // Hash the token for storage
        let token_hash = hash_token(&token);

        // Set expiry to 60 minutes from now
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(60);

        // Store hashed token in database
        client
            .execute(
                "INSERT INTO password_reset_tokens (identity_id, token_hash, expires_at)
                 VALUES ($1, $2, $3)",
                &[&identity_id, &token_hash, &expires_at.naive_utc()],
            )
            .await?;

        // Construct reset link
        let reset_link = format!(
            "{}/reset-password?token={}",
            config.frontend_url.as_deref().unwrap_or("http://localhost:3000"),
            token
        );

        // Send password reset email
        if let Err(e) = email_service.password_reset_email(&req.email, &reset_link).await {
            error!("Failed to send password reset email: {}", e);
            // Continue anyway - don't reveal if email was sent
        }

        info!("Password reset token generated for identity: {}", identity_id);
    } else {
        info!("Password reset requested for non-existent email (no action taken)");
    }

    // Always return success to prevent email enumeration
    Ok(Json(PasswordResetRequestResponse {
        message: "If account exists, reset email sent".to_string(),
        expires_in_minutes: 60,
    }))
}

/// Request body for POST /account/password-reset/confirm
#[derive(Debug, Deserialize)]
pub struct PasswordResetConfirmRequest {
    pub token: String,
    pub new_password: String,
}

/// Response body for POST /account/password-reset/confirm
#[derive(Debug, Serialize)]
pub struct PasswordResetConfirmResponse {
    pub success: bool,
    pub message: String,
}

/// POST /account/password-reset/confirm - Confirm password reset with token
pub async fn password_reset_confirm(
    State(pool): State<Arc<Pool>>,
    Json(req): Json<PasswordResetConfirmRequest>,
) -> Result<Json<PasswordResetConfirmResponse>> {
    info!("Password reset confirmation request");

    // Validate new password complexity
    if let Err(e) = validate_password_complexity(&req.new_password) {
        return Err(GatewayError::InvalidRequest { message: e });
    }

    // Hash the token to look it up
    let token_hash = hash_token(&req.token);

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // Verify token hash and expiry
    let token_row = txn
        .query_opt(
            "SELECT identity_id, expires_at, used_at FROM password_reset_tokens
             WHERE token_hash = $1",
            &[&token_hash],
        )
        .await?;

    let token_row = token_row.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Invalid or expired reset token".to_string(),
    })?;

    let identity_id: uuid::Uuid = token_row.get(0);
    let expires_at: chrono::NaiveDateTime = token_row.get(1);
    let used_at: Option<chrono::NaiveDateTime> = token_row.get(2);

    // Check if token already used
    if used_at.is_some() {
        return Err(GatewayError::InvalidRequest {
            message: "Reset token has already been used".to_string(),
        });
    }

    // Check if token expired
    let now = chrono::Utc::now().naive_utc();
    if expires_at < now {
        return Err(GatewayError::InvalidRequest {
            message: "Reset token has expired".to_string(),
        });
    }

    // Hash new password
    let password_hash = hash_password(&req.new_password).map_err(|e| {
        error!("Failed to hash password: {}", e);
        GatewayError::Internal("Failed to hash password".to_string())
    })?;

    // Update password
    txn.execute(
        "UPDATE identities SET password_hash = $1 WHERE id = $2",
        &[&password_hash, &identity_id],
    )
    .await?;

    // Mark token as used
    txn.execute(
        "UPDATE password_reset_tokens SET used_at = NOW() WHERE token_hash = $1",
        &[&token_hash],
    )
    .await?;

    txn.commit().await?;

    info!("Password successfully reset for identity: {}", identity_id);

    Ok(Json(PasswordResetConfirmResponse {
        success: true,
        message: "Password successfully reset".to_string(),
    }))
}

/// Request body for PUT /account/password
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Response body for PUT /account/password
#[derive(Debug, Serialize)]
pub struct ChangePasswordResponse {
    pub success: bool,
    pub message: String,
}

/// PUT /account/password - Change password (requires authentication)
pub async fn change_password(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<ChangePasswordResponse>> {
    info!("Password change request");

    // Extract and validate bearer token
    let auth_header = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "Missing Authorization header".to_string(),
        })?;

    if !auth_header.starts_with("Bearer ") {
        return Err(GatewayError::InvalidRequest {
            message: "Invalid Authorization header format".to_string(),
        });
    }

    let token = &auth_header[7..]; // Skip "Bearer "

    // Verify JWT
    let claims = jwt_service.verify_token(token).map_err(|e| {
        error!("Invalid access token: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid or expired access token".to_string(),
        }
    })?;

    // Parse identity_id from claims
    let identity_id: uuid::Uuid = claims
        .identity_id
        .parse()
        .map_err(|_| GatewayError::InvalidRequest {
            message: "Invalid identity ID in token".to_string(),
        })?;

    // Validate new password complexity
    if let Err(e) = validate_password_complexity(&req.new_password) {
        return Err(GatewayError::InvalidRequest { message: e });
    }

    // Get database connection
    let client = pool.get().await?;

    // Get current password hash
    let identity_row = client
        .query_one(
            "SELECT password_hash FROM identities WHERE id = $1",
            &[&identity_id],
        )
        .await?;

    let current_password_hash: Option<String> = identity_row.get(0);

    // Verify current password
    let current_password_hash = current_password_hash.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Account does not have a password set (OAuth-only account)".to_string(),
    })?;

    let password_valid = verify_password(&req.current_password, &current_password_hash).map_err(|e| {
        error!("Password verification error: {}", e);
        GatewayError::InvalidRequest {
            message: "Current password is incorrect".to_string(),
        }
    })?;

    if !password_valid {
        return Err(GatewayError::InvalidRequest {
            message: "Current password is incorrect".to_string(),
        });
    }

    // Hash new password
    let new_password_hash = hash_password(&req.new_password).map_err(|e| {
        error!("Failed to hash password: {}", e);
        GatewayError::Internal("Failed to hash password".to_string())
    })?;

    // Update password
    client
        .execute(
            "UPDATE identities SET password_hash = $1 WHERE id = $2",
            &[&new_password_hash, &identity_id],
        )
        .await?;

    info!("Password successfully changed for identity: {}", identity_id);

    Ok(Json(ChangePasswordResponse {
        success: true,
        message: "Password successfully changed".to_string(),
    }))
}

/// OAuth connection info
#[derive(Debug, Serialize)]
pub struct OAuthConnectionInfo {
    pub provider: String,
    pub provider_email: String,
    pub connected_at: String,
}

/// Response body for GET /account/oauth-connections
#[derive(Debug, Serialize)]
pub struct OAuthConnectionsResponse {
    pub connections: Vec<OAuthConnectionInfo>,
}

/// GET /account/oauth-connections - List OAuth connections for authenticated user
pub async fn get_oauth_connections(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    headers: HeaderMap,
) -> Result<Json<OAuthConnectionsResponse>> {
    info!("Get OAuth connections request");

    // Extract and validate bearer token
    let auth_header = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "Missing Authorization header".to_string(),
        })?;

    if !auth_header.starts_with("Bearer ") {
        return Err(GatewayError::InvalidRequest {
            message: "Invalid Authorization header format".to_string(),
        });
    }

    let token = &auth_header[7..]; // Skip "Bearer "

    // Verify JWT
    let claims = jwt_service.verify_token(token).map_err(|e| {
        error!("Invalid access token: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid or expired access token".to_string(),
        }
    })?;

    // Parse identity_id from claims
    let identity_id: uuid::Uuid = claims
        .identity_id
        .parse()
        .map_err(|_| GatewayError::InvalidRequest {
            message: "Invalid identity ID in token".to_string(),
        })?;

    // Get database connection
    let client = pool.get().await?;

    // Query OAuth connections
    let rows = client
        .query(
            "SELECT provider, provider_email, created_at
             FROM oauth_connections
             WHERE identity_id = $1
             ORDER BY created_at ASC",
            &[&identity_id],
        )
        .await?;

    let connections: Vec<OAuthConnectionInfo> = rows
        .iter()
        .map(|row| {
            let provider: String = row.get(0);
            let provider_email: String = row.get(1);
            let created_at: chrono::NaiveDateTime = row.get(2);

            OAuthConnectionInfo {
                provider,
                provider_email,
                connected_at: created_at.to_string(),
            }
        })
        .collect();

    info!(
        "Retrieved {} OAuth connections for identity: {}",
        connections.len(),
        identity_id
    );

    Ok(Json(OAuthConnectionsResponse { connections }))
}

/// Response body for DELETE /account/oauth-connections/:provider
#[derive(Debug, Serialize)]
pub struct DeleteOAuthConnectionResponse {
    pub success: bool,
    pub message: String,
}

/// DELETE /account/oauth-connections/:provider - Remove OAuth connection
pub async fn delete_oauth_connection(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    headers: HeaderMap,
    axum::extract::Path(provider): axum::extract::Path<String>,
) -> Result<Json<DeleteOAuthConnectionResponse>> {
    info!("Delete OAuth connection request for provider: {}", provider);

    // Extract and validate bearer token
    let auth_header = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "Missing Authorization header".to_string(),
        })?;

    if !auth_header.starts_with("Bearer ") {
        return Err(GatewayError::InvalidRequest {
            message: "Invalid Authorization header format".to_string(),
        });
    }

    let token = &auth_header[7..]; // Skip "Bearer "

    // Verify JWT
    let claims = jwt_service.verify_token(token).map_err(|e| {
        error!("Invalid access token: {}", e);
        GatewayError::InvalidRequest {
            message: "Invalid or expired access token".to_string(),
        }
    })?;

    // Parse identity_id from claims
    let identity_id: uuid::Uuid = claims
        .identity_id
        .parse()
        .map_err(|_| GatewayError::InvalidRequest {
            message: "Invalid identity ID in token".to_string(),
        })?;

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // Check if user has a password OR another OAuth connection
    // (must have at least one auth method)
    let identity_row = txn
        .query_one(
            "SELECT password_hash FROM identities WHERE id = $1",
            &[&identity_id],
        )
        .await?;

    let has_password: bool = identity_row.get::<_, Option<String>>(0).is_some();

    let other_connections_count: i64 = txn
        .query_one(
            "SELECT COUNT(*) FROM oauth_connections
             WHERE identity_id = $1 AND provider != $2",
            &[&identity_id, &provider],
        )
        .await?
        .get(0);

    // Ensure user has another auth method
    if !has_password && other_connections_count == 0 {
        return Err(GatewayError::InvalidRequest {
            message: "Cannot remove last authentication method. Please set a password first or add another OAuth connection.".to_string(),
        });
    }

    // Delete OAuth connection
    let deleted = txn
        .execute(
            "DELETE FROM oauth_connections WHERE identity_id = $1 AND provider = $2",
            &[&identity_id, &provider],
        )
        .await?;

    if deleted == 0 {
        return Err(GatewayError::InvalidRequest {
            message: format!("No OAuth connection found for provider '{}'", provider),
        });
    }

    txn.commit().await?;

    info!(
        "OAuth connection removed for identity: {}, provider: {}",
        identity_id, provider
    );

    Ok(Json(DeleteOAuthConnectionResponse {
        success: true,
        message: format!("OAuth connection for '{}' successfully removed", provider),
    }))
}

// ============================================================================
// OAuth Endpoints
// ============================================================================

/// Request body for POST /auth/oauth/initiate
#[derive(Debug, Deserialize)]
pub struct OAuthInitiateRequest {
    pub provider: String,
    pub platform_code: String,
    #[serde(default)]
    pub tenant_slug: Option<String>,
    pub redirect_uri: String,
}

/// Response body for POST /auth/oauth/initiate
#[derive(Debug, Serialize)]
pub struct OAuthInitiateResponse {
    pub authorization_url: String,
}

/// POST /auth/oauth/initiate - Initiate OAuth flow
pub async fn oauth_initiate(
    State((config, oauth_service)): State<(Arc<Config>, Arc<OAuthService>)>,
    Json(req): Json<OAuthInitiateRequest>,
) -> Result<Json<OAuthInitiateResponse>> {
    info!(
        "OAuth initiate request for provider: {}, platform: {}",
        req.provider, req.platform_code
    );

    // Validate provider
    if req.provider.to_lowercase() != "google" {
        return Err(GatewayError::InvalidRequest {
            message: "Only 'google' provider is supported".to_string(),
        });
    }

    // Get OAuth credentials from config
    let client_id = config.google_client_id.as_ref().ok_or_else(|| {
        error!("Google OAuth not configured: missing GOOGLE_CLIENT_ID");
        GatewayError::Internal("OAuth not configured".to_string())
    })?;

    let client_secret = config.google_client_secret.as_ref().ok_or_else(|| {
        error!("Google OAuth not configured: missing GOOGLE_CLIENT_SECRET");
        GatewayError::Internal("OAuth not configured".to_string())
    })?;

    // Generate authorization URL
    let authorization_url = oauth_service.generate_google_auth_url(
        client_id,
        client_secret,
        &req.redirect_uri,
        req.platform_code,
        req.tenant_slug,
    )?;

    info!("Generated OAuth authorization URL");

    Ok(Json(OAuthInitiateResponse { authorization_url }))
}

/// Request body for POST /auth/oauth/callback
#[derive(Debug, Deserialize)]
pub struct OAuthCallbackRequest {
    pub provider: String,
    pub code: String,
    pub state: String,
}

/// POST /auth/oauth/callback - Handle OAuth callback
pub async fn oauth_callback(
    State((pool, jwt_service, oauth_service, config)): State<(
        Arc<Pool>,
        Arc<JwtService>,
        Arc<OAuthService>,
        Arc<Config>,
    )>,
    Json(req): Json<OAuthCallbackRequest>,
) -> Result<Json<LoginResponse>> {
    info!("OAuth callback request for provider: {}", req.provider);

    // Validate provider
    if req.provider.to_lowercase() != "google" {
        return Err(GatewayError::InvalidRequest {
            message: "Only 'google' provider is supported".to_string(),
        });
    }

    // Verify state token and get context
    let state_context = oauth_service.verify_state(&req.state)?;

    // Get OAuth credentials from config
    let client_id = config.google_client_id.as_ref().ok_or_else(|| {
        error!("Google OAuth not configured: missing GOOGLE_CLIENT_ID");
        GatewayError::Internal("OAuth not configured".to_string())
    })?;

    let client_secret = config.google_client_secret.as_ref().ok_or_else(|| {
        error!("Google OAuth not configured: missing GOOGLE_CLIENT_SECRET");
        GatewayError::Internal("OAuth not configured".to_string())
    })?;

    // Exchange code for tokens and get user info
    let (token_response, user_info) = oauth_service
        .exchange_google_code(
            client_id,
            client_secret,
            &state_context.redirect_uri,
            &req.code,
        )
        .await?;

    info!(
        "Retrieved Google user info: {} ({})",
        user_info.email, user_info.id
    );

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // Check if email exists in identities
    let identity_row = txn
        .query_opt(
            "SELECT id, email, status FROM identities WHERE email = $1",
            &[&user_info.email],
        )
        .await?;

    let identity_id: uuid::Uuid = if let Some(row) = identity_row {
        // Identity exists - link OAuth connection if not already linked
        let identity_id: uuid::Uuid = row.get(0);
        let status: String = row.get(2);

        // Check account not suspended
        if status == "suspended" {
            return Err(GatewayError::InvalidRequest {
                message: "Account is suspended".to_string(),
            });
        }

        // Check if OAuth connection already exists
        let existing_connection = txn
            .query_opt(
                "SELECT id FROM oauth_connections WHERE identity_id = $1 AND provider = $2 AND provider_user_id = $3",
                &[&identity_id, &"google", &user_info.id],
            )
            .await?;

        if existing_connection.is_none() {
            // Create OAuth connection
            use oauth2::TokenResponse as _;
            let access_token_bytes = token_response.access_token().secret().as_bytes();
            let refresh_token_bytes = token_response
                .refresh_token()
                .map(|t| t.secret().as_bytes().to_vec());
            let token_expires_at = token_response.expires_in().map(|duration| {
                chrono::Utc::now() + chrono::Duration::from_std(duration).unwrap()
            });

            txn.execute(
                "INSERT INTO oauth_connections (identity_id, provider, provider_user_id, provider_email, access_token_encrypted, refresh_token_encrypted, token_expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &identity_id,
                    &"google",
                    &user_info.id,
                    &user_info.email,
                    &access_token_bytes,
                    &refresh_token_bytes,
                    &token_expires_at,
                ],
            )
            .await?;

            info!("Linked OAuth connection to existing identity: {}", identity_id);
        } else {
            info!("OAuth connection already exists for identity: {}", identity_id);
        }

        identity_id
    } else {
        // Create new identity with 'active' status (OAuth verified)
        let row = txn
            .query_one(
                "INSERT INTO identities (email, password_hash, status)
                 VALUES ($1, NULL, 'active')
                 RETURNING id",
                &[&user_info.email],
            )
            .await?;

        let identity_id: uuid::Uuid = row.get(0);

        // Create OAuth connection
        use oauth2::TokenResponse as _;
        let access_token_bytes = token_response.access_token().secret().as_bytes();
        let refresh_token_bytes = token_response
            .refresh_token()
            .map(|t| t.secret().as_bytes().to_vec());
        let token_expires_at = token_response.expires_in().map(|duration| {
            chrono::Utc::now() + chrono::Duration::from_std(duration).unwrap()
        });

        txn.execute(
            "INSERT INTO oauth_connections (identity_id, provider, provider_user_id, provider_email, access_token_encrypted, refresh_token_encrypted, token_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[
                &identity_id,
                &"google",
                &user_info.id,
                &user_info.email,
                &access_token_bytes,
                &refresh_token_bytes,
                &token_expires_at,
            ],
        )
        .await?;

        info!(
            "Created new identity and OAuth connection: {}",
            identity_id
        );

        identity_id
    };

    // Handle multi-tenant login (same as password login)
    // Query tenant_memberships for platform_code
    let memberships_rows = txn
        .query(
            "SELECT tm.tenant_id, tm.tenant_slug, t.display_name, tm.role, tm.local_user_id
             FROM tenant_memberships tm
             JOIN tenants t ON tm.tenant_id = t.id
             WHERE tm.identity_id = $1
             AND tm.platform_code = $2
             AND tm.status = 'active'
             ORDER BY tm.created_at ASC",
            &[&identity_id, &state_context.platform_code],
        )
        .await?;

    if memberships_rows.is_empty() {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "No active memberships found for platform '{}'",
                state_context.platform_code
            ),
        });
    }

    // Parse all memberships
    let memberships: Vec<_> = memberships_rows
        .iter()
        .map(|row| {
            let tenant_id: uuid::Uuid = row.get(0);
            let tenant_slug: String = row.get(1);
            let tenant_name: String = row.get(2);
            let role: String = row.get(3);
            let local_user_id: Option<i32> = row.get(4);

            MembershipInfo {
                tenant_id: tenant_id.to_string(),
                tenant_slug,
                tenant_name,
                role,
                local_user_id,
            }
        })
        .collect();

    // If tenant_slug provided in state, validate membership exists
    let selected_membership = if let Some(ref tenant_slug) = state_context.tenant_slug {
        memberships
            .iter()
            .find(|m| m.tenant_slug == *tenant_slug)
            .ok_or_else(|| GatewayError::InvalidRequest {
                message: format!("No membership found for tenant '{}'", tenant_slug),
            })?
            .clone()
    } else if memberships.len() == 1 {
        // Single membership - auto-select
        memberships[0].clone()
    } else {
        // Multiple memberships and no tenant_slug - return selection response
        let selection_claims = Claims {
            identity_id: identity_id.to_string(),
            tenant_id: "selection".to_string(),
            tenant_slug: "selection".to_string(),
            platform_code: state_context.platform_code.clone(),
            role: "selection".to_string(),
            local_user_id: None,
            exp: 0,
            iat: 0,
        };

        let selection_token = jwt_service
            .sign_access_token(selection_claims)
            .map_err(|e| {
                error!("Failed to create selection token: {}", e);
                GatewayError::Internal("Failed to create selection token".to_string())
            })?;

        txn.commit().await?;

        return Ok(Json(LoginResponse::TenantSelection(
            LoginTenantSelectionResponse {
                requires_tenant_selection: true,
                selection_token,
                memberships,
            },
        )));
    };

    // Issue JWT with full claims
    let claims = Claims {
        identity_id: identity_id.to_string(),
        tenant_id: selected_membership.tenant_id.clone(),
        tenant_slug: selected_membership.tenant_slug.clone(),
        platform_code: state_context.platform_code.clone(),
        role: selected_membership.role.clone(),
        local_user_id: selected_membership.local_user_id.map(|id| id.to_string()),
        exp: 0,
        iat: 0,
    };

    let access_token = jwt_service
        .sign_access_token(claims.clone())
        .map_err(|e| {
            error!("Failed to create access token: {}", e);
            GatewayError::Internal("Failed to create access token".to_string())
        })?;

    let refresh_token = jwt_service
        .sign_refresh_token(claims)
        .map_err(|e| {
            error!("Failed to create refresh token: {}", e);
            GatewayError::Internal("Failed to create refresh token".to_string())
        })?;

    // Update last_login_at
    txn.execute(
        "UPDATE identities SET last_login_at = NOW() WHERE id = $1",
        &[&identity_id],
    )
    .await?;

    txn.commit().await?;

    info!(
        "Successful OAuth login for user: {} ({})",
        user_info.email, identity_id
    );

    Ok(Json(LoginResponse::Success(LoginSuccessResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        identity: IdentityInfo {
            id: identity_id.to_string(),
            email: user_info.email,
        },
        membership: selected_membership,
    })))
}
