use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

#[derive(Debug, Deserialize)]
pub struct ListDatabasesQuery {
    pub platform: String,
}

#[derive(Serialize)]
pub struct DatabaseInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub db_type: String,
    pub size_mb: i64,
}

#[derive(Serialize)]
pub struct ListDatabasesResponse {
    pub platform: String,
    pub databases: Vec<DatabaseInfo>,
    pub count: usize,
}

pub async fn admin_list_databases(
    State((pool_manager, _)): State<(Arc<PoolManager>, Instant)>,
    Query(query): Query<ListDatabasesQuery>,
) -> Result<impl IntoResponse> {
    let databases = pool_manager
        .list_databases_for_platform(&query.platform)
        .await?;

    let mut db_infos = Vec::with_capacity(databases.len());

    for db_name in &databases {
        // Determine type
        let db_type = if db_name.ends_with("_main") {
            "main"
        } else {
            "tenant"
        };

        // Get size (in bytes), convert to MB
        let size_bytes = match pool_manager.get_database_size(db_name).await {
            Ok(size) => size,
            Err(_) => 0, // If we can't get size, report 0
        };

        let size_mb = size_bytes / (1024 * 1024);

        db_infos.push(DatabaseInfo {
            name: db_name.clone(),
            db_type: db_type.to_string(),
            size_mb,
        });
    }

    let count = db_infos.len();

    Ok((
        StatusCode::OK,
        Json(ListDatabasesResponse {
            platform: query.platform,
            databases: db_infos,
            count,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateTenantRequest {
    pub platform: String,
    pub tenant_id: String,
}

#[derive(Serialize)]
pub struct CreateTenantResponse {
    pub status: String,
    pub database: String,
    pub message: String,
}

pub async fn admin_create_tenant(
    State((pool_manager, _)): State<(Arc<PoolManager>, Instant)>,
    Json(request): Json<CreateTenantRequest>,
) -> Result<impl IntoResponse> {
    let db_name = pool_manager.database_name(&request.platform, Some(&request.tenant_id));

    // Check if already exists
    if pool_manager.database_exists(&db_name).await? {
        return Err(GatewayError::DatabaseAlreadyExists {
            database: db_name,
        });
    }

    // Create the database
    pool_manager.create_database(&db_name).await?;

    info!("Created tenant database: {}", db_name);

    Ok((
        StatusCode::CREATED,
        Json(CreateTenantResponse {
            status: "created".to_string(),
            database: db_name,
            message: "Database created. Run /register or /migrate to deploy schema.".to_string(),
        }),
    ))
}
