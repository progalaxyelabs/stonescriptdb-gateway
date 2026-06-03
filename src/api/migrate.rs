//! Migrate API - Uses stored schemas
//!
//! POST /v2/migrate - Migrate databases using stored schema

use crate::api::platform::PlatformState;
use crate::config::Config;
use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::xdb::{self, CrossDbLinkAuthorization, CrossDbReader};
use crate::schema::{
    ChangeCompatibility, ChangelogManager, CustomTypeManager, ExtensionManager, FunctionDeployer,
    GatewayFunctionInstaller, MigrationGuards, MigrationRunner, SchemaDiff, SchemaDiffChecker,
    SchemaVerifier, TableDeployer,
};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

/// Shared state for migrate endpoint
pub struct MigrateState {
    pub pool_manager: Arc<PoolManager>,
    pub platform_state: Arc<PlatformState>,
    /// Full gateway config — needed for the privileged cross-DB-link URL (#2908).
    pub config: Config,
}

#[derive(Debug, Deserialize)]
pub struct MigrateRequest {
    pub platform: String,
    pub schema_name: String,
    /// Required: specific database/tenant to migrate (e.g., "main" for main DB, or tenant ID for tenant DB)
    pub database_id: String,
    /// Per-operation allow-tokens for guarded destructive changes
    /// (e.g. ["drop_column", "modify_column_type"]). Empty by default.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Bypass the holistic post-migration schema-verification gate only.
    #[serde(default)]
    pub skip_verification: bool,
    /// Legacy allow-all: permits every guarded op AND skips verification.
    #[serde(default)]
    pub force: bool,
    /// Authorization for the gated cross-DB-link capability (#2908). Absent/`authorized:false`
    /// ⇒ any `cross_db/<link>.json` manifest in the bundle is REJECTED (fail-closed).
    /// This block is the contract deploy-manager (#2909) plumbs from `--allow-cross-db-link`.
    #[serde(default)]
    pub cross_db_link: Option<CrossDbLinkAuthorization>,
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
pub struct MigrateResponse {
    status: String,
    platform: String,
    schema_name: String,
    databases_updated: Vec<String>,
    migrations_applied: usize,
    tables_created: usize,
    functions_updated: usize,
    seeder_validations: Vec<SeederValidationInfo>,
    schema_validation: Option<SchemaValidationInfo>,
    verification: Option<VerificationInfo>,
    execution_time_ms: u64,
}

pub async fn migrate_schema(
    State(state): State<Arc<MigrateState>>,
    Json(request): Json<MigrateRequest>,
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
    let cross_db_dir = state
        .platform_state
        .schema_store
        .cross_db_dir(&request.platform, &request.schema_name);

    let changelog_manager = ChangelogManager::new();
    let extension_manager = ExtensionManager::new();
    let type_manager = CustomTypeManager::new();
    let migration_runner = MigrationRunner::new();
    let table_deployer = TableDeployer::new();
    let function_deployer = FunctionDeployer::new();
    let schema_verifier = SchemaVerifier::new();
    let diff_checker = SchemaDiffChecker::new();

    // Operator-granted permissions for guarded destructive operations.
    // force=true preserves the legacy allow-all behavior.
    let guards = MigrationGuards::new(
        request.allow.clone(),
        request.skip_verification,
        request.force,
    );

    let mut databases_updated = Vec::new();
    let mut total_migrations = 0;
    let mut total_tables = 0;
    let mut total_functions = 0;
    let mut all_seeder_validations = Vec::new();
    let mut schema_validation: Option<SchemaValidationInfo> = None;
    let mut verification_info: Option<VerificationInfo> = None;

    // Construct database name using the same sanitization as database creation
    let db_name = if request.database_id == "main" {
        state.pool_manager.database_name(&request.platform, None)
    } else {
        state.pool_manager.database_name(&request.platform, Some(&request.database_id))
    };

    // Verify database exists
    if !state.pool_manager.database_exists(&db_name).await? {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "Database '{}' not found for platform '{}', database_id '{}'",
                db_name, request.platform, request.database_id
            ),
        });
    }

    info!(
        "Migrating database '{}' for platform '{}' schema '{}'",
        db_name,
        request.platform,
        request.schema_name
    );

    let databases_to_migrate = vec![db_name.clone()];

    for (i, db_name) in databases_to_migrate.iter().enumerate() {
        let pool = state.pool_manager.get_pool_by_name(db_name).await?;

        // Ensure changelog table exists
        changelog_manager
            .ensure_changelog_table(&pool, db_name)
            .await?;

        // Install gateway-provided functions (idempotent)
        GatewayFunctionInstaller::ensure_installed(&pool, db_name).await?;

        // Validate schema changes before migration (only once, on first database)
        if i == 0 {
            let diff = diff_checker
                .validate_migration(&pool, db_name, &tables_dir, &migrations_dir, &guards)
                .await?;
            schema_validation = Some(diff_to_validation_info(&diff));
        }

        // 1. Install extensions (idempotent)
        extension_manager
            .install_extensions(&pool, db_name, &extensions_dir)
            .await?;

        // 2. Deploy custom types (enums etc. — must exist before tables reference them)
        type_manager
            .deploy_types(&pool, db_name, &types_dir)
            .await?;

        // 3. Deploy tables (CREATE IF NOT EXISTS for new .pgsql files)
        let tables = table_deployer
            .deploy_tables(&pool, db_name, &tables_dir)
            .await?;

        // 4. Deploy functions (always redeployed)
        let functions = function_deployer
            .deploy_functions(&pool, db_name, &functions_dir)
            .await?;

        // 4b. Materialise gated cross-DB links into staging tables BEFORE migrations run,
        //     so a migration can read `_stonescriptdb_gateway_xdb_<link>`. Fail-closed:
        //     manifests present without authorization ⇒ hard error (never skipped).
        let cross_db_manifests = xdb::load_manifests(&cross_db_dir)?;
        let authz = request.cross_db_link.clone().unwrap_or_default();
        let xdb_reader = CrossDbReader::from_privileged_url(
            state.config.privileged_database_url.as_deref(),
            state.config.max_connections_per_pool,
        );
        let materialized = xdb::materialize_links(
            &cross_db_manifests,
            &authz,
            xdb_reader.as_ref(),
            &pool,
            state.pool_manager.admin_pool(),
            db_name,
            &request.platform,
        )
        .await?;
        if !materialized.is_empty() {
            info!(
                "cross_db_link: materialised {} link(s) into '{}'",
                materialized.len(),
                db_name
            );
        }

        // 5. Run migrations (ALTER TABLE, etc. — after base schema is fully deployed)
        let migrations = migration_runner
            .run_migrations(&pool, db_name, &migrations_dir)
            .await?;

        // 4. Verify schema matches declarative definitions (only on first database)
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

            // If verification failed and not bypassed, return error.
            // Gate #2 is bypassed by --dangerously-skip-verification or allow-all --force.
            if !verification.passed && !guards.skip_verification() {
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
        total_tables += tables;
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
        "Migration complete for platform '{}' schema '{}': {} databases, {} migrations, {} tables, {} functions in {}ms",
        request.platform,
        request.schema_name,
        databases_updated.len(),
        total_migrations,
        total_tables,
        total_functions,
        execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(MigrateResponse {
            status,
            platform: request.platform,
            schema_name: request.schema_name,
            databases_updated,
            migrations_applied: total_migrations,
            tables_created: total_tables,
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
