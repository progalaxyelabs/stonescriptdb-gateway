use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::schema::{FunctionDeployer, MigrationRunner, SchemaExtractor};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Multipart;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Serialize)]
pub struct RegisterResponse {
    status: String,
    database: String,
    migrations_applied: usize,
    functions_deployed: usize,
    execution_time_ms: u64,
}

pub async fn register_schema(
    State((pool_manager, _)): State<(Arc<PoolManager>, Instant)>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse> {
    let start_time = Instant::now();

    let mut platform: Option<String> = None;
    let mut tenant_id: Option<String> = None;
    let mut schema_data: Option<Vec<u8>> = None;

    // Parse multipart form
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        GatewayError::InvalidRequest {
            message: format!("Failed to parse multipart form: {}", e),
        }
    })? {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "platform" => {
                platform = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to read platform field: {}", e),
                        })?,
                );
            }
            "tenant_id" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| GatewayError::InvalidRequest {
                        message: format!("Failed to read tenant_id field: {}", e),
                    })?;
                if !text.is_empty() && text != "null" {
                    tenant_id = Some(text);
                }
            }
            "schema" => {
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
    let platform = platform.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Missing required field: platform".to_string(),
    })?;

    let schema_data = schema_data.ok_or_else(|| GatewayError::InvalidRequest {
        message: "Missing required field: schema".to_string(),
    })?;

    // Generate database name
    let db_name = pool_manager.database_name(&platform, tenant_id.as_deref());

    info!(
        "Registering schema for platform={}, tenant_id={:?}, database={}",
        platform, tenant_id, db_name
    );

    // Create database if it doesn't exist
    pool_manager.create_database(&db_name).await?;

    // Extract schema
    let extractor = SchemaExtractor::from_bytes(&schema_data)?;

    // Get pool for this database
    let pool = pool_manager.get_pool(&platform, tenant_id.as_deref()).await?;

    // Run migrations
    let migration_runner = MigrationRunner::new();
    let migrations_applied = migration_runner
        .run_migrations(&pool, &db_name, &extractor.migrations_dir())
        .await?;

    // Deploy functions
    let function_deployer = FunctionDeployer::new();
    let functions_deployed = function_deployer
        .deploy_functions(&pool, &db_name, &extractor.functions_dir())
        .await?;

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    info!(
        "Schema registered for {}: {} migrations, {} functions in {}ms",
        db_name, migrations_applied, functions_deployed, execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(RegisterResponse {
            status: "ready".to_string(),
            database: db_name,
            migrations_applied,
            functions_deployed,
            execution_time_ms,
        }),
    ))
}
