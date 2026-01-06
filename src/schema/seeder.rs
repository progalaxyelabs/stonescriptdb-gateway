//! Seeder runner with validation
//!
//! Rules:
//! - register: Run seeders only if table is empty
//! - migrate: Never run seeders, only validate
//! - Validation: After migration, check all seeder records exist
//! - If validation fails: Rollback the entire transaction

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

/// Represents a parsed seeder file
#[derive(Debug, Clone)]
pub struct SeederFile {
    pub name: String,
    pub table_name: String,
    pub records: Vec<SeederRecord>,
    pub primary_key_columns: Vec<String>,
}

/// Represents a single record from a seeder
#[derive(Debug, Clone)]
pub struct SeederRecord {
    pub columns: Vec<String>,
    pub values: Vec<String>,
}

/// Result of seeder execution
#[derive(Debug, Clone)]
pub struct SeederResult {
    pub table: String,
    pub inserted: usize,
    pub skipped: usize,
    pub total_expected: usize,
}

/// Result of seeder validation
#[derive(Debug, Clone)]
pub struct SeederValidation {
    pub table: String,
    pub expected: usize,
    pub found: usize,
    pub missing: Vec<String>, // Primary key values of missing records
}

pub struct SeederRunner;

impl SeederRunner {
    pub fn new() -> Self {
        Self
    }

