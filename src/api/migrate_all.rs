//! Migrate All API - Migrate all databases for a platform
//!
//! POST /v2/migrate-all - Migrate all databases for a platform using stored schema

use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::schema::{
    ChangeCompatibility, ChangelogManager, CustomTypeManager, ExtensionManager, FunctionDeployer,
    GatewayFunctionInstaller, MigrationGuards, MigrationRunner, SchemaDiff, SchemaDiffChecker,
    SchemaVerifier, TableDeployer,
};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// Shared state for migrate-all endpoint (reuses MigrateState)
pub use crate::api::migrate::MigrateState;

#[derive(Debug, Deserialize)]
pub struct MigrateAllRequest {
    pub platform: String,
    pub schema_name: String,
    /// Per-operation allow-tokens for guarded destructive changes. Empty by default.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Bypass the holistic post-migration schema-verification gate only.
    #[serde(default)]
    pub skip_verification: bool,
    /// Legacy allow-all: permits every guarded op AND skips verification.
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct DatabaseMigrationResult {
    database: String,
    status: String,
    migrations_applied: usize,
    tables_created: usize,
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
pub struct MigrateAllResponse {
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

pub async fn migrate_all_schema(
    State(state): State<Arc<MigrateState>>,
    Json(request): Json<MigrateAllRequest>,
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

    // List all databases for platform, excluding the main database.
    // The main DB ({platform}_main) stores platform-level data and should NOT
    // have tenant schema applied to it.
    let databases: Vec<String> = state
        .pool_manager
        .list_databases_for_platform(&request.platform)
        .await?
        .into_iter()
        .filter(|db| !db.ends_with("_main"))
        .collect();

    if databases.is_empty() {
        info!(
            "No tenant databases found for platform '{}' — nothing to migrate",
            request.platform
        );
        return Ok((
            StatusCode::OK,
            Json(MigrateAllResponse {
                status: "completed".to_string(),
                platform: request.platform,
                schema_name: request.schema_name,
                total_databases: 0,
                succeeded: 0,
                failed: 0,
                results: vec![],
                schema_validation: None,
                execution_time_ms: start_time.elapsed().as_millis() as u64,
            }),
        ));
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
    let extension_manager = ExtensionManager::new();
    let type_manager = CustomTypeManager::new();
    let migration_runner = MigrationRunner::new();
    let table_deployer = TableDeployer::new();
    let function_deployer = FunctionDeployer::new();
    let schema_verifier = SchemaVerifier::new();
    let diff_checker = SchemaDiffChecker::new();

    // Operator-granted permissions for guarded destructive operations.
    // Applied identically to every tenant database in this run.
    let guards = MigrationGuards::new(
        request.allow.clone(),
        request.skip_verification,
        request.force,
    );

    let mut results = Vec::new();
    let mut schema_validation: Option<SchemaValidationInfo> = None;
    let mut succeeded = 0;
    let mut failed = 0;

    for (i, db_name) in databases.iter().enumerate() {
        let tenant_start = Instant::now();
        info!(
            "  [{}/{}] tenant={} START migrate (force={}, validate_schema={})",
            i + 1,
            databases.len(),
            db_name,
            request.force,
            i == 0,
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
            &extension_manager,
            &type_manager,
            &migration_runner,
            &table_deployer,
            &function_deployer,
            &schema_verifier,
            &diff_checker,
            &guards,
            i == 0, // validate schema only on first database
        )
        .await
        {
            Ok((migrations, tables, functions, diff)) => {
                if i == 0 {
                    if let Some(d) = diff {
                        schema_validation = Some(diff_to_validation_info(&d));
                    }
                }
                info!(
                    "  [{}/{}] tenant={} OK migrations={} tables={} functions={} elapsed={}ms",
                    i + 1,
                    databases.len(),
                    db_name,
                    migrations,
                    tables,
                    functions,
                    tenant_start.elapsed().as_millis()
                );
                results.push(DatabaseMigrationResult {
                    database: db_name.clone(),
                    status: "completed".to_string(),
                    migrations_applied: migrations,
                    tables_created: tables,
                    functions_updated: functions,
                    error: None,
                });
                succeeded += 1;
            }
            Err(e) => {
                // Log BOTH Display (user-facing) and Debug (structured) so operators
                // have full context. Include elapsed + index so it's easy to correlate
                // with the START line when grepping by tenant.
                warn!(
                    "  [{}/{}] tenant={} FAIL elapsed={}ms error={}",
                    i + 1,
                    databases.len(),
                    db_name,
                    tenant_start.elapsed().as_millis(),
                    e
                );
                warn!("  [{}/{}] tenant={} error_debug={:?}", i + 1, databases.len(), db_name, e);
                results.push(DatabaseMigrationResult {
                    database: db_name.clone(),
                    status: "failed".to_string(),
                    migrations_applied: 0,
                    tables_created: 0,
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
                            tables_created: 0,
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
        // Some succeeded and some failed. Only report "partial" if actual migration
        // work was done on the successful databases. If all successful databases had
        // nothing to do (0 migrations, 0 tables, 0 functions), the system is already
        // up to date — failures were operational (e.g. connection issues), not
        // migration failures, so the overall status is "completed".
        let any_work_done = results.iter().any(|r| {
            r.status == "completed"
                && (r.migrations_applied > 0
                    || r.tables_created > 0
                    || r.functions_updated > 0)
        });
        if any_work_done {
            "partial".to_string()
        } else {
            "completed".to_string()
        }
    };

    info!(
        "Migrate-all complete for platform '{}': {}/{} succeeded, {} failed in {}ms",
        request.platform, succeeded, total_databases, failed, execution_time_ms
    );

    // Post-mortem summary: when some tenants failed, emit a single grep-friendly
    // block listing each failed tenant + its error. This is the log block to
    // read when investigating "why did tenant2 fail but tenant1 succeed".
    if failed > 0 {
        warn!(
            "Migrate-all failures for platform '{}' ({} of {} tenants):",
            request.platform, failed, total_databases
        );
        for r in &results {
            if r.status == "failed" {
                warn!(
                    "  FAILED tenant={} migrations_applied={} tables={} functions={} error={}",
                    r.database,
                    r.migrations_applied,
                    r.tables_created,
                    r.functions_updated,
                    r.error.as_deref().unwrap_or("(no error message)")
                );
            } else if r.status == "skipped" {
                warn!("  SKIPPED tenant={} reason={}", r.database, r.error.as_deref().unwrap_or(""));
            }
        }
    }

    Ok((
        StatusCode::OK,
        Json(MigrateAllResponse {
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
    extension_manager: &ExtensionManager,
    type_manager: &CustomTypeManager,
    migration_runner: &MigrationRunner,
    table_deployer: &TableDeployer,
    function_deployer: &FunctionDeployer,
    schema_verifier: &SchemaVerifier,
    diff_checker: &SchemaDiffChecker,
    guards: &MigrationGuards,
    validate_schema: bool,
) -> std::result::Result<(usize, usize, usize, Option<SchemaDiff>), GatewayError> {
    let pool = pool_manager.get_pool_by_name(db_name).await?;

    // Ensure changelog table exists
    changelog_manager
        .ensure_changelog_table(&pool, db_name)
        .await?;

    // Install gateway-provided functions (idempotent)
    GatewayFunctionInstaller::ensure_installed(&pool, db_name).await?;

    // Validate schema changes (only on first database)
    let diff = if validate_schema {
        let d = diff_checker
            .validate_migration(&pool, db_name, tables_dir, migrations_dir, guards)
            .await?;
        Some(d)
    } else {
        None
    };

    // 1. Install extensions (idempotent)
    extension_manager
        .install_extensions(&pool, db_name, extensions_dir)
        .await?;

    // 2. Deploy custom types (enums etc. — must exist before tables reference them)
    type_manager
        .deploy_types(&pool, db_name, types_dir)
        .await?;

    // 3. Deploy tables (CREATE IF NOT EXISTS for new .pgsql files)
    let tables = table_deployer
        .deploy_tables(&pool, db_name, tables_dir)
        .await?;

    // 4. Deploy functions
    let functions = function_deployer
        .deploy_functions(&pool, db_name, functions_dir)
        .await?;

    // 5. Run migrations (ALTER TABLE, etc. — after base schema is fully deployed)
    let migrations = migration_runner
        .run_migrations(&pool, db_name, migrations_dir)
        .await?;

    // 4. Verify schema (only on first database)
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

        if !verification.passed && !guards.skip_verification() {
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

    Ok((migrations, tables, functions, diff))
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
