//! Table deployer for declarative schema
//!
//! Deploys tables from the `tables/` folder during registration.
//! This is the declarative approach: tables are created from their definition files.
//!
//! The `migrations/` folder is NOT used during registration - that's only for migrate.
//!
//! Process:
//! 1. Read all `tables/*.pssql` files
//! 2. Parse CREATE TABLE statements using DependencyAnalyzer
//! 3. Build dependency graph from FOREIGN KEY references
//! 4. Execute CREATE TABLE in topological order
//! 5. Track deployed tables in `_stonescriptdb_gateway_tables`

use crate::error::{GatewayError, Result};
use crate::schema::dependency::DependencyAnalyzer;
use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Represents a table definition from a .pssql file
#[derive(Debug, Clone)]
pub struct TableDefinition {
    pub name: String,
    pub file_path: PathBuf,
    pub sql: String,
    pub checksum: String,
    pub depends_on: Vec<String>,
}

/// Result of table deployment
#[derive(Debug, Clone)]
pub struct TableDeployResult {
    pub tables_created: usize,
    pub tables_skipped: usize,
    pub creation_order: Vec<String>,
}

pub struct TableDeployer;

impl TableDeployer {
    pub fn new() -> Self {
        Self
    }

    /// Ensure the tracking table exists
    pub async fn ensure_tracking_table(&self, pool: &Pool, database: &str) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _stonescriptdb_gateway_tables (
                    id SERIAL PRIMARY KEY,
                    table_name TEXT NOT NULL UNIQUE,
                    checksum TEXT NOT NULL,
                    source_file TEXT NOT NULL,
                    deployed_at TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "_stonescriptdb_gateway_tables table creation".to_string(),
                cause: e.to_string(),
            })?;

        Ok(())
    }

    /// Find all table definition files in the tables directory
    pub fn find_table_files(&self, tables_dir: &Path) -> Result<Vec<PathBuf>> {
        if !tables_dir.exists() {
            debug!(
                "Tables directory {:?} does not exist, returning empty list",
                tables_dir
            );
            return Ok(Vec::new());
        }

        let mut files = Vec::new();

        for entry in fs::read_dir(tables_dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read tables directory: {}", e),
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

    /// Parse a table definition from a file
    pub fn parse_table_definition(&self, file_path: &Path) -> Result<Option<TableDefinition>> {
        let content = fs::read_to_string(file_path).map_err(|e| {
            GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read table file {:?}: {}", file_path, e),
            }
        })?;

        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Use DependencyAnalyzer to extract table info
        let analysis = match DependencyAnalyzer::analyze_sql(&content) {
            Ok(a) => a,
            Err(e) => {
                warn!("Failed to analyze table file {}: {}", file_name, e);
                return Ok(None);
            }
        };

        if analysis.tables.is_empty() {
            debug!("No CREATE TABLE found in {}", file_name);
            return Ok(None);
        }

        // Get the first table (normally one table per file)
        let table_info = &analysis.tables[0];

        let checksum = compute_checksum(&content);

        Ok(Some(TableDefinition {
            name: table_info.name.clone(),
            file_path: file_path.to_path_buf(),
            sql: content.trim().to_string(),
            checksum,
            depends_on: table_info.depends_on.clone(),
        }))
    }

    /// Order tables by dependencies (topological sort)
    pub fn order_by_dependencies(
        &self,
        tables: Vec<TableDefinition>,
    ) -> Result<Vec<TableDefinition>> {
        if tables.is_empty() {
            return Ok(tables);
        }

        // Build lookup map
        let table_map: HashMap<String, &TableDefinition> =
            tables.iter().map(|t| (t.name.clone(), t)).collect();

        // Build dependency graph (table index -> set of dependency indices)
        let table_names: Vec<&String> = tables.iter().map(|t| &t.name).collect();
        let name_to_idx: HashMap<&String, usize> =
            table_names.iter().enumerate().map(|(i, n)| (*n, i)).collect();

        let mut in_degree: Vec<usize> = vec![0; tables.len()];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); tables.len()];

        for (idx, table) in tables.iter().enumerate() {
            for dep_name in &table.depends_on {
                if let Some(&dep_idx) = name_to_idx.get(dep_name) {
                    if dep_idx != idx {
                        dependents[dep_idx].push(idx);
                        in_degree[idx] += 1;
                    }
                }
                // If dependency not found in our tables, it's external (ignore)
            }
        }

        // Kahn's algorithm for topological sort
        let mut queue: Vec<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &deg)| deg == 0)
            .map(|(i, _)| i)
            .collect();

        // Sort queue by table name for deterministic ordering
        queue.sort_by(|a, b| tables[*a].name.cmp(&tables[*b].name));

        let mut ordered_indices = Vec::new();

        while let Some(idx) = queue.pop() {
            ordered_indices.push(idx);

            for &dependent_idx in &dependents[idx] {
                in_degree[dependent_idx] -= 1;
                if in_degree[dependent_idx] == 0 {
                    queue.push(dependent_idx);
                    queue.sort_by(|a, b| tables[*a].name.cmp(&tables[*b].name));
                }
            }
        }

        if ordered_indices.len() != tables.len() {
            // Circular dependency detected
            let remaining: Vec<String> = tables
                .iter()
                .enumerate()
                .filter(|(i, _)| !ordered_indices.contains(i))
                .map(|(_, t)| t.name.clone())
                .collect();

            return Err(GatewayError::SchemaExtractionFailed {
                cause: format!(
                    "Circular dependency detected in table definitions: {}",
                    remaining.join(", ")
                ),
            });
        }

        // Reorder tables
        let ordered: Vec<TableDefinition> = ordered_indices
            .into_iter()
            .map(|i| tables[i].clone())
            .collect();

        info!("Table creation order (based on dependencies):");
        for (i, t) in ordered.iter().enumerate() {
            info!("  {}. {}", i + 1, t.name);
        }

        Ok(ordered)
    }

    /// Check if a table already exists in the database
    async fn table_exists(
        &self,
        client: &deadpool_postgres::Object,
        table_name: &str,
    ) -> Result<bool> {
        let row = client
            .query_opt(
                r#"
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public'
                AND table_name = $1
                "#,
                &[&table_name],
            )
            .await
            .unwrap_or(None);

        Ok(row.is_some())
    }

    /// Get deployed tables from tracking table
    async fn get_deployed_tables(
        &self,
        client: &deadpool_postgres::Object,
    ) -> Result<HashMap<String, String>> {
        let rows = client
            .query(
                "SELECT table_name, checksum FROM _stonescriptdb_gateway_tables",
                &[],
            )
            .await
            .unwrap_or_default();

        let mut tables = HashMap::new();
        for row in rows {
            let name: String = row.get(0);
            let checksum: String = row.get(1);
            tables.insert(name, checksum);
        }

        Ok(tables)
    }

    /// Update tracking table after creating a table
    async fn update_tracking(
        &self,
        client: &deadpool_postgres::Object,
        table: &TableDefinition,
    ) -> Result<()> {
        let file_name = table
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        client
            .execute(
                r#"
                INSERT INTO _stonescriptdb_gateway_tables (table_name, checksum, source_file, deployed_at)
                VALUES ($1, $2, $3, NOW())
                ON CONFLICT (table_name) DO UPDATE SET
                    checksum = EXCLUDED.checksum,
                    source_file = EXCLUDED.source_file,
                    deployed_at = NOW()
                "#,
                &[&table.name, &table.checksum, &file_name],
            )
            .await
            .ok();

        Ok(())
    }

    /// Deploy tables from the tables directory
    /// Returns the number of tables created
    pub async fn deploy_tables(
        &self,
        pool: &Pool,
        database: &str,
        tables_dir: &Path,
    ) -> Result<usize> {
        // Ensure tracking table exists
        self.ensure_tracking_table(pool, database).await?;

        let table_files = self.find_table_files(tables_dir)?;

        if table_files.is_empty() {
            debug!("No table files found in {:?}", tables_dir);
            return Ok(0);
        }

        debug!(
            "Found {} table files in {:?}",
            table_files.len(),
            tables_dir
        );

        // Parse all table definitions
        let mut tables = Vec::new();
        for file_path in &table_files {
            if let Some(table_def) = self.parse_table_definition(file_path)? {
                tables.push(table_def);
            }
        }

        if tables.is_empty() {
            debug!("No valid table definitions found");
            return Ok(0);
        }

        // Order by dependencies
        let ordered_tables = self.order_by_dependencies(tables)?;

        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        // Get already deployed tables
        let deployed = self.get_deployed_tables(&client).await?;

        let mut created = 0;
        let mut skipped = 0;

        for table in &ordered_tables {
            // Check if table already exists
            if self.table_exists(&client, &table.name).await? {
                // Check if it's tracked with same checksum
                if let Some(existing_checksum) = deployed.get(&table.name) {
                    if existing_checksum == &table.checksum {
                        debug!("Table {} unchanged (checksum match), skipping", table.name);
                        skipped += 1;
                        continue;
                    } else {
                        // Table exists but different definition - this is a migration scenario
                        warn!(
                            "Table {} already exists with different definition. Use migrate endpoint for schema changes.",
                            table.name
                        );
                        // Update tracking with new checksum
                        self.update_tracking(&client, table).await?;
                        skipped += 1;
                        continue;
                    }
                } else {
                    // Table exists but not tracked - add to tracking
                    debug!(
                        "Table {} already exists in database, adding to tracking",
                        table.name
                    );
                    self.update_tracking(&client, table).await?;
                    skipped += 1;
                    continue;
                }
            }

            // Create the table
            debug!("Creating table {} in {}", table.name, database);

            match client.batch_execute(&table.sql).await {
                Ok(_) => {
                    info!("Created table {} in database {}", table.name, database);
                    self.update_tracking(&client, table).await?;
                    created += 1;
                }
                Err(e) => {
                    return Err(GatewayError::MigrationFailed {
                        database: database.to_string(),
                        migration: format!("table:{}", table.name),
                        cause: e.to_string(),
                    });
                }
            }
        }

        info!(
            "Table deployment complete for {}: {} created, {} skipped",
            database, created, skipped
        );

        Ok(created)
    }

    /// List tables in database (public schema)
    pub async fn list_tables(&self, pool: &Pool, database: &str) -> Result<Vec<String>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let rows = client
            .query(
                r#"
                SELECT table_name
                FROM information_schema.tables
                WHERE table_schema = 'public'
                AND table_type = 'BASE TABLE'
                AND table_name NOT LIKE '_stonescriptdb_gateway_%'
                ORDER BY table_name
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: database.to_string(),
                function: "list_tables".to_string(),
                cause: e.to_string(),
            })?;

        let tables: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        Ok(tables)
    }
}

