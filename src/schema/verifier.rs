//! Schema Verifier for Migrate
//!
//! After running migrations, verifies that the database schema matches
//! the declarative schema definition (extensions, types, tables, functions, seeders).
//!
//! If verification fails, the migrate operation should rollback and return
//! a detailed error log explaining what doesn't match.

use crate::error::Result;
use crate::schema::{
    CustomTypeManager, ExtensionManager, SchemaDiffChecker, SeederRunner,
};
use deadpool_postgres::Pool;
use serde::Serialize;
use std::path::Path;
use tracing::{debug, info, warn};

/// Result of schema verification
#[derive(Debug, Clone, Serialize)]
pub struct VerificationResult {
    pub passed: bool,
    pub extensions: ExtensionVerification,
    pub types: TypeVerification,
    pub tables: TableVerification,
    pub seeders: SeederVerification,
}

impl VerificationResult {
    pub fn new() -> Self {
        Self {
            passed: true,
            extensions: ExtensionVerification::default(),
            types: TypeVerification::default(),
            tables: TableVerification::default(),
            seeders: SeederVerification::default(),
        }
    }

    /// Generate a human-readable error log
    pub fn error_log(&self) -> String {
        let mut log = String::new();

        log.push_str("═══════════════════════════════════════════════════════════════\n");
        log.push_str("              SCHEMA VERIFICATION FAILED\n");
        log.push_str("═══════════════════════════════════════════════════════════════\n\n");

        if !self.extensions.missing.is_empty() {
            log.push_str("MISSING EXTENSIONS:\n");
            for ext in &self.extensions.missing {
                log.push_str(&format!("  - {}\n", ext));
            }
            log.push('\n');
        }

        if !self.types.missing.is_empty() {
            log.push_str("MISSING TYPES:\n");
            for t in &self.types.missing {
                log.push_str(&format!("  - {}\n", t));
            }
            log.push('\n');
        }

        if !self.tables.mismatches.is_empty() {
            log.push_str("TABLE SCHEMA MISMATCHES:\n");
            for m in &self.tables.mismatches {
                log.push_str(&format!("  - {}: {}\n", m.table, m.issue));
            }
            log.push('\n');
        }

        if !self.tables.missing.is_empty() {
            log.push_str("MISSING TABLES:\n");
            for t in &self.tables.missing {
                log.push_str(&format!("  - {}\n", t));
            }
            log.push('\n');
        }

        if !self.seeders.missing.is_empty() {
            log.push_str("MISSING SEEDER RECORDS:\n");
            for s in &self.seeders.missing {
                log.push_str(&format!("  - {} ({} missing records)\n", s.table, s.count));
            }
            log.push('\n');
        }

        log.push_str("═══════════════════════════════════════════════════════════════\n");
        log.push_str("ACTION REQUIRED: Add migration(s) to fix schema drift\n");
        log.push_str("═══════════════════════════════════════════════════════════════\n");

        log
    }
}

