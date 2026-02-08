use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::auth::jwt::JwtService;
use crate::auth::password::hash_password;
use crate::config::Config;
use crate::email::EmailService;
use crate::error::{GatewayError, Result};

/// Query parameters for GET /memberships
#[derive(Debug, Deserialize)]
pub struct ListMembershipsQuery {
    pub platform_code: String,
}

/// Membership info for list response
#[derive(Debug, Serialize)]
pub struct MembershipListItem {
    pub id: String,
    pub platform_code: String,
    pub tenant_id: String,
    pub tenant_slug: String,
    pub tenant_name: String,
    pub role: String,
    pub status: String,
    pub joined_at: String,
}

/// Response body for GET /memberships
#[derive(Debug, Serialize)]
pub struct ListMembershipsResponse {
    pub memberships: Vec<MembershipListItem>,
}

/// GET /memberships - List all memberships for authenticated user
pub async fn list_memberships(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    headers: HeaderMap,
    Query(query): Query<ListMembershipsQuery>,
) -> Result<Json<ListMembershipsResponse>> {
    info!("List memberships request for platform: {}", query.platform_code);

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

    // Parse identity_id
    let identity_id: Uuid = claims.identity_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid identity ID in token".to_string(),
    })?;

    // Get database connection
    let client = pool.get().await?;

    // Query all memberships for this user and platform
    let rows = client
        .query(
            "SELECT tm.id, tm.platform_code, tm.tenant_id, tm.tenant_slug, t.display_name, tm.role, tm.status, tm.created_at
             FROM tenant_memberships tm
             JOIN tenants t ON tm.tenant_id = t.id
             WHERE tm.identity_id = $1 AND tm.platform_code = $2
             ORDER BY tm.created_at ASC",
            &[&identity_id, &query.platform_code],
        )
        .await?;

    let memberships: Vec<MembershipListItem> = rows
        .iter()
        .map(|row| {
            let id: Uuid = row.get(0);
            let platform_code: String = row.get(1);
            let tenant_id: Uuid = row.get(2);
            let tenant_slug: String = row.get(3);
            let tenant_name: String = row.get(4);
            let role: String = row.get(5);
            let status: String = row.get(6);
            let created_at: chrono::NaiveDateTime = row.get(7);

            MembershipListItem {
                id: id.to_string(),
                platform_code,
                tenant_id: tenant_id.to_string(),
                tenant_slug,
                tenant_name,
                role,
                status,
                joined_at: created_at.and_utc().to_rfc3339(),
            }
        })
        .collect();

    info!("Found {} memberships for user", memberships.len());

    Ok(Json(ListMembershipsResponse { memberships }))
}

/// Request body for POST /memberships/invite
#[derive(Debug, Deserialize)]
pub struct InviteMembershipRequest {
    pub email: String,
    pub tenant_id: String,
    pub role: String,
}

/// Response body for POST /memberships/invite
#[derive(Debug, Serialize)]
pub struct InviteMembershipResponse {
    pub invitation_id: String,
    pub email: String,
    pub expires_at: String,
    pub invite_link: String,
}

