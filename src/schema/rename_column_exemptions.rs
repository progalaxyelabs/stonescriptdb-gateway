//! Rename Column Exemption Scanner
//!
//! Scans pending migration files for calls to `_stonescriptdb_gateway_rename_column`.
//! Extracts (table, old_column, new_column) tuples so the diff checker can collapse
//! the apparent DropColumn(old) + AddColumn(new) pair into a no-op rename.
//!
//! The gateway performs no scaffolding around the rename itself — the developer's
//! helper function is solely responsible for issuing `ALTER TABLE ... RENAME COLUMN`.

use crate::error::{GatewayError, Result};
use crate::schema::MigrationRunner;
use regex::Regex;
use std::path::Path;
use tracing::{debug, info, warn};

/// Substring used to detect possibly-malformed calls (so we can warn on scanner misses).
const FN_TOKEN: &str = "_stonescriptdb_gateway_rename_column";

/// A column rename that is being handled by a gateway migration function.
#[derive(Debug, Clone)]
pub struct RenameColumnExemption {
    pub table: String,
    pub old_column: String,
    pub new_column: String,
    /// Developer-provided helper function name (captured for logs — when the
    /// helper errors during migration, operators can grep for this name).
    pub helper_fn: String,
    pub migration_file: String,
}

/// Scan pending migration files for calls to `_stonescriptdb_gateway_rename_column`.
///
/// Returns exemptions only for migrations that haven't been applied yet.
pub fn scan_migrations_for_exemptions(
    migrations_dir: &Path,
    applied_migrations: &[String],
) -> Result<Vec<RenameColumnExemption>> {
    let runner = MigrationRunner::new();
    let migration_files = runner.find_migration_files(migrations_dir)?;

    let mut exemptions = Vec::new();
    let mut files_scanned = 0usize;
    let mut files_skipped_applied = 0usize;

    // _stonescriptdb_gateway_rename_column('table', 'old_col', 'new_col', 'helper_fn')
    let re = Regex::new(
        r"(?si)_stonescriptdb_gateway_rename_column\s*\(\s*'([^']+)'\s*,\s*'([^']+)'\s*,\s*'([^']+)'\s*,\s*'([^']+)'\s*\)"
    ).map_err(|e| GatewayError::SchemaExtractionFailed {
        cause: format!("Failed to compile rename-exemption regex: {}", e),
    })?;

    for migration in &migration_files {
        if applied_migrations.contains(&migration.name) {
            debug!(
                "Rename scanner: skipping already-applied migration {}",
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
            let old_column = caps[2].to_string();
            let new_column = caps[3].to_string();
            let helper_fn = caps[4].to_string();
            captures_in_file += 1;

            info!(
                "Rename exemption registered: {}.{} -> {} via helper '{}' (migration: {})",
                table, old_column, new_column, helper_fn, migration.name
            );

            exemptions.push(RenameColumnExemption {
                table,
                old_column,
                new_column,
                helper_fn,
                migration_file: migration.name.clone(),
            });
        }

        // Detect malformed calls — function name appears but regex didn't capture.
        if captures_in_file == 0 && content.contains(FN_TOKEN) {
            warn!(
                "Rename scanner: {} contains '{}' but no calls matched the expected signature \
                 ('table','old_col','new_col','helper_fn') with single quotes. The exemption \
                 will NOT be applied — the migration will be blocked. Check for typos, double \
                 quotes, or line-broken args.",
                migration.name, FN_TOKEN
            );
        }
    }

    info!(
        "Rename scanner: {} exemption(s) across {} scanned file(s) ({} already-applied skipped)",
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
    fn test_scan_finds_rename_exemption() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
CREATE OR REPLACE FUNCTION _rename_user_id(p_table TEXT, p_old TEXT, p_new TEXT)
RETURNS void AS $$
BEGIN
    EXECUTE format('ALTER TABLE %I RENAME COLUMN %I TO %I', p_table, p_old, p_new);
END;
$$ LANGUAGE plpgsql;

SELECT _stonescriptdb_gateway_rename_column(
    'users', 'user_id', 'id', '_rename_user_id'
);
"#;

        fs::write(migrations_dir.join("001_rename.pgsql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();

        assert_eq!(exemptions.len(), 1);
        assert_eq!(exemptions[0].table, "users");
        assert_eq!(exemptions[0].old_column, "user_id");
        assert_eq!(exemptions[0].new_column, "id");
        assert_eq!(exemptions[0].helper_fn, "_rename_user_id");
        assert_eq!(exemptions[0].migration_file, "001_rename.pgsql");
    }

    #[test]
    fn test_scan_warns_on_malformed_call() {
        // File contains the function name but uses double quotes — regex misses.
        // The scanner must return 0 exemptions (and log a warn — we can't assert
        // on logs here, but we ensure no silent capture).
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
SELECT _stonescriptdb_gateway_rename_column("users", "user_id", "id", "_f");
"#;
        fs::write(migrations_dir.join("001_bad.pgsql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();
        assert_eq!(exemptions.len(), 0);
    }

    #[test]
    fn test_scan_skips_applied_migrations() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = "SELECT _stonescriptdb_gateway_rename_column('t', 'a', 'b', 'f');";
        fs::write(migrations_dir.join("001.pgsql"), sql).unwrap();

        let applied = vec!["001.pgsql".to_string()];
        let exemptions = scan_migrations_for_exemptions(migrations_dir, &applied).unwrap();
        assert_eq!(exemptions.len(), 0);
    }

    #[test]
    fn test_scan_multiple_renames_in_one_file() {
        let temp_dir = TempDir::new().unwrap();
        let migrations_dir = temp_dir.path();

        let sql = r#"
SELECT _stonescriptdb_gateway_rename_column('items', 'qty', 'quantity', '_f1');
SELECT _stonescriptdb_gateway_rename_column('items', 'prc', 'price', '_f2');
"#;
        fs::write(migrations_dir.join("001.pgsql"), sql).unwrap();

        let exemptions = scan_migrations_for_exemptions(migrations_dir, &[]).unwrap();
        assert_eq!(exemptions.len(), 2);
        assert_eq!(exemptions[0].old_column, "qty");
        assert_eq!(exemptions[0].new_column, "quantity");
        assert_eq!(exemptions[1].old_column, "prc");
        assert_eq!(exemptions[1].new_column, "price");
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
