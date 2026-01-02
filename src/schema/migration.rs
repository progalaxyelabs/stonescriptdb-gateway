use crate::error::{GatewayError, Result};
use crate::schema::DependencyAnalyzer;
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

/// Result of dependency validation
#[derive(Debug, Clone)]
pub struct DependencyValidation {
    pub is_valid: bool,
    pub issues: Vec<DependencyIssue>,
    pub suggested_order: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DependencyIssue {
    pub migration: String,
    pub table: String,
    pub depends_on: String,
    pub depends_on_defined_in: Option<String>,
    pub message: String,
}

pub struct MigrationRunner;

impl MigrationRunner {
    pub fn new() -> Self {
        Self
    }

    /// Validate that migrations are in correct dependency order
    pub fn validate_dependencies(&self, migrations_dir: &Path) -> Result<DependencyValidation> {
        let migration_files = self.find_migration_files(migrations_dir)?;

        if migration_files.is_empty() {
            return Ok(DependencyValidation {
                is_valid: true,
                issues: Vec::new(),
                suggested_order: Vec::new(),
            });
        }

        // Concatenate all SQL to analyze dependencies
        let mut all_sql = String::new();
        let mut table_to_migration: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        for migration in &migration_files {
            let content = fs::read_to_string(&migration.path).map_err(|e| {
                GatewayError::SchemaExtractionFailed {
                    cause: format!("Failed to read migration file {:?}: {}", migration.path, e),
                }
            })?;
            all_sql.push_str(&content);
            all_sql.push('\n');

            // Track which migration defines which tables
            if let Ok(analysis) = DependencyAnalyzer::analyze_sql(&content) {
                for table in &analysis.tables {
                    table_to_migration.insert(table.name.clone(), migration.name.clone());
                }
            }
        }

        // Analyze full schema
        let analysis = DependencyAnalyzer::analyze_sql(&all_sql)
            .map_err(|e| GatewayError::SchemaExtractionFailed { cause: e })?;

        let mut issues = Vec::new();

        // Check each migration's tables against their dependencies
        for (i, migration) in migration_files.iter().enumerate() {
            let content = fs::read_to_string(&migration.path).unwrap_or_default();
            if let Ok(migration_analysis) = DependencyAnalyzer::analyze_sql(&content) {
                for table in &migration_analysis.tables {
                    for dep in &table.depends_on {
                        // Find which migration defines the dependency
                        if let Some(dep_migration) = table_to_migration.get(dep) {
                            // Find index of dependency migration
                            let dep_index = migration_files.iter().position(|m| &m.name == dep_migration);

                            if let Some(dep_idx) = dep_index {
                                if dep_idx > i {
                                    // Dependency is defined AFTER this migration - problem!
                                    issues.push(DependencyIssue {
                                        migration: migration.name.clone(),
                                        table: table.name.clone(),
                                        depends_on: dep.clone(),
                                        depends_on_defined_in: Some(dep_migration.clone()),
                                        message: format!(
                                            "Table '{}' in '{}' references '{}' which is defined later in '{}'",
                                            table.name, migration.name, dep, dep_migration
                                        ),
                                    });
                                }
                            }
                        } else {
                            // Dependency table not found in any migration (might be external)
                            debug!(
                                "Table '{}' references '{}' which is not defined in migrations (may be external)",
                                table.name, dep
                            );
                        }
                    }
                }
            }
        }

        let is_valid = issues.is_empty();

        // Get suggested order from topological sort
        let suggested_order = analysis.creation_order.clone();

        if !is_valid {
            warn!(
                "Dependency validation found {} issues in migrations",
                issues.len()
            );
            for issue in &issues {
                warn!("  - {}", issue.message);
            }
        }

        Ok(DependencyValidation {
            is_valid,
            issues,
            suggested_order,
        })
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

        // Sort alphabetically by default (will be reordered by dependencies if needed)
        migrations.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(migrations)
    }

