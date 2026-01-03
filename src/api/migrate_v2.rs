//! Migrate API v2 - Uses stored schemas
//!
//! POST /v2/migrate - Migrate databases using stored schema

use crate::api::platform::PlatformState;
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
use tracing::info;

/// Shared state for migrate v2 endpoint
pub struct MigrateV2State {
    pub pool_manager: Arc<PoolManager>,
    pub platform_state: Arc<PlatformState>,
}

#[derive(Debug, Deserialize)]
pub struct MigrateV2Request {
    pub platform: String,
    pub schema_name: String,
    #[serde(default)]
    pub database_id: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct SeederValidationInfo {
    table: String,
    expected: usize,
    found: usize,
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
pub struct VerificationInfo {
    passed: bool,
    extensions_verified: bool,
    types_verified: bool,
    tables_verified: bool,
    seeders_verified: bool,
    error_log: Option<String>,
}

#[derive(Serialize)]
pub struct MigrateV2Response {
    status: String,
    platform: String,
    schema_name: String,
    databases_updated: Vec<String>,
    migrations_applied: usize,
    functions_updated: usize,
    seeder_validations: Vec<SeederValidationInfo>,
    schema_validation: Option<SchemaValidationInfo>,
    verification: Option<VerificationInfo>,
    execution_time_ms: u64,
}

pub async fn migrate_schema_v2(
    State(state): State<Arc<MigrateV2State>>,
    Json(request): Json<MigrateV2Request>,
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

    let mut databases_updated = Vec::new();
    let mut total_migrations = 0;
    let mut total_functions = 0;
    let mut all_seeder_validations = Vec::new();
    let mut schema_validation: Option<SchemaValidationInfo> = None;
    let mut verification_info: Option<VerificationInfo> = None;

    // Get databases to migrate
    let databases_to_migrate: Vec<String> = if let Some(db_id) = &request.database_id {
        // Migrate single database
        let db_name = format!(
            "{}_{}_{}",
            request.platform, request.schema_name, db_id
        );
        if !state.pool_manager.database_exists(&db_name).await? {
            return Err(GatewayError::DatabaseNotFound {
                platform: request.platform.clone(),
                tenant_id: Some(db_id.clone()),
            });
        }
        vec![db_name]
    } else {
        // Migrate ALL databases for this platform/schema
        state
            .platform_state
            .registry
            .list_databases(&request.platform, Some(&request.schema_name))?
            .iter()
            .map(|r| r.database_name.clone())
            .collect()
    };

    if databases_to_migrate.is_empty() {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "No databases found for platform '{}' schema '{}'",
                request.platform, request.schema_name
            ),
        });
    }

    info!(
        "Migrating {} databases for platform '{}' schema '{}'",
        databases_to_migrate.len(),
        request.platform,
        request.schema_name
    );

    for (i, db_name) in databases_to_migrate.iter().enumerate() {
        let pool = state.pool_manager.get_pool_by_name(db_name).await?;

        // Ensure changelog table exists
        changelog_manager
            .ensure_changelog_table(&pool, db_name)
            .await?;

        // Validate schema changes before migration (only once, on first database)
        if i == 0 {
            let diff = diff_checker
                .validate_migration(&pool, db_name, &tables_dir, request.force)
                .await?;
            schema_validation = Some(diff_to_validation_info(&diff));
        }

        // 1. Run migrations ONLY from migrations/ folder
        let migrations = migration_runner
            .run_migrations(&pool, db_name, &migrations_dir)
            .await?;

        // 2. Deploy functions (always redeployed)
        let functions = function_deployer
            .deploy_functions(&pool, db_name, &functions_dir)
            .await?;

        // 3. Verify schema matches declarative definitions (only on first database)
        if i == 0 {
            let verification = schema_verifier
                .verify_schema(
                    &pool,
                    db_name,
                    &extensions_dir,
                    &types_dir,
                    &tables_dir,
                    &seeders_dir,
                )
                .await?;

            // Collect seeder validations from verification result
            for seeder_missing in &verification.seeders.missing {
                all_seeder_validations.push(SeederValidationInfo {
                    table: seeder_missing.table.clone(),
                    expected: seeder_missing.count,
                    found: 0,
                });
            }

            // Build verification info
            verification_info = Some(VerificationInfo {
                passed: verification.passed,
                extensions_verified: verification.extensions.missing.is_empty(),
                types_verified: verification.types.missing.is_empty(),
                tables_verified: verification.tables.missing.is_empty()
                    && verification.tables.mismatches.is_empty(),
                seeders_verified: verification.seeders.missing.is_empty(),
                error_log: if verification.passed {
                    None
                } else {
                    Some(verification.error_log())
                },
            });

            // If verification failed and not forced, return error
            if !verification.passed && !request.force {
                return Err(GatewayError::MigrationFailed {
                    database: db_name.clone(),
                    migration: "schema verification".to_string(),
                    cause: verification.error_log(),
                });
            }
        }

        // Log migration summary to changelog for this database
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
                    "migrate",
                )
                .await
                .ok();
        }

        total_migrations += migrations;
        total_functions += functions;
        databases_updated.push(db_name.clone());
    }

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    let status = if verification_info.as_ref().map(|v| v.passed).unwrap_or(true) {
        "completed".to_string()
    } else {
        "completed_with_warnings".to_string()
    };

    info!(
        "Migration complete for platform '{}' schema '{}': {} databases, {} migrations, {} functions in {}ms",
        request.platform,
        request.schema_name,
        databases_updated.len(),
        total_migrations,
        total_functions,
        execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(MigrateV2Response {
            status,
            platform: request.platform,
            schema_name: request.schema_name,
            databases_updated,
            migrations_applied: total_migrations,
            functions_updated: total_functions,
            seeder_validations: all_seeder_validations,
            schema_validation,
            verification: verification_info,
            execution_time_ms,
        }),
    ))
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
        dataloss_changes: diff.dataloss_changes.iter().map(convert_change).collect(),
        incompatible_changes: diff
            .incompatible_changes
            .iter()
            .map(convert_change)
            .collect(),
    }
}
