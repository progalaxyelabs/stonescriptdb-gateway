//! Migrate All API v2 - Migrate all databases for a platform
//!
//! POST /v2/migrate-all - Migrate all databases for a platform using stored schema

use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::schema::{
    ChangeCompatibility, ChangelogManager, FunctionDeployer, MigrationRunner, SchemaDiff,
    SchemaDiffChecker, SchemaVerifier,
};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// Shared state for migrate-all v2 endpoint (reuses MigrateV2State)
pub use crate::api::migrate_v2::MigrateV2State;

#[derive(Debug, Deserialize)]
pub struct MigrateAllV2Request {
    pub platform: String,
    pub schema_name: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct DatabaseMigrationResult {
    database: String,
    status: String,
    migrations_applied: usize,
    functions_updated: usize,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct SchemaChangeInfo {
    table: String,
    change_type: String,
    column: Option<String>,
    from_type: Option<String>,
    to_type: Option<String>,
    compatibility: String,
    reason: Option<String>,
}

#[derive(Serialize)]
pub struct SchemaValidationInfo {
    safe_changes: Vec<SchemaChangeInfo>,
    dataloss_changes: Vec<SchemaChangeInfo>,
    incompatible_changes: Vec<SchemaChangeInfo>,
}

#[derive(Serialize)]
pub struct MigrateAllV2Response {
    status: String,
    platform: String,
    schema_name: String,
    total_databases: usize,
    succeeded: usize,
    failed: usize,
    results: Vec<DatabaseMigrationResult>,
    schema_validation: Option<SchemaValidationInfo>,
    execution_time_ms: u64,
}

pub async fn migrate_all_schema_v2(
    State(state): State<Arc<MigrateV2State>>,
    Json(request): Json<MigrateAllV2Request>,
) -> Result<impl IntoResponse> {
    let start_time = Instant::now();

    // Check platform is registered
    if !state
        .platform_state
        .registry
        .is_registered(&request.platform)
    {
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

    // List all databases for platform
    let databases = state
        .pool_manager
        .list_databases_for_platform(&request.platform)
        .await?;

    if databases.is_empty() {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "No databases found for platform '{}'.",
                request.platform
            ),
        });
    }

    info!(
        "Migrating ALL {} databases for platform '{}' schema '{}'",
        databases.len(),
        request.platform,
        request.schema_name
    );

    // Get schema directories
    let tables_dir = state
        .platform_state
        .schema_store
        .tables_dir(&request.platform, &request.schema_name);
    let functions_dir = state
        .platform_state
        .schema_store
        .functions_dir(&request.platform, &request.schema_name);
    let migrations_dir = state
        .platform_state
        .schema_store
        .migrations_dir(&request.platform, &request.schema_name);
    let extensions_dir = state
        .platform_state
        .schema_store
        .extensions_dir(&request.platform, &request.schema_name);
    let types_dir = state
        .platform_state
        .schema_store
        .types_dir(&request.platform, &request.schema_name);
    let seeders_dir = state
        .platform_state
        .schema_store
        .seeders_dir(&request.platform, &request.schema_name);

    let changelog_manager = ChangelogManager::new();
    let migration_runner = MigrationRunner::new();
    let function_deployer = FunctionDeployer::new();
    let schema_verifier = SchemaVerifier::new();
    let diff_checker = SchemaDiffChecker::new();

    let mut results = Vec::new();
    let mut schema_validation: Option<SchemaValidationInfo> = None;
    let mut succeeded = 0;
    let mut failed = 0;

    for (i, db_name) in databases.iter().enumerate() {
        info!(
            "  [{}/{}] Migrating database '{}'",
            i + 1,
            databases.len(),
            db_name
        );

        match migrate_single_database(
            &state.pool_manager,
            db_name,
            &tables_dir,
            &functions_dir,
            &migrations_dir,
            &extensions_dir,
            &types_dir,
            &seeders_dir,
            &changelog_manager,
            &migration_runner,
            &function_deployer,
            &schema_verifier,
            &diff_checker,
            request.force,
            i == 0, // validate schema only on first database
        )
        .await
        {
            Ok((migrations, functions, diff)) => {
                if i == 0 {
                    if let Some(d) = diff {
                        schema_validation = Some(diff_to_validation_info(&d));
                    }
                }
                results.push(DatabaseMigrationResult {
                    database: db_name.clone(),
                    status: "completed".to_string(),
                    migrations_applied: migrations,
                    functions_updated: functions,
                    error: None,
                });
                succeeded += 1;
            }
            Err(e) => {
                warn!("  Failed to migrate '{}': {}", db_name, e);
                results.push(DatabaseMigrationResult {
                    database: db_name.clone(),
                    status: "failed".to_string(),
                    migrations_applied: 0,
                    functions_updated: 0,
                    error: Some(e.to_string()),
                });
                failed += 1;

                // If the first database fails on schema validation, stop all
                if i == 0 {
                    warn!("First database failed — aborting remaining migrations");
                    for remaining_db in databases.iter().skip(1) {
                        results.push(DatabaseMigrationResult {
                            database: remaining_db.clone(),
                            status: "skipped".to_string(),
                            migrations_applied: 0,
                            functions_updated: 0,
                            error: Some("Skipped due to first database failure".to_string()),
                        });
                    }
                    break;
                }
            }
        }
    }

