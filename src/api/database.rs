//! Database API endpoints
//!
//! - POST /database/create - Create a new database from a registered schema

use crate::api::platform::PlatformState;
use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::schema::{
    ChangelogManager, CustomTypeManager, ExtensionManager, FunctionDeployer, SeederRunner,
    TableDeployer,
};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

/// Shared state for database endpoints (includes both pool manager and platform state)
pub struct DatabaseState {
    pub pool_manager: Arc<PoolManager>,
    pub platform_state: Arc<PlatformState>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDatabaseRequest {
    pub platform: String,
    pub schema_name: String,
    pub database_id: String,
}

#[derive(Serialize)]
pub struct SeederInfo {
    table: String,
    inserted: usize,
    skipped: usize,
}

#[derive(Serialize)]
pub struct CreateDatabaseResponse {
    pub status: String,
    pub platform: String,
    pub schema_name: String,
    pub database_name: String,
    pub extensions_installed: usize,
    pub types_deployed: usize,
    pub tables_created: usize,
    pub functions_deployed: usize,
    pub seeders: Vec<SeederInfo>,
    pub execution_time_ms: u64,
}

pub async fn create_database(
    State(state): State<Arc<DatabaseState>>,
    Json(request): Json<CreateDatabaseRequest>,
) -> Result<impl IntoResponse> {
    let start_time = Instant::now();

    // Check platform is registered
    if !state.platform_state.registry.is_registered(&request.platform) {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "Platform '{}' is not registered. Register it first.",
                request.platform
            ),
        });
    }

    // Check schema exists
    if !state
        .platform_state
        .schema_store
        .schema_exists(&request.platform, &request.schema_name)
    {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "Schema '{}' not found for platform '{}'. Register the schema first.",
                request.schema_name, request.platform
            ),
        });
    }

    // Get schema info
    let schema = state
        .platform_state
        .schema_store
        .get_schema(&request.platform, &request.schema_name)?;

    // Generate database name: platform_schema_id
    let db_name = format!(
        "{}_{}_{}",
        request.platform, request.schema_name, request.database_id
    );

    info!(
        "Creating database '{}' from schema '{}' for platform '{}'",
        db_name, request.schema_name, request.platform
    );

    // Check if database already exists
    if state.pool_manager.database_exists(&db_name).await? {
        return Err(GatewayError::DatabaseAlreadyExists { database: db_name });
    }

    // Create new database
    state.pool_manager.create_database(&db_name).await?;

    // Get pool for this database
    let pool = state.pool_manager.get_pool_by_name(&db_name).await?;

    // Initialize changelog table
    let changelog_manager = ChangelogManager::new();
    changelog_manager
        .ensure_changelog_table(&pool, &db_name)
        .await?;

    // Install extensions
    let extension_manager = ExtensionManager::new();
    let extensions_installed = extension_manager
        .install_extensions(
            &pool,
            &db_name,
            &state
                .platform_state
                .schema_store
                .extensions_dir(&request.platform, &request.schema_name),
        )
        .await?;

    // Deploy custom types
    let type_manager = CustomTypeManager::new();
    let types_deployed = type_manager
        .deploy_types(
            &pool,
            &db_name,
            &state
                .platform_state
                .schema_store
                .types_dir(&request.platform, &request.schema_name),
        )
        .await?;

    // Create tables from declarative schema
    let table_deployer = TableDeployer::new();
    let tables_created = table_deployer
        .deploy_tables(
            &pool,
            &db_name,
            &state
                .platform_state
                .schema_store
                .tables_dir(&request.platform, &request.schema_name),
        )
        .await?;

    // Deploy functions
    let function_deployer = FunctionDeployer::new();
    let functions_deployed = function_deployer
        .deploy_functions(
            &pool,
            &db_name,
            &state
                .platform_state
                .schema_store
                .functions_dir(&request.platform, &request.schema_name),
        )
        .await?;

    // Run seeders
    let seeder_runner = SeederRunner::new();
    let seeder_results = seeder_runner
        .run_seeders_on_register(
            &pool,
            &db_name,
            &state
                .platform_state
                .schema_store
                .seeders_dir(&request.platform, &request.schema_name),
        )
        .await?;

    let seeders: Vec<SeederInfo> = seeder_results
        .into_iter()
        .map(|r| SeederInfo {
            table: r.table,
            inserted: r.inserted,
            skipped: r.skipped,
        })
        .collect();

    let total_seeded: usize = seeders.iter().map(|s| s.inserted).sum();

    // Record database in platform registry
    state.platform_state.registry.record_database(
        &request.platform,
        &request.schema_name,
        &db_name,
    )?;

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    // Log to changelog
    if extensions_installed > 0 {
        changelog_manager
            .log_extension_installed(
                &pool,
                &db_name,
                &format!("{} extensions", extensions_installed),
                None,
                None,
            )
            .await
            .ok();
    }
    if tables_created > 0 {
        changelog_manager
            .log_migration(
                &pool,
                &db_name,
                &format!("{} tables created", tables_created),
                "create",
            )
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
                "create",
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
        "Database '{}' created: {} extensions, {} types, {} tables, {} functions, {} seeder records in {}ms",
        db_name, extensions_installed, types_deployed, tables_created, functions_deployed, total_seeded, execution_time_ms
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateDatabaseResponse {
            status: "created".to_string(),
            platform: request.platform,
            schema_name: request.schema_name,
            database_name: db_name,
            extensions_installed,
            types_deployed,
            tables_created,
            functions_deployed,
            seeders,
            execution_time_ms,
        }),
    ))
}