/// POST /memberships/invite - Invite a user to join a tenant
pub async fn invite_membership(
    State((pool, jwt_service, _config, email_service)): State<(
        Arc<Pool>,
        Arc<JwtService>,
        Arc<Config>,
        Arc<EmailService>,
    )>,
    headers: HeaderMap,
    Json(req): Json<InviteMembershipRequest>,
) -> Result<(StatusCode, Json<InviteMembershipResponse>)> {
    info!("Invite membership request for email: {}", req.email);

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

    // Parse identity_id and tenant_id
    let inviter_id: Uuid = claims.identity_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid identity ID in token".to_string(),
    })?;

    let tenant_id: Uuid = req.tenant_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid tenant ID format".to_string(),
    })?;

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // 1. Check inviter has admin+ role in this tenant
    let inviter_membership = txn
        .query_opt(
            "SELECT role FROM tenant_memberships WHERE identity_id = $1 AND tenant_id = $2 AND status = 'active'",
            &[&inviter_id, &tenant_id],
        )
        .await?;

    let inviter_role = inviter_membership
        .as_ref()
        .map(|row| row.get::<_, String>(0))
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "You do not have membership in this tenant".to_string(),
        })?;

    // Check role is admin or owner
    if inviter_role != "admin" && inviter_role != "owner" {
        return Err(GatewayError::InvalidRequest {
            message: "Only admins and owners can invite members".to_string(),
        });
    }

    // 2. Get tenant info
    let tenant_row = txn
        .query_one(
            "SELECT platform_code, display_name FROM tenants WHERE id = $1",
            &[&tenant_id],
        )
        .await?;

    let platform_code: String = tenant_row.get(0);
    let tenant_name: String = tenant_row.get(1);

    // 3. Check if email already has membership
    let existing_membership = txn
        .query_opt(
            "SELECT tm.id FROM tenant_memberships tm
             JOIN identities i ON tm.identity_id = i.id
             WHERE i.email = $1 AND tm.tenant_id = $2",
            &[&req.email, &tenant_id],
        )
        .await?;

    if existing_membership.is_some() {
        return Err(GatewayError::InvalidRequest {
            message: "User already has a membership in this tenant".to_string(),
        });
    }

    // 4. Generate secure invitation token
    let invitation_token = generate_secure_token();
    let token_hash = hash_invitation_token(&invitation_token);

    // 5. Set expiry to 7 days from now
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);

    // 6. Insert invitation record
    let invitation_row = txn
        .query_one(
            "INSERT INTO invitations (email, platform_code, tenant_id, role, invited_by, token_hash, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id",
            &[
                &req.email,
                &platform_code,
                &tenant_id,
                &req.role,
                &inviter_id,
                &token_hash,
                &expires_at.naive_utc(),
            ],
        )
        .await?;

    let invitation_id: Uuid = invitation_row.get(0);

    // Get inviter's email for the invitation email
    let inviter_row = txn
        .query_one("SELECT email FROM identities WHERE id = $1", &[&inviter_id])
        .await?;

    let inviter_email: String = inviter_row.get(0);

    txn.commit().await?;

    // 7. Generate invite link (using the base URL from config + invitation token)
    // For now, we'll use a placeholder - in production this should be configurable
    let base_url = std::env::var("GATEWAY_BASE_URL").unwrap_or_else(|_| "http://localhost:9000".to_string());
    let invite_link = format!("{}/auth/accept-invite?token={}", base_url, invitation_token);

    // 8. Send invitation email
    if let Err(e) = email_service
        .invitation_email(&req.email, &invite_link, &tenant_name, &inviter_email)
        .await
    {
        error!("Failed to send invitation email: {}", e);
        // Don't fail the request if email fails - the invitation is still valid
    }

    info!("Successfully created invitation {}", invitation_id);

    Ok((
        StatusCode::CREATED,
        Json(InviteMembershipResponse {
            invitation_id: invitation_id.to_string(),
            email: req.email,
            expires_at: expires_at.to_rfc3339(),
            invite_link,
        }),
    ))
}

/// Generate a cryptographically secure random token
fn generate_secure_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    hex::encode(bytes)
}

/// Hash invitation token for storage
fn hash_invitation_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Request body for POST /memberships/accept-invite
#[derive(Debug, Deserialize)]
pub struct AcceptInviteRequest {
    pub token: String,
    #[serde(default)]
    pub password: Option<String>,
}

