//! PostgreSQL custom type manager
//!
//! Handles creation and management of custom PostgreSQL types:
//! - ENUM types (CREATE TYPE ... AS ENUM)
//! - Composite types (CREATE TYPE ... AS (...))
//! - Domain types (CREATE DOMAIN ... AS ...)
//!
//! Types are defined in the `types/` folder with one file per type.
//! Types are installed AFTER extensions but BEFORE migrations,
//! so migrations can use custom types.

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Represents a custom PostgreSQL type
#[derive(Debug, Clone)]
pub struct CustomType {
    pub name: String,
    pub type_kind: TypeKind,
    pub sql: String,
    pub checksum: String,
}

/// The kind of PostgreSQL type
#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    Enum,
    Composite,
    Domain,
    Unknown,
}

impl std::fmt::Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Enum => write!(f, "ENUM"),
            TypeKind::Composite => write!(f, "COMPOSITE"),
            TypeKind::Domain => write!(f, "DOMAIN"),
            TypeKind::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

/// Tracks deployed custom types
#[derive(Debug)]
pub struct DeployedType {
    pub name: String,
    pub checksum: String,
}

pub struct CustomTypeManager;

impl CustomTypeManager {
    pub fn new() -> Self {
        Self
    }

    /// Find type definition files in the types directory
    pub fn find_type_files(&self, types_dir: &Path) -> Result<Vec<PathBuf>> {
        if !types_dir.exists() {
            debug!(
                "Types directory {:?} does not exist, returning empty list",
                types_dir
            );
            return Ok(Vec::new());
        }

        let mut files = Vec::new();

        for entry in fs::read_dir(types_dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read types directory: {}", e),
        })? {
            let entry = entry.map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "pssql" || ext == "pgsql" || ext == "sql" {
                        files.push(path);
                    }
                }
            }
        }

        // Sort for consistent ordering
        files.sort();

        Ok(files)
    }

    /// Parse a type definition from file content
    pub fn parse_type(&self, file_path: &Path) -> Result<CustomType> {
        let content = fs::read_to_string(file_path).map_err(|e| {
            GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read type file {:?}: {}", file_path, e),
            }
        })?;

        // Remove comments for parsing
        let sql = self.remove_comments(&content);
        let sql_upper = sql.to_uppercase();

        // Detect type kind
        let type_kind = if sql_upper.contains("AS ENUM") {
            TypeKind::Enum
        } else if sql_upper.contains("CREATE DOMAIN") {
            TypeKind::Domain
        } else if sql_upper.contains("CREATE TYPE") && sql_upper.contains(" AS (") {
            TypeKind::Composite
        } else if sql_upper.contains("CREATE TYPE") {
            TypeKind::Unknown
        } else {
            TypeKind::Unknown
        };

        // Extract type name
        let name = self.extract_type_name(&sql, &type_kind)?;

        // Compute checksum of normalized content
        let checksum = self.compute_checksum(&sql);

        Ok(CustomType {
            name,
            type_kind,
            sql: content.trim().to_string(),
            checksum,
        })
    }

    /// Extract the type name from SQL
    fn extract_type_name(&self, sql: &str, type_kind: &TypeKind) -> Result<String> {
        let pattern = match type_kind {
            TypeKind::Domain => {
                // CREATE DOMAIN type_name AS ...
                r"(?i)CREATE\s+DOMAIN\s+([a-zA-Z_][a-zA-Z0-9_]*)"
            }
            _ => {
                // CREATE TYPE type_name AS ...
                r"(?i)CREATE\s+TYPE\s+([a-zA-Z_][a-zA-Z0-9_]*)"
            }
        };

        let re = Regex::new(pattern).unwrap();
        if let Some(caps) = re.captures(sql) {
            Ok(caps.get(1).unwrap().as_str().to_lowercase())
        } else {
            Err(GatewayError::SchemaExtractionFailed {
                cause: "Could not extract type name from SQL".to_string(),
            })
        }
    }

    /// Remove SQL comments
    fn remove_comments(&self, sql: &str) -> String {
        // Remove single-line comments
        let single_line_re = Regex::new(r"--[^\n]*").unwrap();
        let sql = single_line_re.replace_all(sql, "");

        // Remove multi-line comments
        let multi_line_re = Regex::new(r"/\*[\s\S]*?\*/").unwrap();
        multi_line_re.replace_all(&sql, "").to_string()
    }

    /// Normalize SQL for checksum comparison
    fn normalize_for_checksum(&self, sql: &str) -> String {
        let whitespace_re = Regex::new(r"\s+").unwrap();
        whitespace_re.replace_all(sql, " ").trim().to_lowercase()
    }

    /// Compute checksum of type definition
    fn compute_checksum(&self, sql: &str) -> String {
        let normalized = self.normalize_for_checksum(sql);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Ensure the tracking table exists
    async fn ensure_tracking_table(&self, client: &deadpool_postgres::Object) -> Result<()> {
        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _stonescriptdb_gateway_types (
                    id SERIAL PRIMARY KEY,
                    type_name TEXT NOT NULL UNIQUE,
                    type_kind TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    source_file TEXT,
                    deployed_at TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: "unknown".to_string(),
                migration: "_stonescriptdb_gateway_types".to_string(),
                cause: e.to_string(),
            })?;

        Ok(())
    }

    /// Get deployed types from tracking table
    async fn get_deployed_types(
        &self,
        client: &deadpool_postgres::Object,
    ) -> Result<HashMap<String, DeployedType>> {
        let rows = client
            .query(
                "SELECT type_name, checksum FROM _stonescriptdb_gateway_types",
                &[],
            )
            .await
            .unwrap_or_default();

        let mut types = HashMap::new();
        for row in rows {
            let name: String = row.get(0);
            let checksum: String = row.get(1);
            types.insert(
                name.clone(),
                DeployedType {
                    name,
                    checksum,
                },
            );
        }

        Ok(types)
    }

    /// Check if type exists in the database
    async fn type_exists(
        &self,
        client: &deadpool_postgres::Object,
        type_name: &str,
    ) -> Result<bool> {
        let row = client
            .query_opt(
                r#"
                SELECT 1 FROM pg_type t
                JOIN pg_namespace n ON t.typnamespace = n.oid
                WHERE t.typname = $1
                AND n.nspname = 'public'
                "#,
                &[&type_name],
            )
            .await
            .unwrap_or(None);

        Ok(row.is_some())
    }

    /// Deploy custom types to database
    /// Returns the number of types deployed
    pub async fn deploy_types(
        &self,
        pool: &Pool,
        database: &str,
        types_dir: &Path,
    ) -> Result<usize> {
        let type_files = self.find_type_files(types_dir)?;

        if type_files.is_empty() {
            debug!("No custom types to deploy for database {}", database);
            return Ok(0);
        }

        debug!(
            "Found {} type files in {:?}",
            type_files.len(),
            types_dir
        );

        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        // Ensure tracking table exists
        self.ensure_tracking_table(&client).await?;

        // Get already deployed types
        let deployed_types = self.get_deployed_types(&client).await?;

        let mut created = 0;
        let mut updated = 0;
        let mut skipped = 0;

        for file_path in &type_files {
            let custom_type = self.parse_type(file_path)?;
            let file_name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            // Check if already deployed with same checksum
            if let Some(deployed) = deployed_types.get(&custom_type.name) {
                if deployed.checksum == custom_type.checksum {
                    debug!(
                        "Type {} unchanged (checksum match), skipping",
                        custom_type.name
                    );
                    skipped += 1;
                    continue;
                }

                // Type changed - need to handle carefully
                // For ENUMs, we can add values but not remove/rename
                // For now, we'll warn and skip if type exists
                if self.type_exists(&client, &custom_type.name).await? {
                    warn!(
                        "Type {} already exists with different definition. Manual migration required.",
                        custom_type.name
                    );
                    // Update tracking table with new checksum anyway
                    self.update_tracking(&client, &custom_type, file_name).await?;
                    updated += 1;
                    continue;
                }
            }

            // Check if type exists in database (but not in tracking)
            if self.type_exists(&client, &custom_type.name).await? {
                debug!(
                    "Type {} already exists in database, adding to tracking",
                    custom_type.name
                );
                self.update_tracking(&client, &custom_type, file_name).await?;
                skipped += 1;
                continue;
            }

            // Create the type
            debug!(
                "Creating {} type {} in {}",
                custom_type.type_kind, custom_type.name, database
            );

            match client.execute(&custom_type.sql, &[]).await {
                Ok(_) => {
                    info!(
                        "Created {} type {} in database {}",
                        custom_type.type_kind, custom_type.name, database
                    );
                    self.update_tracking(&client, &custom_type, file_name).await?;
                    created += 1;
                }
                Err(e) => {
                    return Err(GatewayError::MigrationFailed {
                        database: database.to_string(),
                        migration: format!("type:{}", custom_type.name),
                        cause: e.to_string(),
                    });
                }
            }
        }

        info!(
            "Type deployment complete for {}: {} created, {} updated, {} skipped",
            database, created, updated, skipped
        );

        Ok(created + updated)
    }

    /// Update tracking table
    async fn update_tracking(
        &self,
        client: &deadpool_postgres::Object,
        custom_type: &CustomType,
        source_file: &str,
    ) -> Result<()> {
        client
            .execute(
                r#"
                INSERT INTO _stonescriptdb_gateway_types (type_name, type_kind, checksum, source_file, deployed_at)
                VALUES ($1, $2, $3, $4, NOW())
                ON CONFLICT (type_name) DO UPDATE SET
                    type_kind = EXCLUDED.type_kind,
                    checksum = EXCLUDED.checksum,
                    source_file = EXCLUDED.source_file,
                    deployed_at = NOW()
                "#,
                &[
                    &custom_type.name,
                    &custom_type.type_kind.to_string(),
                    &custom_type.checksum,
                    &source_file,
                ],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: "unknown".to_string(),
                migration: format!("tracking:{}", custom_type.name),
                cause: e.to_string(),
            })?;

        Ok(())
    }

    /// List custom types in database
    pub async fn list_types(&self, pool: &Pool, database: &str) -> Result<Vec<String>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let rows = client
            .query(
                r#"
                SELECT t.typname
                FROM pg_type t
                JOIN pg_namespace n ON t.typnamespace = n.oid
                WHERE n.nspname = 'public'
                AND t.typtype IN ('e', 'c', 'd')  -- enum, composite, domain
                ORDER BY t.typname
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: database.to_string(),
                function: "list_types".to_string(),
                cause: e.to_string(),
            })?;

        let types: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        Ok(types)
    }
}

