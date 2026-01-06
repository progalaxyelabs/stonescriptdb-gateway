//! Platform API endpoints
//!
//! - POST /platform/register - Register a new platform
//! - POST /platform/{platform}/schema - Register a schema for a platform
//! - GET /platform/{platform}/schemas - List schemas for a platform
//! - GET /platform/{platform}/databases - List databases for a platform
//! - GET /platforms - List all registered platforms

use crate::error::{GatewayError, Result};
use crate::registry::{PlatformRegistry, SchemaStore};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Multipart;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

/// Shared state for platform endpoints
pub struct PlatformState {
    pub registry: PlatformRegistry,
    pub schema_store: SchemaStore,
}

impl PlatformState {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            registry: PlatformRegistry::new(data_dir),
            schema_store: SchemaStore::new(data_dir),
        }
    }
}

// === Register Platform ===

#[derive(Debug, Deserialize)]
pub struct RegisterPlatformRequest {
    pub platform: String,
    /// Optional: PostgreSQL username for platform-specific database isolation
    /// If not provided, uses the default gateway user (less secure)
    pub db_user: Option<String>,
    /// Optional: PostgreSQL password for platform-specific database isolation
    pub db_password: Option<String>,
}

#[derive(Serialize)]
pub struct RegisterPlatformResponse {
    pub status: String,
    pub platform: String,
    pub message: String,
    pub has_dedicated_credentials: bool,
}

pub async fn register_platform(
    State(state): State<Arc<PlatformState>>,
    Json(request): Json<RegisterPlatformRequest>,
) -> Result<impl IntoResponse> {
    // Register platform with optional credentials
    let info = if let (Some(db_user), Some(db_password)) = (request.db_user, request.db_password) {
        // Validate credentials are not empty
        if db_user.is_empty() || db_password.is_empty() {
            return Err(GatewayError::InvalidRequest {
                message: "db_user and db_password must not be empty".to_string(),
            });
        }

        let mut info = state.registry.register_platform(&request.platform)?;
        info.db_user = Some(db_user);
        info.db_password = Some(db_password);
        state.registry.save_platform_info(&info)?;
        info
    } else {
        state.registry.register_platform(&request.platform)?
    };

    let has_dedicated_credentials = info.db_user.is_some();

    let message = if has_dedicated_credentials {
        "Platform registered with dedicated PostgreSQL credentials. Database isolation enabled.".to_string()
    } else {
        "Platform registered using default gateway credentials. For better security, provide db_user and db_password.".to_string()
    };

    Ok((
        StatusCode::CREATED,
        Json(RegisterPlatformResponse {
            status: "registered".to_string(),
            platform: info.name,
            message,
            has_dedicated_credentials,
        }),
    ))
}

// === Register Schema ===

#[derive(Serialize)]
pub struct RegisterSchemaResponse {
    pub status: String,
    pub platform: String,
    pub schema_name: String,
    pub has_tables: bool,
    pub has_functions: bool,
    pub has_migrations: bool,
    pub checksum: String,
}

pub async fn register_schema(
    State(state): State<Arc<PlatformState>>,
    Path(platform): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse> {
    // Check platform is registered
    if !state.registry.is_registered(&platform) {
        return Err(GatewayError::InvalidRequest {
            message: format!("Platform '{}' is not registered. Register it first.", platform),
        });
    }

    let mut schema_name: Option<String> = None;
    let mut schema_data: Option<Vec<u8>> = None;

    // Parse multipart form
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        GatewayError::InvalidRequest {
            message: format!("Failed to parse multipart form: {}", e),
        }
    })? {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "schema_name" | "name" => {
                schema_name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to read schema_name field: {}", e),
                        })?,
                );
            }
            "schema" | "file" => {
                schema_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to read schema file: {}", e),
                        })?
                        .to_vec(),
                );
            }
            _ => {
                warn!("Unknown field in multipart: {}", name);
            }
        }
    }

    // Validate required fields
    let schema_name = schema_name.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Missing required field: schema_name".to_string(),
    })?;

    let schema_data = schema_data.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Missing required field: schema (tar.gz file)".to_string(),
    })?;

    // Store schema
    let stored = state.schema_store.store_schema(&platform, &schema_name, &schema_data)?;

    // Update platform info
    state.registry.add_schema(&platform, &schema_name)?;

    info!("Registered schema '{}' for platform '{}'", schema_name, platform);

    Ok((
        StatusCode::CREATED,
        Json(RegisterSchemaResponse {
            status: "registered".to_string(),
            platform,
            schema_name: stored.name,
            has_tables: stored.has_tables,
            has_functions: stored.has_functions,
            has_migrations: stored.has_migrations,
            checksum: stored.checksum,
        }),
    ))
}

