//! Gateway-provided PostgreSQL functions
//!
//! Functions installed by the gateway into every managed database.
//! These provide safe, structured operations that the diff checker
//! can recognize and exempt from compatibility checks.
//!
//! # Philosophy
//!
//! Each gateway function is a thin wrapper that delegates to a developer-provided
//! migration helper function. The gateway makes NO assumptions — no scaffolding,
//! no dependency enumeration, no cascade policy. The developer's helper function
//! does ALL the SQL work.
//!
//! In particular, if a developer needs `DROP COLUMN ... CASCADE` or must handle
//! dependent views / foreign keys, they write that SQL in their own helper. The
//! gateway only records the exemption so the schema-diff checker accepts the
//! resulting state.

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use tracing::debug;

/// Thin wrapper for column type changes. Developer's helper does all the work:
/// type checking, column creation, data conversion, column drop, column rename.
///
/// Developer's helper signature:
///   (p_table TEXT, p_column TEXT, p_new_type TEXT) RETURNS void
const CHANGE_COLUMN_TYPE_SQL: &str = r#"
CREATE OR REPLACE FUNCTION _stonescriptdb_gateway_change_column_type(
    p_table_name TEXT,
    p_column_name TEXT,
    p_new_type TEXT,
    p_migration_function TEXT
) RETURNS void AS $$
BEGIN
    EXECUTE format('SELECT %I(%L, %L, %L)', p_migration_function, p_table_name, p_column_name, p_new_type);
END;
$$ LANGUAGE plpgsql;
"#;

/// Thin wrapper for column renames. Developer's helper runs `ALTER TABLE ... RENAME COLUMN`.
///
/// The scanner regex-extracts (table, old_column, new_column) from this call so the diff
/// checker collapses the apparent DropColumn(old) + AddColumn(new) pair into a no-op.
///
/// Developer's helper signature:
///   (p_table TEXT, p_old_column TEXT, p_new_column TEXT) RETURNS void
const RENAME_COLUMN_SQL: &str = r#"
CREATE OR REPLACE FUNCTION _stonescriptdb_gateway_rename_column(
    p_table_name TEXT,
    p_old_column_name TEXT,
    p_new_column_name TEXT,
    p_migration_function TEXT
) RETURNS void AS $$
BEGIN
    EXECUTE format('SELECT %I(%L, %L, %L)', p_migration_function, p_table_name, p_old_column_name, p_new_column_name);
END;
$$ LANGUAGE plpgsql;
"#;

/// Thin wrapper for intentional column drops. Developer's helper runs the actual
/// `ALTER TABLE ... DROP COLUMN` — with or without CASCADE, as the developer chooses.
///
/// Cascade, dependent views, foreign-key policy, and data preservation are ALL the
/// developer's responsibility. The gateway only records the exemption so the
/// schema-diff checker accepts the resulting state.
///
/// Developer's helper signature:
///   (p_table TEXT, p_column TEXT) RETURNS void
const DROP_COLUMN_SQL: &str = r#"
CREATE OR REPLACE FUNCTION _stonescriptdb_gateway_drop_column(
    p_table_name TEXT,
    p_column_name TEXT,
    p_migration_function TEXT
) RETURNS void AS $$
BEGIN
    EXECUTE format('SELECT %I(%L, %L)', p_migration_function, p_table_name, p_column_name);
END;
$$ LANGUAGE plpgsql;
"#;

/// Installs gateway-provided functions into a database.
pub struct GatewayFunctionInstaller;

impl GatewayFunctionInstaller {
    /// Install all gateway functions into the database (idempotent via CREATE OR REPLACE).
    pub async fn ensure_installed(pool: &Pool, database: &str) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let combined = format!(
            "{}\n{}\n{}",
            CHANGE_COLUMN_TYPE_SQL, RENAME_COLUMN_SQL, DROP_COLUMN_SQL
        );

        client
            .batch_execute(&combined)
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "gateway-provided functions".to_string(),
                cause: e.to_string(),
            })?;

        debug!(
            "Gateway functions installed in database '{}'",
            database
        );
        Ok(())
    }
}

impl Default for GatewayFunctionInstaller {
    fn default() -> Self {
        Self
    }
}