/// POST /memberships/accept-invite - Accept an invitation and create membership
pub async fn accept_invite(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    Json(req): Json<AcceptInviteRequest>,
) -> Result<Json<crate::api::auth::LoginSuccessResponse>> {
    info!("Accept invite request");

    // Hash the token to look it up
    let token_hash = hash_invitation_token(&req.token);

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // 1. Verify invite token exists and not expired
    let invitation_row = txn
        .query_opt(
            "SELECT id, email, platform_code, tenant_id, role, expires_at, accepted_at
             FROM invitations
             WHERE token_hash = $1",
            &[&token_hash],
        )
        .await?
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "Invalid invitation token".to_string(),
        })?;

    let invitation_id: Uuid = invitation_row.get(0);
    let email: String = invitation_row.get(1);
    let platform_code: String = invitation_row.get(2);
    let tenant_id: Uuid = invitation_row.get(3);
    let role: String = invitation_row.get(4);
    let expires_at: chrono::NaiveDateTime = invitation_row.get(5);
    let accepted_at: Option<chrono::NaiveDateTime> = invitation_row.get(6);

    // Check not already accepted
    if accepted_at.is_some() {
        return Err(GatewayError::InvalidRequest {
            message: "Invitation has already been accepted".to_string(),
        });
    }

    // Check not expired
    if expires_at < chrono::Utc::now().naive_utc() {
        return Err(GatewayError::InvalidRequest {
            message: "Invitation has expired".to_string(),
        });
    }

    // 2. Check if email exists in identities
    let identity_row = txn
        .query_opt("SELECT id, password_hash FROM identities WHERE email = $1", &[&email])
        .await?;

    let identity_id: Uuid = if let Some(row) = identity_row {
        // Identity exists - just create membership
        let identity_id: Uuid = row.get(0);
        info!("Found existing identity for email: {}", email);
        identity_id
    } else {
        // 3. Identity doesn't exist - require password and create it
        let password = req.password.as_ref().ok_or_else(|| GatewayError::InvalidRequest {
            message: "Password is required for new accounts".to_string(),
        })?;

        // Validate password complexity (same as registration)
        if let Err(e) = validate_password_complexity(password) {
            return Err(GatewayError::InvalidRequest { message: e });
        }

        // Hash password
        let password_hash = hash_password(password).map_err(|e| {
            error!("Failed to hash password: {}", e);
            GatewayError::Internal("Failed to hash password".to_string())
        })?;

        // Create identity with 'active' status (verified via invitation)
        let row = txn
            .query_one(
                "INSERT INTO identities (email, password_hash, status)
                 VALUES ($1, $2, 'active')
                 RETURNING id",
                &[&email, &password_hash],
            )
            .await?;

        let identity_id: Uuid = row.get(0);
        info!("Created new identity for email: {}", email);
        identity_id
    };

    // Get tenant info
    let tenant_row = txn
        .query_one(
            "SELECT slug, display_name, tenant_db_schema FROM tenants WHERE id = $1",
            &[&tenant_id],
        )
        .await?;

    let tenant_slug: String = tenant_row.get(0);
    let tenant_name: String = tenant_row.get(1);
    let tenant_db_schema: String = tenant_row.get(2);

    // 4. Create membership
    txn.execute(
        "INSERT INTO tenant_memberships (identity_id, platform_code, tenant_id, tenant_slug, tenant_db_schema, role, status)
         VALUES ($1, $2, $3, $4, $5, $6, 'active')",
        &[
            &identity_id,
            &platform_code,
            &tenant_id,
            &tenant_slug,
            &tenant_db_schema,
            &role,
        ],
    )
    .await?;

    // 5. Mark invitation as accepted
    txn.execute(
        "UPDATE invitations SET accepted_at = NOW() WHERE id = $1",
        &[&invitation_id],
    )
    .await?;

    txn.commit().await?;

    info!(
        "Successfully accepted invitation and created membership for {}",
        email
    );

    // 6. Return login response (same as successful login)
    let claims = crate::auth::jwt::Claims {
        identity_id: identity_id.to_string(),
        tenant_id: tenant_id.to_string(),
        tenant_slug: tenant_slug.clone(),
        platform_code: platform_code.clone(),
        role: role.clone(),
        local_user_id: None,
        exp: 0,
        iat: 0,
    };

    let access_token = jwt_service
        .sign_access_token(claims.clone())
        .map_err(|e| {
            error!("Failed to create access token: {}", e);
            GatewayError::Internal("Failed to create access token".to_string())
        })?;

    let refresh_token = jwt_service.sign_refresh_token(claims).map_err(|e| {
        error!("Failed to create refresh token: {}", e);
        GatewayError::Internal("Failed to create refresh token".to_string())
    })?;

    Ok(Json(crate::api::auth::LoginSuccessResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        identity: crate::api::auth::IdentityInfo {
            id: identity_id.to_string(),
            email,
        },
        membership: crate::api::auth::MembershipInfo {
            tenant_id: tenant_id.to_string(),
            tenant_slug,
            tenant_name,
            role,
            local_user_id: None,
        },
    }))
}

