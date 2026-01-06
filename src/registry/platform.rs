//! Platform Registry
//!
//! Manages platform registrations and metadata.

use crate::error::{GatewayError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Platform metadata stored in platform.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub name: String,
    pub registered_at: DateTime<Utc>,
    pub schemas: Vec<String>,
    pub databases: HashMap<String, DatabaseRecord>,
    /// PostgreSQL username for this platform (for database isolation)
    #[serde(default)]
    pub db_user: Option<String>,
    /// PostgreSQL password for this platform (stored encrypted in production)
    #[serde(default)]
    pub db_password: Option<String>,
}

/// Record of a created database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRecord {
    pub schema_name: String,
    pub database_name: String,
    pub created_at: DateTime<Utc>,
}

impl PlatformInfo {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            registered_at: Utc::now(),
            schemas: Vec::new(),
            databases: HashMap::new(),
            db_user: None,
            db_password: None,
        }
    }

    pub fn with_credentials(name: &str, db_user: String, db_password: String) -> Self {
        Self {
            name: name.to_string(),
            registered_at: Utc::now(),
            schemas: Vec::new(),
            databases: HashMap::new(),
            db_user: Some(db_user),
            db_password: Some(db_password),
        }
    }
}

/// Platform registry for managing platform registrations
pub struct PlatformRegistry {
    data_dir: PathBuf,
}

impl PlatformRegistry {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Get the directory for a platform
    pub fn platform_dir(&self, platform: &str) -> PathBuf {
        self.data_dir.join(platform)
    }

    /// Get the platform.json path
    fn platform_json_path(&self, platform: &str) -> PathBuf {
        self.platform_dir(platform).join("platform.json")
    }

    /// Check if a platform is registered
    pub fn is_registered(&self, platform: &str) -> bool {
        self.platform_json_path(platform).exists()
    }

    /// Register a new platform
    pub fn register_platform(&self, platform: &str) -> Result<PlatformInfo> {
        // Validate platform name
        if !is_valid_identifier(platform) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Invalid platform name: {}. Must be alphanumeric with underscores.", platform),
            });
        }

        // Check if already registered
        if self.is_registered(platform) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Platform '{}' is already registered", platform),
            });
        }

        // Create platform directory
        let platform_dir = self.platform_dir(platform);
        fs::create_dir_all(&platform_dir).map_err(|e| GatewayError::Internal(
            format!("Failed to create platform directory: {}", e)
        ))?;

        // Create platform info
        let info = PlatformInfo::new(platform);

        // Save platform.json
        self.save_platform_info(&info)?;

        info!("Registered platform: {}", platform);
        Ok(info)
    }

    /// Get platform info
    pub fn get_platform_info(&self, platform: &str) -> Result<PlatformInfo> {
        let path = self.platform_json_path(platform);

        if !path.exists() {
            return Err(GatewayError::InvalidRequest {
                message: format!("Platform '{}' is not registered", platform),
            });
        }

        let content = fs::read_to_string(&path).map_err(|e| GatewayError::Internal(
            format!("Failed to read platform.json: {}", e)
        ))?;

        let info: PlatformInfo = serde_json::from_str(&content).map_err(|e| GatewayError::Internal(
            format!("Failed to parse platform.json: {}", e)
        ))?;

        Ok(info)
    }

    /// Save platform info
    pub fn save_platform_info(&self, info: &PlatformInfo) -> Result<()> {
        let path = self.platform_json_path(&info.name);

        let content = serde_json::to_string_pretty(info).map_err(|e| GatewayError::Internal(
            format!("Failed to serialize platform info: {}", e)
        ))?;

        fs::write(&path, content).map_err(|e| GatewayError::Internal(
            format!("Failed to write platform.json: {}", e)
        ))?;

        Ok(())
    }

    /// Add a schema to platform
    pub fn add_schema(&self, platform: &str, schema_name: &str) -> Result<()> {
        let mut info = self.get_platform_info(platform)?;

        if !info.schemas.contains(&schema_name.to_string()) {
            info.schemas.push(schema_name.to_string());
            self.save_platform_info(&info)?;
        }

        Ok(())
    }

    /// Record a database creation
    pub fn record_database(&self, platform: &str, schema_name: &str, database_name: &str) -> Result<()> {
        let mut info = self.get_platform_info(platform)?;

        info.databases.insert(database_name.to_string(), DatabaseRecord {
            schema_name: schema_name.to_string(),
            database_name: database_name.to_string(),
            created_at: Utc::now(),
        });

        self.save_platform_info(&info)?;
        Ok(())
    }

    /// List all registered platforms
    pub fn list_platforms(&self) -> Result<Vec<String>> {
        if !self.data_dir.exists() {
            return Ok(Vec::new());
        }

        let mut platforms = Vec::new();

        for entry in fs::read_dir(&self.data_dir).map_err(|e| GatewayError::Internal(
            format!("Failed to read data directory: {}", e)
        ))? {
            let entry = entry.map_err(|e| GatewayError::Internal(
                format!("Failed to read directory entry: {}", e)
            ))?;

            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // Check if it has platform.json
                    if path.join("platform.json").exists() {
                        platforms.push(name.to_string());
                    }
                }
            }
        }

        platforms.sort();
        Ok(platforms)
    }

    /// List databases for a platform, optionally filtered by schema
    pub fn list_databases(&self, platform: &str, schema_filter: Option<&str>) -> Result<Vec<DatabaseRecord>> {
        let info = self.get_platform_info(platform)?;

        let mut databases: Vec<DatabaseRecord> = info.databases.values()
            .filter(|db| {
                schema_filter.map(|s| db.schema_name == s).unwrap_or(true)
            })
            .cloned()
            .collect();

        databases.sort_by(|a, b| a.database_name.cmp(&b.database_name));
        Ok(databases)
    }
}

/// Check if a string is a valid identifier (alphanumeric + underscore)
fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_register_platform() {
        let temp_dir = TempDir::new().unwrap();
        let registry = PlatformRegistry::new(temp_dir.path());

        let info = registry.register_platform("testapp").unwrap();
        assert_eq!(info.name, "testapp");
        assert!(info.schemas.is_empty());
        assert_eq!(info.db_user, None);
        assert_eq!(info.db_password, None);

        // Should fail on duplicate
        assert!(registry.register_platform("testapp").is_err());
    }

    #[test]
    fn test_invalid_platform_name() {
        let temp_dir = TempDir::new().unwrap();
        let registry = PlatformRegistry::new(temp_dir.path());

        assert!(registry.register_platform("test-app").is_err());
        assert!(registry.register_platform("test app").is_err());
        assert!(registry.register_platform("").is_err());
    }

    #[test]
    fn test_list_platforms() {
        let temp_dir = TempDir::new().unwrap();
        let registry = PlatformRegistry::new(temp_dir.path());

        registry.register_platform("app_a").unwrap();
        registry.register_platform("app_b").unwrap();

        let platforms = registry.list_platforms().unwrap();
        assert_eq!(platforms, vec!["app_a", "app_b"]);
    }
}
