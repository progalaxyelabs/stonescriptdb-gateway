//! Changelog tracking for schema changes
//!
//! Tracks all schema changes (migrations, function deployments, extensions)
//! for audit and debugging purposes.

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::debug;

/// Types of schema changes that can be tracked
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    MigrationApplied,
    FunctionDeployed,
    FunctionDropped,
    FunctionSkipped,
    ExtensionInstalled,
    ExtensionSkipped,
    SeederRun,
    SeederSkipped,
    SeederValidated,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::MigrationApplied => write!(f, "migration_applied"),
            ChangeType::FunctionDeployed => write!(f, "function_deployed"),
            ChangeType::FunctionDropped => write!(f, "function_dropped"),
            ChangeType::FunctionSkipped => write!(f, "function_skipped"),
            ChangeType::ExtensionInstalled => write!(f, "extension_installed"),
            ChangeType::ExtensionSkipped => write!(f, "extension_skipped"),
            ChangeType::SeederRun => write!(f, "seeder_run"),
            ChangeType::SeederSkipped => write!(f, "seeder_skipped"),
            ChangeType::SeederValidated => write!(f, "seeder_validated"),
        }
    }
}

/// Details about a changelog entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub change_type: ChangeType,
    pub object_name: String,
    pub details: Option<JsonValue>,
    pub forced: bool,
}

/// Manager for changelog operations
pub struct ChangelogManager;

impl ChangelogManager {
    pub fn new() -> Self {
        Self
    }

