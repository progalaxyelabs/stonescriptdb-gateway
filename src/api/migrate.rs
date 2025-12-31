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
pub struct MigrateResponse {
    status: String,
    databases_updated: Vec<String>,
    migrations_applied: usize,
    functions_updated: usize,
    execution_time_ms: u64,
}

pub async fn migrate_schema(
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

    // Extract schema
    let extractor = SchemaExtractor::from_bytes(&schema_data)?;

    let migration_runner = MigrationRunner::new();
    let function_deployer = FunctionDeployer::new();

    let mut databases_updated = Vec::new();
    let mut total_migrations = 0;
    let mut total_functions = 0;

    if let Some(tid) = &tenant_id {
        // Migrate single tenant database
        let db_name = pool_manager.database_name(&platform, Some(tid));

        if !pool_manager.database_exists(&db_name).await? {
            return Err(GatewayError::DatabaseNotFound {
                platform: platform.clone(),
                tenant_id: Some(tid.clone()),
            });
        }

        let pool = pool_manager.get_pool(&platform, Some(tid)).await?;

        let migrations = migration_runner
            .run_migrations(&pool, &db_name, &extractor.migrations_dir())
            .await?;

        let functions = function_deployer
            .deploy_functions(&pool, &db_name, &extractor.functions_dir())
            .await?;

        total_migrations += migrations;
        total_functions += functions;
        databases_updated.push(db_name);
    } else {
        // Migrate ALL databases for this platform
        let all_databases = pool_manager.list_databases_for_platform(&platform).await?;

        info!(
            "Migrating {} databases for platform {}",
            all_databases.len(),
            platform
        );

        for db_name in all_databases {
            let pool = pool_manager.get_pool_by_name(&db_name).await?;

            let migrations = migration_runner
                .run_migrations(&pool, &db_name, &extractor.migrations_dir())
                .await?;

            let functions = function_deployer
                .deploy_functions(&pool, &db_name, &extractor.functions_dir())
                .await?;

            total_migrations += migrations;
            total_functions += functions;
            databases_updated.push(db_name);
        }
    }

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    info!(
        "Migration complete for platform {}: {} databases, {} migrations, {} functions in {}ms",
        platform,
        databases_updated.len(),
        total_migrations,
        total_functions,
        execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(MigrateResponse {
            status: "completed".to_string(),
            databases_updated,
            migrations_applied: total_migrations,
            functions_updated: total_functions,
            execution_time_ms,
        }),
    ))
}