    /// Find all seeder files in directory
    pub fn find_seeder_files(&self, seeders_dir: &Path) -> Result<Vec<SeederFile>> {
        if !seeders_dir.exists() {
            debug!("Seeders directory {:?} does not exist", seeders_dir);
            return Ok(Vec::new());
        }

        let mut seeders = Vec::new();

        for entry in fs::read_dir(seeders_dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read seeders directory: {}", e),
        })? {
            let entry = entry.map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "pssql" || ext == "pgsql" || ext == "sql" {
                        let content = fs::read_to_string(&path).map_err(|e| {
                            GatewayError::SchemaExtractionFailed {
                                cause: format!("Failed to read seeder file {:?}: {}", path, e),
                            }
                        })?;

                        if let Some(seeder) = self.parse_seeder(&path, &content)? {
                            seeders.push(seeder);
                        }
                    }
                }
            }
        }

        // Sort by filename for deterministic order
        seeders.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(seeders)
    }

    /// Parse a seeder file to extract table name, columns, and values
    fn parse_seeder(&self, path: &Path, content: &str) -> Result<Option<SeederFile>> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // Remove comments
        let content = self.remove_comments(content);

        // Find INSERT INTO statement
        // Capture everything after VALUES but stop at ON CONFLICT, ON DUPLICATE KEY, or semicolon
        let insert_re = regex::Regex::new(
            r"(?is)INSERT\s+INTO\s+(\w+)\s*\(\s*([^)]+)\s*\)\s*VALUES\s+(.*?)(?:ON\s+(?:CONFLICT|DUPLICATE\s+KEY)|;|$)"
        ).unwrap();

        let caps = match insert_re.captures(&content) {
            Some(c) => c,
            None => {
                debug!("No INSERT statement found in seeder: {}", name);
                return Ok(None);
            }
        };

        let table_name = caps[1].to_lowercase();
        let columns: Vec<String> = caps[2]
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .collect();

        let values_str = &caps[3];

        // Parse individual value tuples
        let records = self.parse_values(values_str, &columns, &name, &table_name)?;

        // First column is assumed to be primary key (common convention)
        // TODO: Could be enhanced to detect actual PK from table definition
        let primary_key_columns = if !columns.is_empty() {
            vec![columns[0].clone()]
        } else {
            Vec::new()
        };

        Ok(Some(SeederFile {
            name,
            table_name,
            records,
            primary_key_columns,
        }))
    }

    /// Remove SQL comments
    fn remove_comments(&self, sql: &str) -> String {
        let single_line_re = regex::Regex::new(r"--[^\n]*").unwrap();
        let sql = single_line_re.replace_all(sql, "");

        let multi_line_re = regex::Regex::new(r"/\*[\s\S]*?\*/").unwrap();
        multi_line_re.replace_all(&sql, "").to_string()
    }

    /// Parse VALUES clause into individual records
    fn parse_values(&self, values_str: &str, columns: &[String], file_name: &str, table_name: &str) -> Result<Vec<SeederRecord>> {
        let mut records = Vec::new();

        // Match individual value tuples: (val1, val2, ...)
        let tuple_re = regex::Regex::new(r"\(([^)]+)\)").unwrap();

        for cap in tuple_re.captures_iter(values_str) {
            let values_inner = &cap[1];
            let values = self.parse_value_tuple(values_inner);

            if values.len() == columns.len() {
                records.push(SeederRecord {
                    columns: columns.to_vec(),
                    values,
                });
            } else {
                warn!(
                    "Seeder file '{}' for table '{}': Value count mismatch in tuple '{}': expected {} columns {:?}, got {} values {:?}",
                    file_name,
                    table_name,
                    values_inner,
                    columns.len(),
                    columns,
                    values.len(),
                    values
                );
            }
        }

        Ok(records)
    }

    /// Parse a single value tuple, handling quoted strings
    fn parse_value_tuple(&self, tuple_str: &str) -> Vec<String> {
        let mut values = Vec::new();
        let mut current = String::new();
        let mut in_string = false;
        let mut string_char = ' ';

        for ch in tuple_str.chars() {
            match ch {
                '\'' | '"' if !in_string => {
                    in_string = true;
                    string_char = ch;
                    current.push(ch);
                }
                c if c == string_char && in_string => {
                    in_string = false;
                    current.push(ch);
                }
                ',' if !in_string => {
                    values.push(current.trim().to_string());
                    current = String::new();
                }
                _ => {
                    current.push(ch);
                }
            }
        }

        if !current.trim().is_empty() {
            values.push(current.trim().to_string());
        }

        values
    }

    /// Run seeders on register (only if table is empty)
    pub async fn run_seeders_on_register(
        &self,
        pool: &Pool,
        database: &str,
        seeders_dir: &Path,
    ) -> Result<Vec<SeederResult>> {
        let seeders = self.find_seeder_files(seeders_dir)?;

        if seeders.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for seeder in seeders {
            let result = self
                .run_seeder_if_empty(pool, database, &seeder)
                .await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Run a single seeder only if the table is empty
    async fn run_seeder_if_empty(
        &self,
        pool: &Pool,
        database: &str,
        seeder: &SeederFile,
    ) -> Result<SeederResult> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        // Check if table is empty
        let count_sql = format!("SELECT COUNT(*) FROM {}", seeder.table_name);
        let row = client.query_one(&count_sql, &[]).await.map_err(|e| {
            GatewayError::QueryFailed {
                database: database.to_string(),
                function: format!("seeder check: {}", seeder.table_name),
                cause: e.to_string(),
            }
        })?;

        let count: i64 = row.get(0);

        if count > 0 {
            info!(
                "Skipping seeder for {} - table has {} existing rows",
                seeder.table_name, count
            );
            return Ok(SeederResult {
                table: seeder.table_name.clone(),
                inserted: 0,
                skipped: seeder.records.len(),
                total_expected: seeder.records.len(),
            });
        }

        // Table is empty, insert all records
        let mut inserted = 0;

        for record in &seeder.records {
            let columns_str = record.columns.join(", ");
            let values_str = record.values.join(", ");

            let insert_sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                seeder.table_name, columns_str, values_str
            );

            debug!("Executing seeder SQL for {}: {}", seeder.table_name, insert_sql);

            client.execute(&insert_sql, &[]).await.map_err(|e| {
                // Extract detailed error message from PostgreSQL error
                let error_detail = if let Some(db_err) = e.as_db_error() {
                    format!("{} - {}", db_err.message(),
                        db_err.detail().unwrap_or("no additional detail"))
                } else {
                    e.to_string()
                };

                warn!("Seeder insert failed for table {}: SQL = '{}', Error = {}",
                    seeder.table_name, insert_sql, error_detail);

                GatewayError::QueryFailed {
                    database: database.to_string(),
                    function: format!("seeder insert: {}", seeder.table_name),
                    cause: error_detail,
                }
            })?;

            inserted += 1;
        }

        info!(
            "Seeder {} inserted {} records into {}",
            seeder.name, inserted, seeder.table_name
        );

        Ok(SeederResult {
            table: seeder.table_name.clone(),
            inserted,
            skipped: 0,
            total_expected: seeder.records.len(),
        })
    }

    /// Validate seeders after migration (check all records exist)
    /// Returns Err if validation fails - caller should rollback
    pub async fn validate_seeders(
        &self,
        pool: &Pool,
        database: &str,
        seeders_dir: &Path,
    ) -> Result<Vec<SeederValidation>> {
        let seeders = self.find_seeder_files(seeders_dir)?;

        if seeders.is_empty() {
            return Ok(Vec::new());
        }

        let mut validations = Vec::new();
        let mut has_errors = false;

        for seeder in seeders {
            let validation = self.validate_seeder(pool, database, &seeder).await?;

            if validation.found < validation.expected {
                has_errors = true;
                warn!(
                    "Seeder validation failed for {}: expected {} records, found {}. Missing: {:?}",
                    validation.table, validation.expected, validation.found, validation.missing
                );
            }

            validations.push(validation);
        }

        if has_errors {
            let missing_details: Vec<String> = validations
                .iter()
                .filter(|v| v.found < v.expected)
                .map(|v| {
                    format!(
                        "{}: {}/{} (missing: {})",
                        v.table,
                        v.found,
                        v.expected,
                        v.missing.join(", ")
                    )
                })
                .collect();

            return Err(GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "seeder validation".to_string(),
                cause: format!(
                    "Seeder validation failed. Missing records in: {}. \
                    These records should have been inserted during /register but are missing. \
                    Possible causes: (1) Initial /register failed - check logs for INSERT errors (e.g., missing required columns, constraint violations), \
                    (2) Records were manually deleted. \
                    Solution: Fix the table schema (e.g., add SERIAL to auto-increment columns) and re-register, \
                    OR create a migration to manually INSERT the missing seeder data.",
                    missing_details.join("; ")
                ),
            });
        }

        Ok(validations)
    }

    /// Validate a single seeder - check all records exist in database
    async fn validate_seeder(
        &self,
        pool: &Pool,
        database: &str,
        seeder: &SeederFile,
    ) -> Result<SeederValidation> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let mut found = 0;
        let mut missing = Vec::new();

        for record in &seeder.records {
            // Build WHERE clause using primary key
            let pk_conditions: Vec<String> = seeder
                .primary_key_columns
                .iter()
                .filter_map(|pk_col| {
                    let idx = record.columns.iter().position(|c| c == pk_col)?;
                    Some(format!("{} = {}", pk_col, record.values[idx]))
                })
                .collect();

            if pk_conditions.is_empty() {
                // No PK defined, skip validation for this record
                found += 1;
                continue;
            }

            let check_sql = format!(
                "SELECT 1 FROM {} WHERE {} LIMIT 1",
                seeder.table_name,
                pk_conditions.join(" AND ")
            );

            let row = client.query_opt(&check_sql, &[]).await.map_err(|e| {
                GatewayError::QueryFailed {
                    database: database.to_string(),
                    function: format!("seeder validation: {}", seeder.table_name),
                    cause: e.to_string(),
                }
            })?;

            if row.is_some() {
                found += 1;
            } else {
                // Record PK value for error message
                let pk_value: String = seeder
                    .primary_key_columns
                    .iter()
                    .filter_map(|pk_col| {
                        let idx = record.columns.iter().position(|c| c == pk_col)?;
                        Some(record.values[idx].clone())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                missing.push(pk_value);
            }
        }

        Ok(SeederValidation {
            table: seeder.table_name.clone(),
            expected: seeder.records.len(),
            found,
            missing,
        })
    }
}

impl Default for SeederRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_value_tuple() {
        let runner = SeederRunner::new();

        let values = runner.parse_value_tuple("1, 'admin', 'Administrator'");
        assert_eq!(values, vec!["1", "'admin'", "'Administrator'"]);

        let values = runner.parse_value_tuple("'USD', 'US Dollar', '$'");
        assert_eq!(values, vec!["'USD'", "'US Dollar'", "'$'"]);
    }

    #[test]
    fn test_remove_comments() {
        let runner = SeederRunner::new();

        let sql = "-- This is a comment\nINSERT INTO test VALUES (1);";
        let cleaned = runner.remove_comments(sql);
        assert!(cleaned.contains("INSERT"));
        assert!(!cleaned.contains("comment"));
    }
}