impl Default for TableDeployer {
    fn default() -> Self {
        Self::new()
    }
}

fn compute_checksum(content: &str) -> String {
    // Normalize: remove comments, collapse whitespace, lowercase
    let single_line_re = regex::Regex::new(r"--[^\n]*").unwrap();
    let content = single_line_re.replace_all(content, "");

    let multi_line_re = regex::Regex::new(r"/\*[\s\S]*?\*/").unwrap();
    let content = multi_line_re.replace_all(&content, "");

    let whitespace_re = regex::Regex::new(r"\s+").unwrap();
    let normalized = whitespace_re.replace_all(&content, " ").trim().to_lowercase();

    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_table_files() {
        let deployer = TableDeployer::new();
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        fs::write(
            temp_dir.path().join("users.pssql"),
            "CREATE TABLE users (id SERIAL PRIMARY KEY);",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("posts.sql"),
            "CREATE TABLE posts (id SERIAL PRIMARY KEY);",
        )
        .unwrap();
        fs::write(temp_dir.path().join("readme.md"), "docs").unwrap(); // Should be ignored

        let files = deployer.find_table_files(temp_dir.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_parse_table_definition() {
        let deployer = TableDeployer::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("users.pssql");
        let content = r#"
-- Users table
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);
"#;
        fs::write(&file_path, content).unwrap();

        let table_def = deployer.parse_table_definition(&file_path).unwrap().unwrap();
        assert_eq!(table_def.name, "users");
        assert!(table_def.depends_on.is_empty());
    }

    #[test]
    fn test_parse_table_with_foreign_key() {
        let deployer = TableDeployer::new();
        let temp_dir = TempDir::new().unwrap();

        let file_path = temp_dir.path().join("posts.pssql");
        let content = r#"
CREATE TABLE posts (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id),
    title TEXT NOT NULL
);
"#;
        fs::write(&file_path, content).unwrap();

        let table_def = deployer.parse_table_definition(&file_path).unwrap().unwrap();
        assert_eq!(table_def.name, "posts");
        assert!(table_def.depends_on.contains(&"users".to_string()));
    }

    #[test]
    fn test_order_by_dependencies() {
        let deployer = TableDeployer::new();

        let tables = vec![
            TableDefinition {
                name: "posts".to_string(),
                file_path: PathBuf::from("posts.pssql"),
                sql: "CREATE TABLE posts...".to_string(),
                checksum: "abc".to_string(),
                depends_on: vec!["users".to_string()],
            },
            TableDefinition {
                name: "users".to_string(),
                file_path: PathBuf::from("users.pssql"),
                sql: "CREATE TABLE users...".to_string(),
                checksum: "def".to_string(),
                depends_on: vec![],
            },
            TableDefinition {
                name: "comments".to_string(),
                file_path: PathBuf::from("comments.pssql"),
                sql: "CREATE TABLE comments...".to_string(),
                checksum: "ghi".to_string(),
                depends_on: vec!["users".to_string(), "posts".to_string()],
            },
        ];

        let ordered = deployer.order_by_dependencies(tables).unwrap();

        // users should come before posts, and both before comments
        let user_idx = ordered.iter().position(|t| t.name == "users").unwrap();
        let post_idx = ordered.iter().position(|t| t.name == "posts").unwrap();
        let comment_idx = ordered.iter().position(|t| t.name == "comments").unwrap();

        assert!(user_idx < post_idx);
        assert!(user_idx < comment_idx);
        assert!(post_idx < comment_idx);
    }

    #[test]
    fn test_circular_dependency_detection() {
        let deployer = TableDeployer::new();

        let tables = vec![
            TableDefinition {
                name: "a".to_string(),
                file_path: PathBuf::from("a.pssql"),
                sql: "CREATE TABLE a...".to_string(),
                checksum: "abc".to_string(),
                depends_on: vec!["b".to_string()],
            },
            TableDefinition {
                name: "b".to_string(),
                file_path: PathBuf::from("b.pssql"),
                sql: "CREATE TABLE b...".to_string(),
                checksum: "def".to_string(),
                depends_on: vec!["a".to_string()],
            },
        ];

        let result = deployer.order_by_dependencies(tables);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Circular dependency"));
    }

    #[test]
    fn test_checksum_normalization() {
        let sql1 = "CREATE TABLE users (id INT);";
        let sql2 = "CREATE   TABLE   users   (id   INT);";
        let sql3 = "create table users (id int);";

        assert_eq!(compute_checksum(sql1), compute_checksum(sql2));
        assert_eq!(compute_checksum(sql1), compute_checksum(sql3));
    }
}
