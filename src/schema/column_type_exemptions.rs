//! Column Type Change Exemption Scanner
//!
//! Scans pending migration files for calls to `_stonescriptdb_gateway_change_column_type`.
//! Extracts (table, column, new_type) tuples so the diff checker can exempt
//! those specific columns from compatibility checks.
//!
//! The diff checker still validates everything else on the table.
//! After migrations run, the schema verifier confirms the final state matches.

use crate::error::{GatewayError, Result};
use crate::schema::MigrationRunner;
use regex::Regex;
use std::path::Path;
use tracing::{debug, info, warn};

const FN_TOKEN: &str = "_stonescriptdb_gateway_change_column_type";

/// A column type change that is being handled by a gateway migration function.
#[derive(Debug, Clone)]
pub struct ColumnTypeExemption {
    pub table: String,
    pub column: String,
    pub new_type: String,
    pub migration_file: String,
}

/// Scan pending migration files for calls to `_stonescriptdb_gateway_change_column_type`.
///
/// Returns exemptions only for migrations that haven't been applied yet.
pub fn scan_migrations_for_exemptions(
    migrations_dir: &Path,
    applied_migrations: &[String],
) -> Result<Vec<ColumnTypeExemption>> {
    let runner = MigrationRunner::new();
    let migration_files = runner.find_migration_files(migrations_dir)?;

    let mut exemptions = Vec::new();
    let mut files_scanned = 0usize;
    let mut files_skipped_applied = 0usize;

    // Regex to match: _stonescriptdb_gateway_change_column_type('table', 'column', 'type', 'func')
    // Tolerant of whitespace, newlines, and various quoting styles
    let re = Regex::new(
        r"(?si)_stonescriptdb_gateway_change_column_type\s*\(\s*'([^']+)'\s*,\s*'([^']+)'\s*,\s*'([^']+)'\s*,\s*'([^']+)'\s*\)"
    ).map_err(|e| GatewayError::SchemaExtractionFailed {
        cause: format!("Failed to compile exemption regex: {}", e),
    })?;

    for migration in &migration_files {
        if applied_migrations.contains(&migration.name) {
            debug!(
                "Type-change scanner: skipping already-applied migration {}",
                migration.name
            );
            files_skipped_applied += 1;
            continue;
        }

        let content = std::fs::read_to_string(&migration.path).map_err(|e| {
            GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read migration file {:?}: {}", migration.path, e),
            }
        })?;

        files_scanned += 1;
        let mut captures_in_file = 0usize;

        for caps in re.captures_iter(&content) {
            let table = caps[1].to_string();
            let column = caps[2].to_string();
            let new_type = caps[3].to_string();
            let helper_fn = caps[4].to_string();
            captures_in_file += 1;

            info!(
                "Type-change exemption registered: {}.{} -> {} via helper '{}' (migration: {})",
                table, column, new_type, helper_fn, migration.name
            );

            exemptions.push(ColumnTypeExemption {
                table,
                column,
                new_type,
                migration_file: migration.name.clone(),
            });
        }

        if captures_in_file == 0 && content.contains(FN_TOKEN) {
            warn!(
                "Type-change scanner: {} contains '{}' but no calls matched the expected \
                 signature ('table','column','new_type','helper_fn') with single quotes. The \
                 exemption will NOT be applied — the migration will be blocked. Check for \
                 typos, double quotes, or line-broken args.",
                migration.name, FN_TOKEN
            );
        }
    }

    info!(
        "Type-change scanner: {} exemption(s) across {} scanned file(s) ({} already-applied skipped)",
        exemptions.len(),
        files_scanned,
        files_skipped_applied
    );

    Ok(exemptions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_finds_exemptions() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
CREATE OR REPLACE FUNCTION _migrate_pack_size(p_table TEXT, p_old_col TEXT, p_new_col TEXT)
RETURNS void AS $$
BEGIN
    EXECUTE format('UPDATE %I SET %I = 1', p_table, p_new_col);
END;
$$ LANGUAGE plpgsql;

SELECT _stonescriptdb_gateway_change_column_type(
    'item_prices', 'pack_size', 'INTEGER', '_migrate_pack_size'
);

DROP FUNCTION _migrate_pack_size(TEXT, TEXT, TEXT);
"#;

        fs::write(migrations_dir.join("004_change_type.pssql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();

        assert_eq!(exemptions.len(), 1);
        assert_eq!(exemptions[0].table, "item_prices");
        assert_eq!(exemptions[0].column, "pack_size");
        assert_eq!(exemptions[0].new_type, "INTEGER");
        assert_eq!(exemptions[0].migration_file, "004_change_type.pssql");
    }

    #[test]
    fn test_scan_skips_applied_migrations() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
SELECT _stonescriptdb_gateway_change_column_type('items', 'qty', 'BIGINT', '_migrate_qty');
"#;

        fs::write(migrations_dir.join("001_change.pssql"), sql).unwrap();

        // Migration already applied
        let applied = vec!["001_change.pssql".to_string()];
        let exemptions = scan_migrations_for_exemptions(migrations_dir, &applied).unwrap();

        assert_eq!(exemptions.len(), 0);
    }

    #[test]
    fn test_scan_multiple_exemptions_in_one_file() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
SELECT _stonescriptdb_gateway_change_column_type('items', 'qty', 'BIGINT', '_migrate_qty');
SELECT _stonescriptdb_gateway_change_column_type('items', 'price', 'NUMERIC(10,2)', '_migrate_price');
"#;

        fs::write(migrations_dir.join("001_changes.pssql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();

        assert_eq!(exemptions.len(), 2);
        assert_eq!(exemptions[0].column, "qty");
        assert_eq!(exemptions[1].column, "price");
    }

    #[test]
    fn test_scan_no_migrations_dir() {
        let temp_dir = TempDir::new().unwrap();
        let non_existent = temp_dir.path().join("does_not_exist");

        let exemptions = scan_migrations_for_exemptions(&non_existent, &[]).unwrap();
        assert_eq!(exemptions.len(), 0);
    }

    #[test]
    fn test_scan_no_exemptions() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = "ALTER TABLE items ADD COLUMN IF NOT EXISTS qty INTEGER DEFAULT 0;";
        fs::write(migrations_dir.join("001_add_col.pssql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();
        assert_eq!(exemptions.len(), 0);
    }
}