    /// Reorder migrations based on table dependencies
    /// Returns migrations in the order they should be executed
    pub fn order_by_dependencies(&self, migrations: Vec<MigrationFile>) -> Result<Vec<MigrationFile>> {
        if migrations.is_empty() {
            return Ok(migrations);
        }

        // Build a map of table -> migration that defines it
        let mut table_to_migration: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut migration_tables: Vec<Vec<String>> = Vec::new(); // Tables defined in each migration
        let mut migration_deps: Vec<std::collections::HashSet<String>> = Vec::new(); // Dependencies for each migration

        for (i, migration) in migrations.iter().enumerate() {
            let content = fs::read_to_string(&migration.path).unwrap_or_default();

            let mut tables = Vec::new();
            let mut deps = std::collections::HashSet::new();

            if let Ok(analysis) = DependencyAnalyzer::analyze_sql(&content) {
                for table in &analysis.tables {
                    table_to_migration.insert(table.name.clone(), i);
                    tables.push(table.name.clone());

                    for dep in &table.depends_on {
                        deps.insert(dep.clone());
                    }
                }
            }

            migration_tables.push(tables);
            migration_deps.push(deps);
        }

        // Build migration dependency graph (migration index -> migration indices it depends on)
        let mut migration_graph: Vec<std::collections::HashSet<usize>> = vec![std::collections::HashSet::new(); migrations.len()];

        for (i, deps) in migration_deps.iter().enumerate() {
            for dep_table in deps {
                if let Some(&dep_migration_idx) = table_to_migration.get(dep_table) {
                    if dep_migration_idx != i {
                        migration_graph[i].insert(dep_migration_idx);
                    }
                }
            }
        }

        // Topological sort of migrations
        let mut in_degree: Vec<usize> = vec![0; migrations.len()];
        for deps in &migration_graph {
            for &dep in deps {
                in_degree[dep] += 0; // Just to ensure we touch each
            }
        }
        for (i, deps) in migration_graph.iter().enumerate() {
            in_degree[i] = deps.len();
        }

        // Reverse the graph for Kahn's algorithm
        let mut reverse_graph: Vec<Vec<usize>> = vec![Vec::new(); migrations.len()];
        for (i, deps) in migration_graph.iter().enumerate() {
            for &dep in deps {
                reverse_graph[dep].push(i);
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &deg)| deg == 0)
            .map(|(i, _)| i)
            .collect();
        queue.sort_by(|a, b| migrations[*a].name.cmp(&migrations[*b].name)); // Stable sort by name

        let mut ordered_indices = Vec::new();

        while let Some(idx) = queue.pop() {
            ordered_indices.push(idx);

            for &dependent in &reverse_graph[idx] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    queue.push(dependent);
                    queue.sort_by(|a, b| migrations[*a].name.cmp(&migrations[*b].name));
                }
            }
        }

        if ordered_indices.len() != migrations.len() {
            // Circular dependency detected
            let remaining: Vec<String> = migrations
                .iter()
                .enumerate()
                .filter(|(i, _)| !ordered_indices.contains(i))
                .map(|(_, m)| m.name.clone())
                .collect();

            return Err(GatewayError::SchemaExtractionFailed {
                cause: format!(
                    "Circular dependency detected in migrations: {}",
                    remaining.join(", ")
                ),
            });
        }

        // Reorder migrations
        let ordered: Vec<MigrationFile> = ordered_indices
            .into_iter()
            .map(|i| migrations[i].clone())
            .collect();

        // Log the order
        info!("Migration execution order (based on dependencies):");
        for (i, m) in ordered.iter().enumerate() {
            info!("  {}. {}", i + 1, m.name);
        }

        Ok(ordered)
    }

    /// Run migrations with optional dependency validation
    /// If validate_deps is true and dependencies are invalid, returns an error
    pub async fn run_migrations_with_validation(
        &self,
        pool: &Pool,
        database: &str,
        migrations_dir: &Path,
        validate_deps: bool,
    ) -> Result<(usize, Option<DependencyValidation>)> {
        // Validate dependencies first if requested
        let validation = if validate_deps {
            let v = self.validate_dependencies(migrations_dir)?;
            if !v.is_valid {
                return Err(GatewayError::MigrationFailed {
                    database: database.to_string(),
                    migration: "dependency validation".to_string(),
                    cause: format!(
                        "Table dependency order is invalid. {} issues found. Suggested table creation order: {}",
                        v.issues.len(),
                        v.suggested_order.join(" → ")
                    ),
                });
            }
            Some(v)
        } else {
            None
        };

        let count = self.run_migrations(pool, database, migrations_dir).await?;
        Ok((count, validation))
    }

    pub async fn run_migrations(
        &self,
        pool: &Pool,
        database: &str,
        migrations_dir: &Path,
    ) -> Result<usize> {
        self.run_migrations_ordered(pool, database, migrations_dir, true).await
    }

    /// Run migrations with optional automatic dependency ordering
    pub async fn run_migrations_ordered(
        &self,
        pool: &Pool,
        database: &str,
        migrations_dir: &Path,
        auto_order: bool,
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

        // Order by dependencies if requested
        let migration_files = if auto_order && !migration_files.is_empty() {
            self.order_by_dependencies(migration_files)?
        } else {
            migration_files
        };

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