/// Validate password complexity (same as in auth.rs)
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

/// Request body for PUT /memberships/:id
#[derive(Debug, Deserialize)]
pub struct UpdateMembershipRequest {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Response body for PUT /memberships/:id
#[derive(Debug, Serialize)]
pub struct UpdateMembershipResponse {
    pub id: String,
    pub role: String,
    pub status: String,
    pub updated_at: String,
}

/// PUT /memberships/:id - Update membership role or status
pub async fn update_membership(
    State((pool, jwt_service)): State<(Arc<Pool>, Arc<JwtService>)>,
    headers: HeaderMap,
    Path(membership_id): Path<String>,
    Json(req): Json<UpdateMembershipRequest>,
) -> Result<Json<UpdateMembershipResponse>> {
    info!("Update membership request for id: {}", membership_id);

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

    // Parse identity_id and membership_id
    let updater_id: Uuid = claims.identity_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid identity ID in token".to_string(),
    })?;

    let membership_id: Uuid = membership_id.parse().map_err(|_| GatewayError::InvalidRequest {
        message: "Invalid membership ID format".to_string(),
    })?;

    // Validate at least one field is provided
    if req.role.is_none() && req.status.is_none() {
        return Err(GatewayError::InvalidRequest {
            message: "At least one field (role or status) must be provided".to_string(),
        });
    }

    // Get database connection
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;

    // 1. Get the membership being updated
    let target_membership = txn
        .query_opt(
            "SELECT tenant_id, identity_id, role, status FROM tenant_memberships WHERE id = $1",
            &[&membership_id],
        )
        .await?
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "Membership not found".to_string(),
        })?;

    let tenant_id: Uuid = target_membership.get(0);
    let _target_identity_id: Uuid = target_membership.get(1);
    let current_role: String = target_membership.get(2);
    let current_status: String = target_membership.get(3);

    // 2. Check updater has admin+ role in this tenant
    let updater_membership = txn
        .query_opt(
            "SELECT role FROM tenant_memberships WHERE identity_id = $1 AND tenant_id = $2 AND status = 'active'",
            &[&updater_id, &tenant_id],
        )
        .await?;

    let updater_role = updater_membership
        .as_ref()
        .map(|row| row.get::<_, String>(0))
        .ok_or_else(|| GatewayError::InvalidRequest {
            message: "You do not have membership in this tenant".to_string(),
        })?;

    // Check role is admin or owner
    if updater_role != "admin" && updater_role != "owner" {
        return Err(GatewayError::InvalidRequest {
            message: "Only admins and owners can update memberships".to_string(),
        });
    }

    // 3. Build update query dynamically
    let new_role = req.role.as_ref().unwrap_or(&current_role);
    let new_status = req.status.as_ref().unwrap_or(&current_status);

    // Validate role if provided
    if req.role.is_some() {
        let valid_roles = ["member", "admin", "owner"];
        if !valid_roles.contains(&new_role.as_str()) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Invalid role. Must be one of: {}", valid_roles.join(", ")),
            });
        }
    }

    // Validate status if provided
    if req.status.is_some() {
        let valid_statuses = ["active", "suspended"];
        if !valid_statuses.contains(&new_status.as_str()) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Invalid status. Must be one of: {}", valid_statuses.join(", ")),
            });
        }
    }

    // 4. Update membership
    let updated_row = txn
        .query_one(
            "UPDATE tenant_memberships
             SET role = $1, status = $2, updated_at = NOW()
             WHERE id = $3
             RETURNING id, role, status, updated_at",
            &[new_role, new_status, &membership_id],
        )
        .await?;

    let id: Uuid = updated_row.get(0);
    let role: String = updated_row.get(1);
    let status: String = updated_row.get(2);
    let updated_at: chrono::NaiveDateTime = updated_row.get(3);

    txn.commit().await?;

    info!("Successfully updated membership {}", membership_id);

    Ok(Json(UpdateMembershipResponse {
        id: id.to_string(),
        role,
        status,
        updated_at: updated_at.and_utc().to_rfc3339(),
    }))
}
