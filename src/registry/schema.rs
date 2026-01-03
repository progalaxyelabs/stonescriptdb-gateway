//! Schema Store
//!
//! Manages schema storage and retrieval for platforms.
//! Each schema is stored as a directory with subdirectories for each component.

use crate::error::{GatewayError, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tar::Archive;
use tracing::info;

/// Information about a stored schema
#[derive(Debug, Clone)]
pub struct StoredSchema {
    pub name: String,
    pub path: PathBuf,
    pub checksum: String,
    pub has_extensions: bool,
    pub has_types: bool,
    pub has_tables: bool,
    pub has_functions: bool,
    pub has_seeders: bool,
    pub has_migrations: bool,
}

/// Schema store for managing schema files
pub struct SchemaStore {
    data_dir: PathBuf,
}

impl SchemaStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Get the directory for a schema
    pub fn schema_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.data_dir.join(platform).join(schema_name)
    }

    /// Check if a schema exists
    pub fn schema_exists(&self, platform: &str, schema_name: &str) -> bool {
        self.schema_dir(platform, schema_name).exists()
    }

    /// Store a schema from a tar.gz archive
    ///
    /// The archive should contain:
    /// - extensions/ (optional)
    /// - types/ (optional)
    /// - tables/
    /// - functions/
    /// - seeders/ (optional)
    /// - migrations/ (optional)
    pub fn store_schema(
        &self,
        platform: &str,
        schema_name: &str,
        archive_data: &[u8],
    ) -> Result<StoredSchema> {
        // Validate schema name
        if !is_valid_identifier(schema_name) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Invalid schema name: {}. Must be alphanumeric with underscores.", schema_name),
            });
        }

        let schema_dir = self.schema_dir(platform, schema_name);

        // Remove existing schema if present
        if schema_dir.exists() {
            fs::remove_dir_all(&schema_dir).map_err(|e| GatewayError::Internal(
                format!("Failed to remove existing schema: {}", e)
            ))?;
        }

        // Create schema directory
        fs::create_dir_all(&schema_dir).map_err(|e| GatewayError::Internal(
            format!("Failed to create schema directory: {}", e)
        ))?;

        // Compute checksum
        let checksum = compute_checksum(archive_data);

        // Extract archive
        let decoder = GzDecoder::new(archive_data);
        let mut archive = Archive::new(decoder);

        for entry in archive.entries().map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read archive entries: {}", e),
        })? {
            let mut entry = entry.map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read entry: {}", e),
            })?;

            let path = entry.path().map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to get entry path: {}", e),
            })?.to_path_buf();

            // Skip the root "postgresql/" prefix if present
            let relative_path = path.strip_prefix("postgresql/")
                .or_else(|_| path.strip_prefix("postgresql"))
                .unwrap_or(&path)
                .to_path_buf();

            if relative_path.as_os_str().is_empty() {
                continue;
            }

            let target_path = schema_dir.join(&relative_path);

            // Create parent directories
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).ok();
            }

            // Extract file
            if entry.header().entry_type().is_file() {
                entry.unpack(&target_path).map_err(|e| GatewayError::SchemaExtractionFailed {
                    cause: format!("Failed to extract {}: {}", relative_path.display(), e),
                })?;
            } else if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&target_path).ok();
            }
        }

        // Build schema info
        let schema = StoredSchema {
            name: schema_name.to_string(),
            path: schema_dir.clone(),
            checksum,
            has_extensions: schema_dir.join("extensions").exists(),
            has_types: schema_dir.join("types").exists(),
            has_tables: schema_dir.join("tables").exists(),
            has_functions: schema_dir.join("functions").exists(),
            has_seeders: schema_dir.join("seeders").exists(),
            has_migrations: schema_dir.join("migrations").exists(),
        };

        info!(
            "Stored schema '{}' for platform '{}' (tables={}, functions={}, migrations={})",
            schema_name, platform, schema.has_tables, schema.has_functions, schema.has_migrations
        );

        Ok(schema)
    }

    /// Get a stored schema
    pub fn get_schema(&self, platform: &str, schema_name: &str) -> Result<StoredSchema> {
        let schema_dir = self.schema_dir(platform, schema_name);

        if !schema_dir.exists() {
            return Err(GatewayError::InvalidRequest {
                message: format!("Schema '{}' not found for platform '{}'", schema_name, platform),
            });
        }

        // Read checksum from stored file if exists, otherwise compute placeholder
        let checksum = "stored".to_string();

        Ok(StoredSchema {
            name: schema_name.to_string(),
            path: schema_dir.clone(),
            checksum,
            has_extensions: schema_dir.join("extensions").exists(),
            has_types: schema_dir.join("types").exists(),
            has_tables: schema_dir.join("tables").exists(),
            has_functions: schema_dir.join("functions").exists(),
            has_seeders: schema_dir.join("seeders").exists(),
            has_migrations: schema_dir.join("migrations").exists(),
        })
    }

    /// List schemas for a platform
    pub fn list_schemas(&self, platform: &str) -> Result<Vec<String>> {
        let platform_dir = self.data_dir.join(platform);

        if !platform_dir.exists() {
            return Ok(Vec::new());
        }

        let mut schemas = Vec::new();

        for entry in fs::read_dir(&platform_dir).map_err(|e| GatewayError::Internal(
            format!("Failed to read platform directory: {}", e)
        ))? {
            let entry = entry.map_err(|e| GatewayError::Internal(
                format!("Failed to read directory entry: {}", e)
            ))?;

            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // Skip platform.json and other non-schema directories
                    if name != "platform.json" && has_schema_structure(&path) {
                        schemas.push(name.to_string());
                    }
                }
            }
        }

        schemas.sort();
        Ok(schemas)
    }

    /// Get schema component directories
    pub fn extensions_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.schema_dir(platform, schema_name).join("extensions")
    }

    pub fn types_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.schema_dir(platform, schema_name).join("types")
    }

    pub fn tables_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.schema_dir(platform, schema_name).join("tables")
    }

    pub fn functions_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.schema_dir(platform, schema_name).join("functions")
    }

    pub fn seeders_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.schema_dir(platform, schema_name).join("seeders")
    }

    pub fn migrations_dir(&self, platform: &str, schema_name: &str) -> PathBuf {
        self.schema_dir(platform, schema_name).join("migrations")
    }
}