impl Default for CustomTypeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_enum_type() {
        let manager = CustomTypeManager::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("order_status.pssql");
        let content = r#"
-- Order status enum
CREATE TYPE order_status AS ENUM (
    'pending',
    'processing',
    'shipped',
    'delivered',
    'cancelled'
);
"#;
        fs::write(&file_path, content).unwrap();

        let custom_type = manager.parse_type(&file_path).unwrap();
        assert_eq!(custom_type.name, "order_status");
        assert_eq!(custom_type.type_kind, TypeKind::Enum);
    }

    #[test]
    fn test_parse_composite_type() {
        let manager = CustomTypeManager::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("address.pssql");
        let content = r#"
-- Address composite type
CREATE TYPE address AS (
    street TEXT,
    city TEXT,
    state TEXT,
    zip_code TEXT,
    country TEXT
);
"#;
        fs::write(&file_path, content).unwrap();

        let custom_type = manager.parse_type(&file_path).unwrap();
        assert_eq!(custom_type.name, "address");
        assert_eq!(custom_type.type_kind, TypeKind::Composite);
    }

    #[test]
    fn test_parse_domain_type() {
        let manager = CustomTypeManager::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("email.pssql");
        let content = r#"
-- Email domain with validation
CREATE DOMAIN email AS TEXT
CHECK (VALUE ~ '^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$');
"#;
        fs::write(&file_path, content).unwrap();

        let custom_type = manager.parse_type(&file_path).unwrap();
        assert_eq!(custom_type.name, "email");
        assert_eq!(custom_type.type_kind, TypeKind::Domain);
    }

    #[test]
    fn test_find_type_files() {
        let manager = CustomTypeManager::new();
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        fs::write(temp_dir.path().join("order_status.pssql"), "CREATE TYPE order_status AS ENUM ('pending');").unwrap();
        fs::write(temp_dir.path().join("address.sql"), "CREATE TYPE address AS (city TEXT);").unwrap();
        fs::write(temp_dir.path().join("readme.md"), "docs").unwrap(); // Should be ignored

        let files = manager.find_type_files(temp_dir.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_checksum_normalization() {
        let manager = CustomTypeManager::new();

        let sql1 = "CREATE TYPE status AS ENUM ('a', 'b');";
        let sql2 = "CREATE   TYPE   status   AS   ENUM   ('a',   'b');";
        let sql3 = "create type status as enum ('a', 'b');";

        assert_eq!(
            manager.compute_checksum(sql1),
            manager.compute_checksum(sql2)
        );
        assert_eq!(
            manager.compute_checksum(sql1),
            manager.compute_checksum(sql3)
        );
    }
}