    /// Ensure the changelog table exists
    pub async fn ensure_changelog_table(&self, pool: &Pool, database: &str) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _stonescriptdb_gateway_changelog (
                    id SERIAL PRIMARY KEY,
                    change_type TEXT NOT NULL,
                    object_name TEXT NOT NULL,
                    change_detail JSONB,
                    forced BOOLEAN DEFAULT FALSE,
                    executed_at TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "_stonescriptdb_gateway_changelog table creation".to_string(),
                cause: e.to_string(),
            })?;

        // Create index for faster queries by change_type
        client
            .execute(
                r#"
                CREATE INDEX IF NOT EXISTS idx_changelog_change_type
                ON _stonescriptdb_gateway_changelog (change_type)
                "#,
                &[],
            )
            .await
            .ok(); // Ignore if exists

        // Create index for faster queries by object_name
        client
            .execute(
                r#"
                CREATE INDEX IF NOT EXISTS idx_changelog_object_name
                ON _stonescriptdb_gateway_changelog (object_name)
                "#,
                &[],
            )
            .await
            .ok(); // Ignore if exists

        // Create index for faster queries by executed_at
        client
            .execute(
                r#"
                CREATE INDEX IF NOT EXISTS idx_changelog_executed_at
                ON _stonescriptdb_gateway_changelog (executed_at DESC)
                "#,
                &[],
            )
            .await
            .ok(); // Ignore if exists

        debug!("Changelog table ensured for database {}", database);
        Ok(())
    }

    /// Log a single changelog entry
    pub async fn log_change(
        &self,
        pool: &Pool,
        database: &str,
        entry: &ChangelogEntry,
    ) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let change_type = entry.change_type.to_string();
        let detail_json = entry.details.as_ref().map(|d| d.to_string());

        client
            .execute(
                r#"
                INSERT INTO _stonescriptdb_gateway_changelog
                    (change_type, object_name, change_detail, forced)
                VALUES ($1, $2, $3::jsonb, $4)
                "#,
                &[&change_type, &entry.object_name, &detail_json, &entry.forced],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "changelog entry".to_string(),
                cause: format!("Failed to log changelog entry: {}", e),
            })?;

        debug!(
            "Logged changelog: {} - {} (forced: {})",
            change_type, entry.object_name, entry.forced
        );
        Ok(())
    }

    /// Log a migration applied
    pub async fn log_migration(
        &self,
        pool: &Pool,
        database: &str,
        migration_name: &str,
        checksum: &str,
    ) -> Result<()> {
        let details = serde_json::json!({
            "checksum": checksum
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::MigrationApplied,
                object_name: migration_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Log a function deployment
    pub async fn log_function_deployed(
        &self,
        pool: &Pool,
        database: &str,
        function_name: &str,
        signature: &str,
        checksum: &str,
        source_file: &str,
    ) -> Result<()> {
        let details = serde_json::json!({
            "signature": signature,
            "checksum": checksum,
            "source_file": source_file
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::FunctionDeployed,
                object_name: function_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Log a function dropped (due to signature change)
    pub async fn log_function_dropped(
        &self,
        pool: &Pool,
        database: &str,
        function_name: &str,
        old_signature: &str,
        reason: &str,
    ) -> Result<()> {
        let details = serde_json::json!({
            "old_signature": old_signature,
            "reason": reason
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::FunctionDropped,
                object_name: function_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Log a function skipped (unchanged checksum)
    pub async fn log_function_skipped(
        &self,
        pool: &Pool,
        database: &str,
        function_name: &str,
    ) -> Result<()> {
        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::FunctionSkipped,
                object_name: function_name.to_string(),
                details: None,
                forced: false,
            },
        )
        .await
    }

    /// Log an extension installed
    pub async fn log_extension_installed(
        &self,
        pool: &Pool,
        database: &str,
        extension_name: &str,
        version: Option<&str>,
        schema: Option<&str>,
    ) -> Result<()> {
        let details = serde_json::json!({
            "version": version,
            "schema": schema
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::ExtensionInstalled,
                object_name: extension_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Log an extension skipped (already installed)
    pub async fn log_extension_skipped(
        &self,
        pool: &Pool,
        database: &str,
        extension_name: &str,
    ) -> Result<()> {
        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::ExtensionSkipped,
                object_name: extension_name.to_string(),
                details: None,
                forced: false,
            },
        )
        .await
    }

    /// Log a seeder run
    pub async fn log_seeder_run(
        &self,
        pool: &Pool,
        database: &str,
        table_name: &str,
        inserted: usize,
        skipped: usize,
    ) -> Result<()> {
        let details = serde_json::json!({
            "inserted": inserted,
            "skipped": skipped
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::SeederRun,
                object_name: table_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Log a seeder skipped (table not empty)
    pub async fn log_seeder_skipped(
        &self,
        pool: &Pool,
        database: &str,
        table_name: &str,
        reason: &str,
    ) -> Result<()> {
        let details = serde_json::json!({
            "reason": reason
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::SeederSkipped,
                object_name: table_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Log seeder validation
    pub async fn log_seeder_validated(
        &self,
        pool: &Pool,
        database: &str,
        table_name: &str,
        expected: usize,
        found: usize,
    ) -> Result<()> {
        let details = serde_json::json!({
            "expected": expected,
            "found": found
        });

        self.log_change(
            pool,
            database,
            &ChangelogEntry {
                change_type: ChangeType::SeederValidated,
                object_name: table_name.to_string(),
                details: Some(details),
                forced: false,
            },
        )
        .await
    }

    /// Get recent changelog entries
    pub async fn get_recent_entries(
        &self,
        pool: &Pool,
        database: &str,
        limit: i64,
    ) -> Result<Vec<ChangelogRecord>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, change_type, object_name, change_detail, forced, executed_at
                FROM _stonescriptdb_gateway_changelog
                ORDER BY executed_at DESC
                LIMIT $1
                "#,
                &[&limit],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "query changelog".to_string(),
                cause: e.to_string(),
            })?;

        let mut entries = Vec::new();
        for row in rows {
            let detail_str: Option<String> = row.get(3);
            let change_detail = detail_str
                .and_then(|s| serde_json::from_str(&s).ok());

            entries.push(ChangelogRecord {
                id: row.get(0),
                change_type: row.get(1),
                object_name: row.get(2),
                change_detail,
                forced: row.get(4),
                executed_at: row.get(5),
            });
        }

        Ok(entries)
    }

    /// Get changelog entries by type
    pub async fn get_entries_by_type(
        &self,
        pool: &Pool,
        database: &str,
        change_type: &str,
        limit: i64,
    ) -> Result<Vec<ChangelogRecord>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, change_type, object_name, change_detail, forced, executed_at
                FROM _stonescriptdb_gateway_changelog
                WHERE change_type = $1
                ORDER BY executed_at DESC
                LIMIT $2
                "#,
                &[&change_type, &limit],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "query changelog by type".to_string(),
                cause: e.to_string(),
            })?;

        let mut entries = Vec::new();
        for row in rows {
            let detail_str: Option<String> = row.get(3);
            let change_detail = detail_str
                .and_then(|s| serde_json::from_str(&s).ok());

            entries.push(ChangelogRecord {
                id: row.get(0),
                change_type: row.get(1),
                object_name: row.get(2),
                change_detail,
                forced: row.get(4),
                executed_at: row.get(5),
            });
        }

        Ok(entries)
    }
}

impl Default for ChangelogManager {
    fn default() -> Self {
        Self::new()
    }
}

/// A record from the changelog table
#[derive(Debug, Clone, Serialize)]
pub struct ChangelogRecord {
    pub id: i32,
    pub change_type: String,
    pub object_name: String,
    pub change_detail: Option<JsonValue>,
    pub forced: bool,
    pub executed_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_type_display() {
        assert_eq!(ChangeType::MigrationApplied.to_string(), "migration_applied");
        assert_eq!(ChangeType::FunctionDeployed.to_string(), "function_deployed");
        assert_eq!(ChangeType::ExtensionInstalled.to_string(), "extension_installed");
    }

    #[test]
    fn test_changelog_entry_serialization() {
        let entry = ChangelogEntry {
            change_type: ChangeType::MigrationApplied,
            object_name: "001_create_users.pssql".to_string(),
            details: Some(serde_json::json!({"checksum": "abc123"})),
            forced: false,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("migration_applied"));
        assert!(json.contains("001_create_users.pssql"));
    }
}