/// Check if a directory has schema structure (at least tables or functions)
fn has_schema_structure(path: &Path) -> bool {
    path.join("tables").exists() || path.join("functions").exists()
}

/// Compute SHA256 checksum of data
fn compute_checksum(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Check if a string is a valid identifier
fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tar::Builder;
    use tempfile::TempDir;

    fn create_test_archive() -> Vec<u8> {
        let mut archive_data = Vec::new();
        {
            let encoder = GzEncoder::new(&mut archive_data, Compression::default());
            let mut builder = Builder::new(encoder);

            // Add tables directory with a file
            let table_content = b"CREATE TABLE users (id SERIAL PRIMARY KEY);";
            let mut header = tar::Header::new_gnu();
            header.set_path("tables/users.pssql").unwrap();
            header.set_size(table_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &table_content[..]).unwrap();

            // Add functions directory with a file
            let func_content = b"CREATE FUNCTION test() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql;";
            let mut header = tar::Header::new_gnu();
            header.set_path("functions/test.pssql").unwrap();
            header.set_size(func_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &func_content[..]).unwrap();

            builder.into_inner().unwrap().finish().unwrap();
        }
        archive_data
    }

    #[test]
    fn test_store_schema() {
        let temp_dir = TempDir::new().unwrap();
        let store = SchemaStore::new(temp_dir.path());

        // Create platform directory first
        fs::create_dir_all(temp_dir.path().join("testapp")).unwrap();

        let archive = create_test_archive();
        let schema = store.store_schema("testapp", "tenant_db", &archive).unwrap();

        assert_eq!(schema.name, "tenant_db");
        assert!(schema.has_tables);
        assert!(schema.has_functions);
        assert!(!schema.has_migrations);
    }

    #[test]
    fn test_list_schemas() {
        let temp_dir = TempDir::new().unwrap();
        let store = SchemaStore::new(temp_dir.path());

        // Create platform directory
        fs::create_dir_all(temp_dir.path().join("testapp")).unwrap();

        let archive = create_test_archive();
        store.store_schema("testapp", "main_db", &archive).unwrap();
        store.store_schema("testapp", "tenant_db", &archive).unwrap();

        let schemas = store.list_schemas("testapp").unwrap();
        assert_eq!(schemas, vec!["main_db", "tenant_db"]);
    }
}
