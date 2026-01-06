use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::schema::{ChangelogManager, CustomTypeManager, ExtensionManager, FunctionDeployer, SchemaExtractor, SeederRunner, TableDeployer};
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
pub struct SeederInfo {
    table: String,
    inserted: usize,
    skipped: usize,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    status: String,
    database: String,
    extensions_installed: usize,
    types_deployed: usize,
    tables_created: usize,
    functions_deployed: usize,
    seeders: Vec<SeederInfo>,
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

    // Check if database already exists - register is ONLY for new databases
    if pool_manager.database_exists(&db_name).await? {
        return Err(GatewayError::DatabaseAlreadyExists {
            database: db_name,
        });
    }

    // Create new database
    pool_manager.create_database(&db_name).await?;

    // Extract schema
    let extractor = SchemaExtractor::from_bytes(&schema_data)?;

    // Deploy schema - if anything fails, we'll drop the database to maintain atomicity
    // Database creation is outside this block, and we use DROP DATABASE on failure for rollback
    let deployment_result = async {
        // Get pool for this database
        let pool = pool_manager.get_pool(&platform, tenant_id.as_deref()).await?;

        // Initialize changelog table for tracking all schema changes
        let changelog_manager = ChangelogManager::new();
        changelog_manager.ensure_changelog_table(&pool, &db_name).await?;

        // Install extensions first (before types/migrations, as they may depend on them)
        let extension_manager = ExtensionManager::new();
        let extensions_installed = extension_manager
            .install_extensions(&pool, &db_name, &extractor.extensions_dir())
            .await?;

        // Deploy custom types (after extensions, before tables)
        let type_manager = CustomTypeManager::new();
        let types_deployed = type_manager
            .deploy_types(&pool, &db_name, &extractor.types_dir())
            .await?;

        // Create tables from declarative schema (NOT from migrations/)
        let table_deployer = TableDeployer::new();
        let tables_created = table_deployer
            .deploy_tables(&pool, &db_name, &extractor.tables_dir())
            .await?;

        // Deploy functions
        let function_deployer = FunctionDeployer::new();
        let functions_deployed = function_deployer
            .deploy_functions(&pool, &db_name, &extractor.functions_dir())
            .await?;

        // Run seeders (only inserts into empty tables)
        // This is critical - if seeder fails, the entire registration fails
        let seeder_runner = SeederRunner::new();
        let seeder_results = seeder_runner
            .run_seeders_on_register(&pool, &db_name, &extractor.seeders_dir())
            .await?;

        Ok::<_, GatewayError>((
            pool,
            changelog_manager,
            extensions_installed,
            types_deployed,
            tables_created,
            functions_deployed,
            seeder_results,
        ))
    }.await;

    // Handle deployment result - drop database on failure
    let (pool, changelog_manager, extensions_installed, types_deployed, tables_created, functions_deployed, seeder_results) = match deployment_result {
        Ok(data) => data,
        Err(e) => {
            warn!("Schema deployment failed for '{}', dropping database: {}", db_name, e);
            // Drop the database on any failure
            if let Err(drop_err) = pool_manager.drop_database(&db_name).await {
                warn!("Failed to drop database '{}' after deployment failure: {}", db_name, drop_err);
            }
            return Err(e);
        }
    };

    let seeders: Vec<SeederInfo> = seeder_results
        .into_iter()
        .map(|r| SeederInfo {
            table: r.table,
            inserted: r.inserted,
            skipped: r.skipped,
        })
        .collect();

    let total_seeded: usize = seeders.iter().map(|s| s.inserted).sum();

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    // Log registration summary to changelog
    if extensions_installed > 0 {
        changelog_manager
            .log_extension_installed(&pool, &db_name, &format!("{} extensions", extensions_installed), None, None)
            .await
            .ok(); // Don't fail registration if changelog logging fails
    }
    if tables_created > 0 {
        changelog_manager
            .log_migration(&pool, &db_name, &format!("{} tables created", tables_created), "register")
            .await
            .ok();
    }
    if functions_deployed > 0 {
        changelog_manager
            .log_function_deployed(
                &pool,
                &db_name,
                &format!("{} functions", functions_deployed),
                "batch",
                "batch",
                "register",
            )
            .await
            .ok();
    }
    for seeder in &seeders {
        if seeder.inserted > 0 {
            changelog_manager
                .log_seeder_run(&pool, &db_name, &seeder.table, seeder.inserted, seeder.skipped)
                .await
                .ok();
        }
    }

    info!(
        "Schema registered for {}: {} extensions, {} types, {} tables, {} functions, {} seeder records in {}ms",
        db_name, extensions_installed, types_deployed, tables_created, functions_deployed, total_seeded, execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(RegisterResponse {
            status: "ready".to_string(),
            database: db_name,
            extensions_installed,
            types_deployed,
            tables_created,
            functions_deployed,
            seeders,
            execution_time_ms,
        }),
    ))
}
