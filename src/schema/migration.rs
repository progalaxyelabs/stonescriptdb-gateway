use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct MigrationFile {
    pub name: String,
    pub path: PathBuf,
    pub checksum: String,
}

pub struct MigrationRunner;

impl MigrationRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn ensure_migrations_table(&self, pool: &Pool, database: &str) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _stonescriptdb_gateway_migrations (
                    id SERIAL PRIMARY KEY,
                    migration_file TEXT NOT NULL UNIQUE,
                    checksum TEXT NOT NULL,
                    applied_at TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "_stonescriptdb_gateway_migrations table creation".to_string(),
                cause: e.to_string(),
            })?;

        Ok(())
    }

    pub async fn get_applied_migrations(&self, pool: &Pool, database: &str) -> Result<Vec<String>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let rows = client
            .query(
                "SELECT migration_file FROM _stonescriptdb_gateway_migrations ORDER BY id",
                &[],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "query applied migrations".to_string(),
                cause: e.to_string(),
            })?;

        Ok(rows.iter().map(|row| row.get(0)).collect())
    }

    pub fn find_migration_files(&self, migrations_dir: &Path) -> Result<Vec<MigrationFile>> {
        if !migrations_dir.exists() {
            debug!(
                "Migrations directory {:?} does not exist, returning empty list",
                migrations_dir
            );
            return Ok(Vec::new());
        }

        let mut migrations = Vec::new();

        for entry in fs::read_dir(migrations_dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read migrations directory: {}", e),
        })? {
            let entry = entry.map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "pssql" {
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();

                        let content = fs::read_to_string(&path).map_err(|e| {
                            GatewayError::SchemaExtractionFailed {
                                cause: format!("Failed to read migration file {:?}: {}", path, e),
                            }
                        })?;

                        let checksum = compute_checksum(&content);

                        migrations.push(MigrationFile {
                            name,
                            path,
                            checksum,
                        });
                    }
                }
            }
        }

        // Sort by filename (which should have numeric prefixes like 001_, 002_, etc.)
        migrations.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(migrations)
    }

    pub async fn run_migrations(
        &self,
        pool: &Pool,
        database: &str,
        migrations_dir: &Path,
    ) -> Result<usize> {
        // Ensure migrations table exists
        self.ensure_migrations_table(pool, database).await?;

        // Get already applied migrations
        let applied = self.get_applied_migrations(pool, database).await?;
        debug!(
            "Database {} has {} applied migrations",
            database,
            applied.len()
        );

        // Find migration files
        let migration_files = self.find_migration_files(migrations_dir)?;
        debug!(
            "Found {} migration files in {:?}",
            migration_files.len(),
            migrations_dir
        );

        let mut count = 0;

        for migration in migration_files {
            if applied.contains(&migration.name) {
                debug!("Skipping already applied migration: {}", migration.name);
                continue;
            }

            info!("Applying migration: {} to {}", migration.name, database);

            // Read and execute migration
            let sql = fs::read_to_string(&migration.path).map_err(|e| {
                GatewayError::MigrationFailed {
                    database: database.to_string(),
                    migration: migration.name.clone(),
                    cause: format!("Failed to read file: {}", e),
                }
            })?;

            let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
                database: database.to_string(),
                cause: e.to_string(),
            })?;

            client
                .batch_execute(&sql)
                .await
                .map_err(|e| GatewayError::MigrationFailed {
                    database: database.to_string(),
                    migration: migration.name.clone(),
                    cause: e.to_string(),
                })?;

            // Record the migration
            client
                .execute(
                    "INSERT INTO _stonescriptdb_gateway_migrations (migration_file, checksum) VALUES ($1, $2)",
                    &[&migration.name, &migration.checksum],
                )
                .await
                .map_err(|e| GatewayError::MigrationFailed {
                    database: database.to_string(),
                    migration: migration.name.clone(),
                    cause: format!("Failed to record migration: {}", e),
                })?;

            count += 1;
            info!(
                "Successfully applied migration: {} (checksum: {})",
                migration.name, migration.checksum
            );
        }

        Ok(count)
    }

    pub async fn verify_checksum(
        &self,
        pool: &Pool,
        database: &str,
        migration_name: &str,
        expected_checksum: &str,
    ) -> Result<bool> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let row = client
            .query_opt(
                "SELECT checksum FROM _stonescriptdb_gateway_migrations WHERE migration_file = $1",
                &[&migration_name],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: migration_name.to_string(),
                cause: e.to_string(),
            })?;

        match row {
            Some(row) => {
                let stored_checksum: String = row.get(0);
                if stored_checksum != expected_checksum {
                    warn!(
                        "Checksum mismatch for migration {} in {}: stored={}, expected={}",
                        migration_name, database, stored_checksum, expected_checksum
                    );
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
            None => Ok(false),
        }
    }
}

impl Default for MigrationRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn compute_checksum(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_checksum() {
        let content = "CREATE TABLE test (id INT);";
        let checksum = compute_checksum(content);
        assert_eq!(checksum.len(), 64); // SHA256 produces 64 hex characters

        // Same content should produce same checksum
        let checksum2 = compute_checksum(content);
        assert_eq!(checksum, checksum2);

        // Different content should produce different checksum
        let checksum3 = compute_checksum("CREATE TABLE other (id INT);");
        assert_ne!(checksum, checksum3);
    }
}