impl Default for VerificationResult {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtensionVerification {
    pub expected: Vec<String>,
    pub found: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TypeVerification {
    pub expected: Vec<String>,
    pub found: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TableVerification {
    pub expected: Vec<String>,
    pub found: Vec<String>,
    pub missing: Vec<String>,
    pub mismatches: Vec<TableMismatch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableMismatch {
    pub table: String,
    pub issue: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SeederVerification {
    pub missing: Vec<MissingSeeder>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissingSeeder {
    pub table: String,
    pub count: usize,
    pub keys: Vec<String>,
}

/// Schema verifier for post-migration checks
pub struct SchemaVerifier {
    extension_manager: ExtensionManager,
    type_manager: CustomTypeManager,
    diff_checker: SchemaDiffChecker,
    seeder_runner: SeederRunner,
}

impl SchemaVerifier {
    pub fn new() -> Self {
        Self {
            extension_manager: ExtensionManager::new(),
            type_manager: CustomTypeManager::new(),
            diff_checker: SchemaDiffChecker::new(),
            seeder_runner: SeederRunner::new(),
        }
    }

    /// Verify all schema components after migration
    pub async fn verify_schema(
        &self,
        pool: &Pool,
        database: &str,
        extensions_dir: &Path,
        types_dir: &Path,
        tables_dir: &Path,
        seeders_dir: &Path,
    ) -> Result<VerificationResult> {
        let mut result = VerificationResult::new();

        // 1. Verify extensions
        debug!("Verifying extensions for {}", database);
        result.extensions = self.verify_extensions(pool, database, extensions_dir).await?;
        if !result.extensions.missing.is_empty() {
            result.passed = false;
        }

        // 2. Verify types
        debug!("Verifying types for {}", database);
        result.types = self.verify_types(pool, database, types_dir).await?;
        if !result.types.missing.is_empty() {
            result.passed = false;
        }

        // 3. Verify tables match declarative schema
        debug!("Verifying tables for {}", database);
        result.tables = self.verify_tables(pool, database, tables_dir).await?;
        if !result.tables.missing.is_empty() || !result.tables.mismatches.is_empty() {
            result.passed = false;
        }

        // 4. Verify seeders
        debug!("Verifying seeders for {}", database);
        result.seeders = self.verify_seeders(pool, database, seeders_dir).await?;
        if !result.seeders.missing.is_empty() {
            result.passed = false;
        }

        if result.passed {
            info!("Schema verification PASSED for {}", database);
        } else {
            warn!("Schema verification FAILED for {}", database);
        }

        Ok(result)
    }

    /// Verify that all expected extensions are installed
    async fn verify_extensions(
        &self,
        pool: &Pool,
        database: &str,
        extensions_dir: &Path,
    ) -> Result<ExtensionVerification> {
        let mut verification = ExtensionVerification::default();

        // Get expected extensions from files
        let extension_files = self.extension_manager.find_extension_files(extensions_dir)?;
        for file in &extension_files {
            let ext = self.extension_manager.parse_extension(file)?;
            verification.expected.push(ext.name);
        }

        // Get installed extensions
        verification.found = self.extension_manager.list_extensions(pool, database).await?;

        // Find missing
        for expected in &verification.expected {
            if !verification.found.contains(expected) {
                verification.missing.push(expected.clone());
            }
        }

        Ok(verification)
    }

    /// Verify that all expected types exist
    async fn verify_types(
        &self,
        pool: &Pool,
        database: &str,
        types_dir: &Path,
    ) -> Result<TypeVerification> {
        let mut verification = TypeVerification::default();

        // Get expected types from files
        let type_files = self.type_manager.find_type_files(types_dir)?;
        for file in &type_files {
            if let Ok(custom_type) = self.type_manager.parse_type(file) {
                verification.expected.push(custom_type.name);
            }
        }

        // Get installed types
        verification.found = self.type_manager.list_types(pool, database).await?;

        // Find missing
        for expected in &verification.expected {
            if !verification.found.contains(expected) {
                verification.missing.push(expected.clone());
            }
        }

        Ok(verification)
    }

    /// Verify that database tables match declarative schema
    async fn verify_tables(
        &self,
        pool: &Pool,
        database: &str,
        tables_dir: &Path,
    ) -> Result<TableVerification> {
        let mut verification = TableVerification::default();

        // Parse desired schema from tables directory
        let desired = self.diff_checker.parse_desired_schema(tables_dir)?;

        for table_name in desired.keys() {
            verification.expected.push(table_name.clone());
        }

        // Query current schema
        let current = self.diff_checker.query_current_schema(pool, database).await?;

        for table_name in current.keys() {
            verification.found.push(table_name.clone());
        }

        // Find missing tables
        for expected in &verification.expected {
            if !current.contains_key(expected) {
                verification.missing.push(expected.clone());
            }
        }

        // Find mismatches in existing tables
        let diff = self.diff_checker.diff_schemas(&desired, &current);

        // Convert dataloss and incompatible changes to mismatches
        for change in diff.dataloss_changes.iter().chain(diff.incompatible_changes.iter()) {
            let issue = match &change.column {
                Some(col) => format!(
                    "{:?} column '{}': {} -> {}",
                    change.change_type,
                    col,
                    change.from_type.as_deref().unwrap_or("-"),
                    change.to_type.as_deref().unwrap_or("-")
                ),
                None => format!("{:?}", change.change_type),
            };

            verification.mismatches.push(TableMismatch {
                table: change.table.clone(),
                issue,
            });
        }

        Ok(verification)
    }

    /// Verify that all seeder records exist
    async fn verify_seeders(
        &self,
        pool: &Pool,
        database: &str,
        seeders_dir: &Path,
    ) -> Result<SeederVerification> {
        let mut verification = SeederVerification::default();

        // Use seeder validation (returns Err on failure, so we handle differently)
        match self.seeder_runner.validate_seeders(pool, database, seeders_dir).await {
            Ok(validations) => {
                // Check for any with missing records
                for v in validations {
                    if v.found < v.expected {
                        verification.missing.push(MissingSeeder {
                            table: v.table,
                            count: v.expected - v.found,
                            keys: v.missing,
                        });
                    }
                }
            }
            Err(e) => {
                // Parse error to extract missing info
                warn!("Seeder validation failed: {}", e);
                // We'll mark as failed but continue verification
                verification.missing.push(MissingSeeder {
                    table: "unknown".to_string(),
                    count: 0,
                    keys: vec![e.to_string()],
                });
            }
        }

        Ok(verification)
    }
}

impl Default for SchemaVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verification_result_error_log() {
        let mut result = VerificationResult::new();
        result.passed = false;
        result.extensions.missing = vec!["pgvector".to_string()];
        result.tables.mismatches.push(TableMismatch {
            table: "users".to_string(),
            issue: "Column 'email' type mismatch: VARCHAR(100) -> VARCHAR(255)".to_string(),
        });

        let log = result.error_log();

        assert!(log.contains("pgvector"));
        assert!(log.contains("users"));
        assert!(log.contains("email"));
        assert!(log.contains("ACTION REQUIRED"));
    }

    #[test]
    fn test_verification_result_empty_is_passed() {
        let result = VerificationResult::new();
        assert!(result.passed);
        assert!(result.extensions.missing.is_empty());
        assert!(result.types.missing.is_empty());
        assert!(result.tables.missing.is_empty());
        assert!(result.seeders.missing.is_empty());
    }
}
