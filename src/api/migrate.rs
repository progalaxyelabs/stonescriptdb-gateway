use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use crate::schema::{
    ChangeCompatibility, ChangelogManager, FunctionDeployer, MigrationRunner, SchemaExtractor,
    SchemaDiff, SchemaDiffChecker, SchemaVerifier,
};
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
    databases_updated: Vec<String>,
    migrations_applied: usize,
    functions_updated: usize,
    seeder_validations: Vec<SeederValidationInfo>,
    schema_validation: Option<SchemaValidationInfo>,
    verification: Option<VerificationInfo>,
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
    let mut force: bool = false;

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
            "force" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| GatewayError::InvalidRequest {
                        message: format!("Failed to read force field: {}", e),
                    })?;
                force = text == "true" || text == "1";
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

        // Ensure changelog table exists
        changelog_manager.ensure_changelog_table(&pool, &db_name).await?;

        // Validate schema changes before migration (will fail if dataloss detected and force=false)
        let diff = diff_checker
            .validate_migration(&pool, &db_name, &extractor.tables_dir(), force)
            .await?;

        schema_validation = Some(diff_to_validation_info(&diff));

        // 1. Run migrations ONLY from migrations/ folder
        let migrations = migration_runner
            .run_migrations(&pool, &db_name, &extractor.migrations_dir())
            .await?;

        // 2. Deploy functions (always redeployed)
        let functions = function_deployer
            .deploy_functions(&pool, &db_name, &extractor.functions_dir())
            .await?;

        // 3. Verify schema matches declarative definitions
        let verification = schema_verifier
            .verify_schema(
                &pool,
                &db_name,
                &extractor.extensions_dir(),
                &extractor.types_dir(),
                &extractor.tables_dir(),
                &extractor.seeders_dir(),
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
            tables_verified: verification.tables.missing.is_empty() && verification.tables.mismatches.is_empty(),
            seeders_verified: verification.seeders.missing.is_empty(),
            error_log: if verification.passed {
                None
            } else {
                Some(verification.error_log())
            },
        });

        // If verification failed and not forced, return error
        if !verification.passed && !force {
            return Err(GatewayError::MigrationFailed {
                database: db_name,
                migration: "schema verification".to_string(),
                cause: verification.error_log(),
            });
        }

        // Log migration summary to changelog
        if migrations > 0 {
            changelog_manager
                .log_migration(&pool, &db_name, &format!("{} migrations applied", migrations), "batch")
                .await
                .ok();
        }
        if functions > 0 {
            changelog_manager
                .log_function_deployed(&pool, &db_name, &format!("{} functions", functions), "batch", "batch", "migrate")
                .await
                .ok();
        }

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

        for (i, db_name) in all_databases.iter().enumerate() {
            let pool = pool_manager.get_pool_by_name(db_name).await?;

            // Ensure changelog table exists
            changelog_manager.ensure_changelog_table(&pool, db_name).await?;

            // Validate schema changes before migration (only once, on first database)
            if i == 0 {
                let diff = diff_checker
                    .validate_migration(&pool, db_name, &extractor.tables_dir(), force)
                    .await?;
                schema_validation = Some(diff_to_validation_info(&diff));
            }

            // 1. Run migrations ONLY from migrations/ folder
            let migrations = migration_runner
                .run_migrations(&pool, db_name, &extractor.migrations_dir())
                .await?;

            // 2. Deploy functions (always redeployed)
            let functions = function_deployer
                .deploy_functions(&pool, db_name, &extractor.functions_dir())
                .await?;

            // 3. Verify schema matches declarative definitions (only on first database)
            if i == 0 {
                let verification = schema_verifier
                    .verify_schema(
                        &pool,
                        db_name,
                        &extractor.extensions_dir(),
                        &extractor.types_dir(),
                        &extractor.tables_dir(),
                        &extractor.seeders_dir(),
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
                    tables_verified: verification.tables.missing.is_empty() && verification.tables.mismatches.is_empty(),
                    seeders_verified: verification.seeders.missing.is_empty(),
                    error_log: if verification.passed {
                        None
                    } else {
                        Some(verification.error_log())
                    },
                });

                // If verification failed and not forced, return error
                if !verification.passed && !force {
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
                    .log_migration(&pool, db_name, &format!("{} migrations applied", migrations), "batch")
                    .await
                    .ok();
            }
            if functions > 0 {
                changelog_manager
                    .log_function_deployed(&pool, db_name, &format!("{} functions", functions), "batch", "batch", "migrate")
                    .await
                    .ok();
            }

            total_migrations += migrations;
            total_functions += functions;
            databases_updated.push(db_name.clone());
        }
    }

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    let status = if verification_info.as_ref().map(|v| v.passed).unwrap_or(true) {
        "completed".to_string()
    } else {
        "completed_with_warnings".to_string()
    };

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
            status,
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
        incompatible_changes: diff.incompatible_changes.iter().map(convert_change).collect(),
    }
}
