//! PostgreSQL extension manager
//!
//! Handles installation of PostgreSQL extensions like uuid-ossp, pgvector, etc.
//! Extensions are defined in the `extensions/` folder with one file per extension.

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Represents a PostgreSQL extension to be installed
#[derive(Debug, Clone)]
pub struct Extension {
    pub name: String,
    pub version: Option<String>,
    pub schema: Option<String>,
}

pub struct ExtensionManager;

impl ExtensionManager {
    pub fn new() -> Self {
        Self
    }

    /// Find extension files in the extensions directory
    /// Supports .pssql, .pgsql, .sql, and .txt files
    /// File format: extension name as filename, optional version/schema in content
    pub fn find_extension_files(&self, extensions_dir: &Path) -> Result<Vec<PathBuf>> {
        if !extensions_dir.exists() {
            debug!(
                "Extensions directory {:?} does not exist, returning empty list",
                extensions_dir
            );
            return Ok(Vec::new());
        }

        let mut files = Vec::new();

        for entry in fs::read_dir(extensions_dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read extensions directory: {}", e),
        })? {
            let entry = entry.map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "pssql" || ext == "pgsql" || ext == "sql" || ext == "txt" {
                        files.push(path);
                    }
                }
            }
        }

        // Sort for consistent ordering
        files.sort();

        Ok(files)
    }

    /// Parse extension definition from file
    ///
    /// Simple format - just the extension name in filename:
    ///   uuid-ossp.sql (empty or with comments)
    ///
    /// Advanced format - with options in content:
    ///   -- version: 1.1
    ///   -- schema: extensions
    pub fn parse_extension(&self, file_path: &Path) -> Result<Extension> {
        let file_name = file_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Extension name from filename
        let name = file_name.to_string();

        // Read file content for optional version/schema
        let content = fs::read_to_string(file_path).unwrap_or_default();

        let mut version: Option<String> = None;
        let mut schema: Option<String> = None;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("--") {
                let comment = line.trim_start_matches("--").trim();
                if let Some(v) = comment.strip_prefix("version:") {
                    version = Some(v.trim().to_string());
                } else if let Some(s) = comment.strip_prefix("schema:") {
                    schema = Some(s.trim().to_string());
                }
            }
        }

        Ok(Extension { name, version, schema })
    }

    /// Install extensions in the database
    /// Returns the number of extensions installed
    pub async fn install_extensions(
        &self,
        pool: &Pool,
        database: &str,
        extensions_dir: &Path,
    ) -> Result<usize> {
        let extension_files = self.find_extension_files(extensions_dir)?;

        if extension_files.is_empty() {
            debug!("No extensions to install for database {}", database);
            return Ok(0);
        }

        debug!(
            "Found {} extension files in {:?}",
            extension_files.len(),
            extensions_dir
        );

        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let mut installed = 0;
        let mut skipped = 0;

        for file_path in &extension_files {
            let extension = self.parse_extension(file_path)?;

            // Check if extension already exists
            let exists = self.extension_exists(&client, &extension.name).await?;

            if exists {
                debug!("Extension {} already installed, skipping", extension.name);
                skipped += 1;
                continue;
            }

            // Build CREATE EXTENSION statement
            let sql = self.build_create_extension_sql(&extension);

            debug!("Installing extension: {} in {}", extension.name, database);

            match client.execute(&sql, &[]).await {
                Ok(_) => {
                    info!("Installed extension {} in database {}", extension.name, database);
                    installed += 1;
                }
                Err(e) => {
                    // Check if it's a "extension not available" error
                    let err_str = e.to_string();
                    if err_str.contains("could not open extension control file")
                        || err_str.contains("extension") && err_str.contains("is not available") {
                        warn!(
                            "Extension {} not available on this PostgreSQL server: {}",
                            extension.name, e
                        );
                        return Err(GatewayError::ExtensionNotAvailable {
                            extension: extension.name.clone(),
                            cause: e.to_string(),
                        });
                    }
                    return Err(GatewayError::ExtensionInstallFailed {
                        database: database.to_string(),
                        extension: extension.name.clone(),
                        cause: e.to_string(),
                    });
                }
            }
        }

        info!(
            "Extension installation complete for {}: {} installed, {} skipped",
            database, installed, skipped
        );

        Ok(installed)
    }

    /// Check if an extension is already installed
    async fn extension_exists(
        &self,
        client: &deadpool_postgres::Object,
        extension_name: &str,
    ) -> Result<bool> {
        let row = client
            .query_opt(
                "SELECT 1 FROM pg_extension WHERE extname = $1",
                &[&extension_name],
            )
            .await
            .unwrap_or(None);

        Ok(row.is_some())
    }

    /// Build CREATE EXTENSION SQL statement
    fn build_create_extension_sql(&self, extension: &Extension) -> String {
        let mut sql = format!("CREATE EXTENSION IF NOT EXISTS \"{}\"", extension.name);

        if let Some(ref schema) = extension.schema {
            sql.push_str(&format!(" SCHEMA \"{}\"", schema));
        }

        if let Some(ref version) = extension.version {
            sql.push_str(&format!(" VERSION '{}'", version));
        }

        sql
    }

    /// Get list of installed extensions in database
    pub async fn list_extensions(
        &self,
        pool: &Pool,
        database: &str,
    ) -> Result<Vec<String>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let rows = client
            .query("SELECT extname FROM pg_extension ORDER BY extname", &[])
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: database.to_string(),
                function: "list_extensions".to_string(),
                cause: e.to_string(),
            })?;

        let extensions: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        Ok(extensions)
    }
}

