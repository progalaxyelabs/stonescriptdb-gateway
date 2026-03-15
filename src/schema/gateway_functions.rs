//! Gateway-provided PostgreSQL functions
//!
//! Functions installed by the gateway into every managed database.
//! These provide safe, structured operations that the diff checker
//! can recognize and exempt from compatibility checks.

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use tracing::debug;

/// SQL for the column type change function.
///
/// This is a thin wrapper that the diff checker can recognize via regex.
/// It simply delegates to the developer's migration function which is
/// responsible for ALL the work: type checking, column creation,
/// data conversion, column drop, and column rename.
///
/// The developer's migration function must have the signature:
///   (p_table TEXT, p_column TEXT, p_new_type TEXT) RETURNS void
///
/// The gateway makes NO assumptions — no scaffolding, no defaults, nothing.
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

/// Installs gateway-provided functions into a database.
pub struct GatewayFunctionInstaller;

impl GatewayFunctionInstaller {
    /// Install all gateway functions into the database (idempotent via CREATE OR REPLACE).
    pub async fn ensure_installed(pool: &Pool, database: &str) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        client
            .batch_execute(CHANGE_COLUMN_TYPE_SQL)
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "_stonescriptdb_gateway_change_column_type function".to_string(),
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
