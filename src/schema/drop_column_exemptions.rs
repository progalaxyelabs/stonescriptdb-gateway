//! Drop Column Exemption Scanner
//!
//! Scans pending migration files for calls to `_stonescriptdb_gateway_drop_column`.
//! Extracts (table, column) tuples so the diff checker treats that specific
//! DropColumn as Safe instead of DataLoss.
//!
//! # Cascade is the developer's responsibility
//!
//! The gateway does NOT enumerate dependents, does NOT require a declared
//! dependent list, and does NOT impose a cascade policy. If the developer needs
//! `DROP COLUMN ... CASCADE` or must manually drop dependent views / foreign-key
//! constraints / functions first, they do that in their own helper function.
//!
//! The gateway's only job here is to record the intentional-drop exemption so
//! the schema-diff checker accepts the resulting state. Consistent with
//! `_stonescriptdb_gateway_change_column_type` — the gateway makes no
//! assumptions about the SQL the developer runs.

use crate::error::{GatewayError, Result};
use crate::schema::MigrationRunner;
use regex::Regex;
use std::path::Path;
use tracing::{debug, info, warn};

const FN_TOKEN: &str = "_stonescriptdb_gateway_drop_column";

/// A column drop that is being handled by a gateway migration function.
#[derive(Debug, Clone)]
pub struct DropColumnExemption {
    pub table: String,
    pub column: String,
    /// Developer-provided helper function name (captured for logs — when the
    /// helper errors during migration, operators can grep for this name).
    pub helper_fn: String,
    pub migration_file: String,
}

/// Scan pending migration files for calls to `_stonescriptdb_gateway_drop_column`.
///
/// Returns exemptions only for migrations that haven't been applied yet.
pub fn scan_migrations_for_exemptions(
    migrations_dir: &Path,
    applied_migrations: &[String],
) -> Result<Vec<DropColumnExemption>> {
    let runner = MigrationRunner::new();
    let migration_files = runner.find_migration_files(migrations_dir)?;

    let mut exemptions = Vec::new();
    let mut files_scanned = 0usize;
    let mut files_skipped_applied = 0usize;

    // _stonescriptdb_gateway_drop_column('table', 'column', 'helper_fn')
    let re = Regex::new(
        r"(?si)_stonescriptdb_gateway_drop_column\s*\(\s*'([^']+)'\s*,\s*'([^']+)'\s*,\s*'([^']+)'\s*\)"
    ).map_err(|e| GatewayError::SchemaExtractionFailed {
        cause: format!("Failed to compile drop-exemption regex: {}", e),
    })?;

    for migration in &migration_files {
        if applied_migrations.contains(&migration.name) {
            debug!(
                "Drop scanner: skipping already-applied migration {}",
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
            let helper_fn = caps[3].to_string();
            captures_in_file += 1;

            info!(
                "Drop exemption registered: {}.{} via helper '{}' (migration: {}; cascade is developer's responsibility)",
                table, column, helper_fn, migration.name
            );

            exemptions.push(DropColumnExemption {
                table,
                column,
                helper_fn,
                migration_file: migration.name.clone(),
            });
        }

        if captures_in_file == 0 && content.contains(FN_TOKEN) {
            warn!(
                "Drop scanner: {} contains '{}' but no calls matched the expected signature \
                 ('table','column','helper_fn') with single quotes. The exemption will NOT be \
                 applied — the migration will be blocked. Check for typos, double quotes, or \
                 line-broken args.",
                migration.name, FN_TOKEN
            );
        }
    }

    info!(
        "Drop scanner: {} exemption(s) across {} scanned file(s) ({} already-applied skipped)",
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
    fn test_scan_finds_drop_exemption() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
CREATE OR REPLACE FUNCTION _drop_legacy_col(p_table TEXT, p_col TEXT)
RETURNS void AS $$
BEGIN
    EXECUTE format('ALTER TABLE %I DROP COLUMN %I CASCADE', p_table, p_col);
END;
$$ LANGUAGE plpgsql;

SELECT _stonescriptdb_gateway_drop_column(
    'users', 'legacy_field', '_drop_legacy_col'
);
"#;

        fs::write(migrations_dir.join("001_drop.pgsql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();

        assert_eq!(exemptions.len(), 1);
        assert_eq!(exemptions[0].table, "users");
        assert_eq!(exemptions[0].column, "legacy_field");
        assert_eq!(exemptions[0].helper_fn, "_drop_legacy_col");
        assert_eq!(exemptions[0].migration_file, "001_drop.pgsql");
    }

    #[test]
    fn test_scan_warns_on_malformed_call() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
SELECT _stonescriptdb_gateway_drop_column("users", "legacy", "_f");
"#;
        fs::write(migrations_dir.join("001_bad.pgsql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();
        assert_eq!(exemptions.len(), 0);
    }

    #[test]
    fn test_scan_skips_applied_migrations() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = "SELECT _stonescriptdb_gateway_drop_column('t', 'c', 'f');";
        fs::write(migrations_dir.join("001.pgsql"), sql).unwrap();

        let applied = vec!["001.pgsql".to_string()];
        let exemptions = scan_migrations_for_exemptions(migrations_dir, &applied).unwrap();
        assert_eq!(exemptions.len(), 0);
    }

    #[test]
    fn test_scan_multiple_drops_in_one_file() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
SELECT _stonescriptdb_gateway_drop_column('items', 'deprecated1', '_f1');
SELECT _stonescriptdb_gateway_drop_column('items', 'deprecated2', '_f2');
"#;
        fs::write(migrations_dir.join("001.pgsql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();
        assert_eq!(exemptions.len(), 2);
        assert_eq!(exemptions[0].column, "deprecated1");
        assert_eq!(exemptions[1].column, "deprecated2");
    }

    #[test]
    fn test_scan_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        fs::write(
            migrations_dir.join("001.pgsql"),
            "ALTER TABLE t ADD COLUMN x INTEGER;",
        )
        .unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();
        assert_eq!(exemptions.len(), 0);
    }
}