    let execution_time_ms = start_time.elapsed().as_millis() as u64;
    let total_databases = results.len();

    let status = if failed == 0 {
        "completed".to_string()
    } else if succeeded == 0 {
        "failed".to_string()
    } else {
        "partial".to_string()
    };

    info!(
        "Migrate-all complete for platform '{}': {}/{} succeeded in {}ms",
        request.platform, succeeded, total_databases, execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(MigrateAllV2Response {
            status,
            platform: request.platform,
            schema_name: request.schema_name,
            total_databases,
            succeeded,
            failed,
            results,
            schema_validation,
            execution_time_ms,
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn migrate_single_database(
    pool_manager: &PoolManager,
    db_name: &str,
    tables_dir: &std::path::Path,
    functions_dir: &std::path::Path,
    migrations_dir: &std::path::Path,
    extensions_dir: &std::path::Path,
    types_dir: &std::path::Path,
    seeders_dir: &std::path::Path,
    changelog_manager: &ChangelogManager,
    migration_runner: &MigrationRunner,
    function_deployer: &FunctionDeployer,
    schema_verifier: &SchemaVerifier,
    diff_checker: &SchemaDiffChecker,
    force: bool,
    validate_schema: bool,
) -> std::result::Result<(usize, usize, Option<SchemaDiff>), GatewayError> {
    let pool = pool_manager.get_pool_by_name(db_name).await?;

    // Ensure changelog table exists
    changelog_manager
        .ensure_changelog_table(&pool, db_name)
        .await?;

    // Validate schema changes (only on first database)
    let diff = if validate_schema {
        let d = diff_checker
            .validate_migration(&pool, db_name, tables_dir, force)
            .await?;
        Some(d)
    } else {
        None
    };

    // 1. Run migrations from migrations/ folder
    let migrations = migration_runner
        .run_migrations(&pool, db_name, migrations_dir)
        .await?;

    // 2. Deploy functions
    let functions = function_deployer
        .deploy_functions(&pool, db_name, functions_dir)
        .await?;

    // 3. Verify schema (only on first database)
    if validate_schema {
        let verification = schema_verifier
            .verify_schema(
                &pool,
                db_name,
                extensions_dir,
                types_dir,
                tables_dir,
                seeders_dir,
            )
            .await?;

        if !verification.passed && !force {
            return Err(GatewayError::MigrationFailed {
                database: db_name.to_string(),
                migration: "schema verification".to_string(),
                cause: verification.error_log(),
            });
        }
    }

    // Log to changelog
    if migrations > 0 {
        changelog_manager
            .log_migration(
                &pool,
                db_name,
                &format!("{} migrations applied", migrations),
                "batch",
            )
            .await
            .ok();
    }
    if functions > 0 {
        changelog_manager
            .log_function_deployed(
                &pool,
                db_name,
                &format!("{} functions", functions),
                "batch",
                "batch",
                "migrate-all",
            )
            .await
            .ok();
    }

    Ok((migrations, functions, diff))
}

/// Convert SchemaDiff to SchemaValidationInfo for JSON response
fn diff_to_validation_info(diff: &SchemaDiff) -> SchemaValidationInfo {
    let convert_change = |change: &crate::schema::SchemaChange| SchemaChangeInfo {
        table: change.table.clone(),
        change_type: format!("{:?}", change.change_type),
        column: change.column.clone(),
        from_type: change.from_type.clone(),
        to_type: change.to_type.clone(),
        compatibility: match change.compatibility {
            ChangeCompatibility::Safe => "safe".to_string(),
            ChangeCompatibility::DataLoss => "dataloss".to_string(),
            ChangeCompatibility::Incompatible => "incompatible".to_string(),
        },
        reason: change.reason.clone(),
    };

    SchemaValidationInfo {
        safe_changes: diff.safe_changes.iter().map(convert_change).collect(),
        dataloss_changes: diff
            .dataloss_changes
            .iter()
            .map(convert_change)
            .collect(),
        incompatible_changes: diff
            .incompatible_changes
            .iter()
            .map(convert_change)
            .collect(),
    }
}