// === List Schemas ===

#[derive(Serialize)]
pub struct SchemaInfo {
    pub name: String,
    pub has_tables: bool,
    pub has_functions: bool,
    pub has_migrations: bool,
    pub has_seeders: bool,
}

#[derive(Serialize)]
pub struct ListSchemasResponse {
    pub platform: String,
    pub schemas: Vec<SchemaInfo>,
    pub count: usize,
}

pub async fn list_schemas(
    State(state): State<Arc<PlatformState>>,
    Path(platform): Path<String>,
) -> Result<impl IntoResponse> {
    // Check platform is registered
    if !state.registry.is_registered(&platform) {
        return Err(GatewayError::InvalidRequest {
            message: format!("Platform '{}' is not registered", platform),
        });
    }

    let schema_names = state.schema_store.list_schemas(&platform)?;

    let mut schemas = Vec::new();
    for name in &schema_names {
        if let Ok(schema) = state.schema_store.get_schema(&platform, name) {
            schemas.push(SchemaInfo {
                name: schema.name,
                has_tables: schema.has_tables,
                has_functions: schema.has_functions,
                has_migrations: schema.has_migrations,
                has_seeders: schema.has_seeders,
            });
        }
    }

    let count = schemas.len();

    Ok((
        StatusCode::OK,
        Json(ListSchemasResponse {
            platform,
            schemas,
            count,
        }),
    ))
}

// === List Databases ===

#[derive(Debug, Deserialize)]
pub struct ListDatabasesQuery {
    pub schema: Option<String>,
}

#[derive(Serialize)]
pub struct DatabaseInfo {
    pub id: String,
    pub database_name: String,
    pub schema_name: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ListDatabasesResponse {
    pub platform: String,
    pub databases: Vec<DatabaseInfo>,
    pub count: usize,
}

pub async fn list_databases(
    State(state): State<Arc<PlatformState>>,
    Path(platform): Path<String>,
    Query(query): Query<ListDatabasesQuery>,
) -> Result<impl IntoResponse> {
    // Check platform is registered
    if !state.registry.is_registered(&platform) {
        return Err(GatewayError::InvalidRequest {
            message: format!("Platform '{}' is not registered", platform),
        });
    }

    let records = state.registry.list_databases(&platform, query.schema.as_deref())?;

    let databases: Vec<DatabaseInfo> = records.iter().map(|r| {
        // Extract ID from database name (format: platform_schema_id)
        let id = r.database_name
            .strip_prefix(&format!("{}_", platform))
            .and_then(|s| {
                // Strip schema prefix if present
                let schema_prefix = format!("{}_", r.schema_name.trim_end_matches("_db"));
                s.strip_prefix(&schema_prefix).or(Some(s))
            })
            .unwrap_or(&r.database_name)
            .to_string();

        DatabaseInfo {
            id,
            database_name: r.database_name.clone(),
            schema_name: r.schema_name.clone(),
            created_at: r.created_at.to_rfc3339(),
        }
    }).collect();

    let count = databases.len();

    Ok((
        StatusCode::OK,
        Json(ListDatabasesResponse {
            platform,
            databases,
            count,
        }),
    ))
}

// === List Platforms ===

#[derive(Serialize)]
pub struct PlatformSummary {
    pub name: String,
    pub schemas: usize,
    pub databases: usize,
}

#[derive(Serialize)]
pub struct ListPlatformsResponse {
    pub platforms: Vec<PlatformSummary>,
    pub count: usize,
}

pub async fn list_platforms(
    State(state): State<Arc<PlatformState>>,
) -> Result<impl IntoResponse> {
    let platform_names = state.registry.list_platforms()?;

    let mut platforms = Vec::new();
    for name in &platform_names {
        if let Ok(info) = state.registry.get_platform_info(name) {
            platforms.push(PlatformSummary {
                name: info.name,
                schemas: info.schemas.len(),
                databases: info.databases.len(),
            });
        }
    }

    let count = platforms.len();

    Ok((
        StatusCode::OK,
        Json(ListPlatformsResponse {
            platforms,
            count,
        }),
    ))
}