impl Default for ExtensionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_simple_extension() {
        let manager = ExtensionManager::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("uuid-ossp.sql");
        fs::write(&file_path, "-- UUID extension\n").unwrap();

        let ext = manager.parse_extension(&file_path).unwrap();
        assert_eq!(ext.name, "uuid-ossp");
        assert!(ext.version.is_none());
        assert!(ext.schema.is_none());
    }

    #[test]
    fn test_parse_extension_with_options() {
        let manager = ExtensionManager::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("pgvector.sql");
        let content = r#"
-- PostgreSQL vector similarity search
-- version: 0.5.0
-- schema: extensions
"#;
        fs::write(&file_path, content).unwrap();

        let ext = manager.parse_extension(&file_path).unwrap();
        assert_eq!(ext.name, "pgvector");
        assert_eq!(ext.version, Some("0.5.0".to_string()));
        assert_eq!(ext.schema, Some("extensions".to_string()));
    }

    #[test]
    fn test_build_create_extension_sql_simple() {
        let manager = ExtensionManager::new();
        let ext = Extension {
            name: "uuid-ossp".to_string(),
            version: None,
            schema: None,
        };

        let sql = manager.build_create_extension_sql(&ext);
        assert_eq!(sql, "CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\"");
    }

    #[test]
    fn test_build_create_extension_sql_with_options() {
        let manager = ExtensionManager::new();
        let ext = Extension {
            name: "pgvector".to_string(),
            version: Some("0.5.0".to_string()),
            schema: Some("extensions".to_string()),
        };

        let sql = manager.build_create_extension_sql(&ext);
        assert_eq!(
            sql,
            "CREATE EXTENSION IF NOT EXISTS \"pgvector\" SCHEMA \"extensions\" VERSION '0.5.0'"
        );
    }

    #[test]
    fn test_find_extension_files() {
        let manager = ExtensionManager::new();
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        fs::write(temp_dir.path().join("uuid-ossp.sql"), "").unwrap();
        fs::write(temp_dir.path().join("pgvector.pgsql"), "").unwrap();
        fs::write(temp_dir.path().join("postgis.txt"), "").unwrap();
        fs::write(temp_dir.path().join("readme.md"), "").unwrap(); // Should be ignored

        let files = manager.find_extension_files(temp_dir.path()).unwrap();
        assert_eq!(files.len(), 3);
    }
}
