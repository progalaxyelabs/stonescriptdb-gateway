//! Schema Diff Checker
//!
//! Compares desired schema (from tables/ folder) against current database schema.
//! Validates type changes using the compatibility matrix before allowing migration.
//!
//! Flow:
//! 1. Parse desired schema from tables/*.pssql files
//! 2. Query current schema from information_schema
//! 3. Compare and classify changes as SAFE or DATALOSS
//! 4. Block migration if DATALOSS detected (unless force=true)

use crate::error::{GatewayError, Result};
use crate::schema::dependency::DependencyAnalyzer;
use crate::schema::types::{TypeChecker, TypeCompatibility};
use deadpool_postgres::Pool;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

/// Represents a column in the schema
#[derive(Debug, Clone, Serialize)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
    pub character_maximum_length: Option<i32>,
    pub numeric_precision: Option<i32>,
    pub numeric_scale: Option<i32>,
}

impl ColumnSchema {
    /// Get the full type string (e.g., "VARCHAR(100)", "NUMERIC(10,2)")
    pub fn full_type(&self) -> String {
        let base = self.data_type.to_uppercase();

        if let Some(len) = self.character_maximum_length {
            return format!("{}({})", base, len);
        }

        if let (Some(prec), Some(scale)) = (self.numeric_precision, self.numeric_scale) {
            if base == "NUMERIC" || base == "DECIMAL" {
                return format!("{}({},{})", base, prec, scale);
            }
        }

        base
    }
}

/// Represents a table in the schema
#[derive(Debug, Clone, Serialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: HashMap<String, ColumnSchema>,
}

/// A single schema change
#[derive(Debug, Clone, Serialize)]
pub struct SchemaChange {
    pub table: String,
    pub change_type: ChangeType,
    pub column: Option<String>,
    pub from_type: Option<String>,
    pub to_type: Option<String>,
    pub compatibility: ChangeCompatibility,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ChangeType {
    CreateTable,
    DropTable,
    AddColumn,
    DropColumn,
    ModifyColumnType,
    ModifyColumnNullable,
    ModifyColumnDefault,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ChangeCompatibility {
    Safe,
    DataLoss,
    Incompatible,
}

/// Result of schema diff
#[derive(Debug, Clone, Serialize)]
pub struct SchemaDiff {
    pub safe_changes: Vec<SchemaChange>,
    pub dataloss_changes: Vec<SchemaChange>,
    pub incompatible_changes: Vec<SchemaChange>,
}

impl SchemaDiff {
    pub fn new() -> Self {
        Self {
            safe_changes: Vec::new(),
            dataloss_changes: Vec::new(),
            incompatible_changes: Vec::new(),
        }
    }

    pub fn is_safe(&self) -> bool {
        self.dataloss_changes.is_empty() && self.incompatible_changes.is_empty()
    }

    pub fn has_changes(&self) -> bool {
        !self.safe_changes.is_empty()
            || !self.dataloss_changes.is_empty()
            || !self.incompatible_changes.is_empty()
    }

    pub fn add_change(&mut self, change: SchemaChange) {
        match change.compatibility {
            ChangeCompatibility::Safe => self.safe_changes.push(change),
            ChangeCompatibility::DataLoss => self.dataloss_changes.push(change),
            ChangeCompatibility::Incompatible => self.incompatible_changes.push(change),
        }
    }
}

impl Default for SchemaDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Schema diff checker
pub struct SchemaDiffChecker {
    type_checker: TypeChecker,
}

impl SchemaDiffChecker {
    pub fn new() -> Self {
        Self {
            type_checker: TypeChecker::new(),
        }
    }

    /// Parse desired schema from tables directory
    pub fn parse_desired_schema(&self, tables_dir: &Path) -> Result<HashMap<String, TableSchema>> {
        let mut tables = HashMap::new();

        if !tables_dir.exists() {
            debug!("Tables directory {:?} does not exist", tables_dir);
            return Ok(tables);
        }

        // Read all SQL files
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
                        let content = fs::read_to_string(&path).map_err(|e| {
                            GatewayError::SchemaExtractionFailed {
                                cause: format!("Failed to read file {:?}: {}", path, e),
                            }
                        })?;

                        // Parse tables from this file
                        if let Ok(analysis) = DependencyAnalyzer::analyze_sql(&content) {
                            for table_info in analysis.tables {
                                let mut columns = HashMap::new();

                                for col in table_info.columns {
                                    columns.insert(
                                        col.name.clone(),
                                        ColumnSchema {
                                            name: col.name,
                                            data_type: col.data_type,
                                            is_nullable: col.is_nullable,
                                            column_default: if col.has_default {
                                                Some("(has default)".to_string())
                                            } else {
                                                None
                                            },
                                            character_maximum_length: None, // Would need enhanced parsing
                                            numeric_precision: None,
                                            numeric_scale: None,
                                        },
                                    );
                                }

                                tables.insert(
                                    table_info.name.clone(),
                                    TableSchema {
                                        name: table_info.name,
                                        columns,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(tables)
    }

    /// Query current schema from database
    pub async fn query_current_schema(
        &self,
        pool: &Pool,
        database: &str,
    ) -> Result<HashMap<String, TableSchema>> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let mut tables: HashMap<String, TableSchema> = HashMap::new();

        // Query all tables and columns from information_schema
        let rows = client
            .query(
                r#"
                SELECT
                    t.table_name,
                    c.column_name,
                    c.data_type,
                    c.is_nullable,
                    c.column_default,
                    c.character_maximum_length,
                    c.numeric_precision,
                    c.numeric_scale
                FROM information_schema.tables t
                JOIN information_schema.columns c
                    ON t.table_name = c.table_name
                    AND t.table_schema = c.table_schema
                WHERE t.table_schema = 'public'
                    AND t.table_type = 'BASE TABLE'
                    AND t.table_name NOT LIKE '_stonescriptdb_gateway_%'
                ORDER BY t.table_name, c.ordinal_position
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: database.to_string(),
                function: "schema query".to_string(),
                cause: e.to_string(),
            })?;

        for row in rows {
            let table_name: String = row.get(0);
            let column_name: String = row.get(1);
            let data_type: String = row.get(2);
            let is_nullable_str: String = row.get(3);
            let column_default: Option<String> = row.get(4);
            let char_max_len: Option<i32> = row.get(5);
            let numeric_precision: Option<i32> = row.get(6);
            let numeric_scale: Option<i32> = row.get(7);

            let is_nullable = is_nullable_str.to_uppercase() == "YES";

            let column = ColumnSchema {
                name: column_name.clone(),
                data_type: data_type.to_uppercase(),
                is_nullable,
                column_default,
                character_maximum_length: char_max_len,
                numeric_precision,
                numeric_scale,
            };

            tables
                .entry(table_name.clone())
                .or_insert_with(|| TableSchema {
                    name: table_name,
                    columns: HashMap::new(),
                })
                .columns
                .insert(column_name, column);
        }

        Ok(tables)
    }

    /// Compare desired schema against current schema
    pub fn diff_schemas(
        &self,
        desired: &HashMap<String, TableSchema>,
        current: &HashMap<String, TableSchema>,
    ) -> SchemaDiff {
        let mut diff = SchemaDiff::new();

        // Check for new tables and modified tables
        for (table_name, desired_table) in desired {
            match current.get(table_name) {
                None => {
                    // New table - always safe
                    diff.add_change(SchemaChange {
                        table: table_name.clone(),
                        change_type: ChangeType::CreateTable,
                        column: None,
                        from_type: None,
                        to_type: None,
                        compatibility: ChangeCompatibility::Safe,
                        reason: None,
                    });
                }
                Some(current_table) => {
                    // Compare columns
                    self.diff_table_columns(&mut diff, table_name, desired_table, current_table);
                }
            }
        }

        // Check for dropped tables
        for table_name in current.keys() {
            if !desired.contains_key(table_name) {
                diff.add_change(SchemaChange {
                    table: table_name.clone(),
                    change_type: ChangeType::DropTable,
                    column: None,
                    from_type: None,
                    to_type: None,
                    compatibility: ChangeCompatibility::DataLoss,
                    reason: Some("Dropping table will delete all data".to_string()),
                });
            }
        }

        diff
    }

    /// Compare columns between desired and current table
    fn diff_table_columns(
        &self,
        diff: &mut SchemaDiff,
        table_name: &str,
        desired: &TableSchema,
        current: &TableSchema,
    ) {
        // Check for new and modified columns
        for (col_name, desired_col) in &desired.columns {
            match current.columns.get(col_name) {
                None => {
                    // New column
                    let compatibility = if !desired_col.is_nullable
                        && desired_col.column_default.is_none()
                    {
                        // NOT NULL without DEFAULT on existing table with data - needs special handling
                        ChangeCompatibility::DataLoss
                    } else {
                        ChangeCompatibility::Safe
                    };

                    diff.add_change(SchemaChange {
                        table: table_name.to_string(),
                        change_type: ChangeType::AddColumn,
                        column: Some(col_name.clone()),
                        from_type: None,
                        to_type: Some(desired_col.full_type()),
                        compatibility,
                        reason: if !desired_col.is_nullable && desired_col.column_default.is_none()
                        {
                            Some(
                                "Adding NOT NULL column without DEFAULT requires data migration"
                                    .to_string(),
                            )
                        } else {
                            None
                        },
                    });
                }
                Some(current_col) => {
                    // Check type change
                    self.diff_column_type(diff, table_name, col_name, desired_col, current_col);

                    // Check nullable change
                    if desired_col.is_nullable != current_col.is_nullable {
                        let compatibility = if !desired_col.is_nullable {
                            // Making NOT NULL - might fail if NULLs exist
                            ChangeCompatibility::DataLoss
                        } else {
                            // Making nullable - always safe
                            ChangeCompatibility::Safe
                        };

                        diff.add_change(SchemaChange {
                            table: table_name.to_string(),
                            change_type: ChangeType::ModifyColumnNullable,
                            column: Some(col_name.clone()),
                            from_type: Some(if current_col.is_nullable {
                                "NULLABLE"
                            } else {
                                "NOT NULL"
                            }
                            .to_string()),
                            to_type: Some(if desired_col.is_nullable {
                                "NULLABLE"
                            } else {
                                "NOT NULL"
                            }
                            .to_string()),
                            compatibility,
                            reason: if !desired_col.is_nullable {
                                Some("May fail if NULL values exist".to_string())
                            } else {
                                None
                            },
                        });
                    }
                }
            }
        }

        // Check for dropped columns
        for col_name in current.columns.keys() {
            if !desired.columns.contains_key(col_name) {
                diff.add_change(SchemaChange {
                    table: table_name.to_string(),
                    change_type: ChangeType::DropColumn,
                    column: Some(col_name.clone()),
                    from_type: Some(current.columns[col_name].full_type()),
                    to_type: None,
                    compatibility: ChangeCompatibility::DataLoss,
                    reason: Some("Dropping column will delete all data in that column".to_string()),
                });
            }
        }
    }

    /// Compare column types and check compatibility
    fn diff_column_type(
        &self,
        diff: &mut SchemaDiff,
        table_name: &str,
        col_name: &str,
        desired: &ColumnSchema,
        current: &ColumnSchema,
    ) {
        let desired_type = desired.full_type();
        let current_type = current.full_type();

        // Use type checker to validate the change
        let compat = self.type_checker.check_compatibility(&current_type, &desired_type);

        match compat {
            TypeCompatibility::Identical => {
                // No change needed
            }
            TypeCompatibility::Safe => {
                diff.add_change(SchemaChange {
                    table: table_name.to_string(),
                    change_type: ChangeType::ModifyColumnType,
                    column: Some(col_name.to_string()),
                    from_type: Some(current_type),
                    to_type: Some(desired_type),
                    compatibility: ChangeCompatibility::Safe,
                    reason: None,
                });
            }
            TypeCompatibility::DataLoss { reason } => {
                diff.add_change(SchemaChange {
                    table: table_name.to_string(),
                    change_type: ChangeType::ModifyColumnType,
                    column: Some(col_name.to_string()),
                    from_type: Some(current_type),
                    to_type: Some(desired_type),
                    compatibility: ChangeCompatibility::DataLoss,
                    reason: Some(reason),
                });
            }
            TypeCompatibility::Incompatible { reason } => {
                diff.add_change(SchemaChange {
                    table: table_name.to_string(),
                    change_type: ChangeType::ModifyColumnType,
                    column: Some(col_name.to_string()),
                    from_type: Some(current_type),
                    to_type: Some(desired_type),
                    compatibility: ChangeCompatibility::Incompatible,
                    reason: Some(reason),
                });
            }
        }
    }

    /// Validate schema changes before migration
    /// Returns Ok if safe, Err if dataloss/incompatible changes detected
    pub async fn validate_migration(
        &self,
        pool: &Pool,
        database: &str,
        tables_dir: &Path,
        force: bool,
    ) -> Result<SchemaDiff> {
        // Parse desired schema
        let desired = self.parse_desired_schema(tables_dir)?;

        if desired.is_empty() {
            debug!("No tables found in {:?}, skipping schema validation", tables_dir);
            return Ok(SchemaDiff::new());
        }

        // Query current schema
        let current = self.query_current_schema(pool, database).await?;

        // Compute diff
        let diff = self.diff_schemas(&desired, &current);

        // Log changes
        if !diff.safe_changes.is_empty() {
            info!(
                "Schema diff for {}: {} safe changes",
                database,
                diff.safe_changes.len()
            );
        }

        if !diff.dataloss_changes.is_empty() {
            warn!(
                "Schema diff for {}: {} DATALOSS changes detected",
                database,
                diff.dataloss_changes.len()
            );
            for change in &diff.dataloss_changes {
                warn!(
                    "  - {:?} on {}.{}: {} -> {} ({})",
                    change.change_type,
                    change.table,
                    change.column.as_deref().unwrap_or("*"),
                    change.from_type.as_deref().unwrap_or("-"),
                    change.to_type.as_deref().unwrap_or("-"),
                    change.reason.as_deref().unwrap_or("potential data loss")
                );
            }
        }

        if !diff.incompatible_changes.is_empty() {
            warn!(
                "Schema diff for {}: {} INCOMPATIBLE changes detected",
                database,
                diff.incompatible_changes.len()
            );
        }

        // Check if we should block
        if !diff.is_safe() && !force {
            let mut reasons = Vec::new();

            for change in &diff.dataloss_changes {
                reasons.push(format!(
                    "{:?} {}.{}: {}",
                    change.change_type,
                    change.table,
                    change.column.as_deref().unwrap_or("*"),
                    change.reason.as_deref().unwrap_or("potential data loss")
                ));
            }

            for change in &diff.incompatible_changes {
                reasons.push(format!(
                    "{:?} {}.{}: {}",
                    change.change_type,
                    change.table,
                    change.column.as_deref().unwrap_or("*"),
                    change.reason.as_deref().unwrap_or("incompatible types")
                ));
            }

            return Err(GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "schema validation".to_string(),
                cause: format!(
                    "Schema changes blocked due to potential data loss. {} issues found:\n  - {}\n\nUse force=true to proceed anyway.",
                    reasons.len(),
                    reasons.join("\n  - ")
                ),
            });
        }

        Ok(diff)
    }

    /// Format diff as readable string
    pub fn format_diff(diff: &SchemaDiff) -> String {
        let mut output = String::new();

        output.push_str("═══════════════════════════════════════════════════════════════\n");
        output.push_str("                      SCHEMA DIFF REPORT\n");
        output.push_str("═══════════════════════════════════════════════════════════════\n\n");

        if !diff.has_changes() {
            output.push_str("No schema changes detected.\n");
            return output;
        }

        if !diff.safe_changes.is_empty() {
            output.push_str(&format!("SAFE CHANGES ({}):\n", diff.safe_changes.len()));
            output.push_str("───────────────────────────────────────────────────────────────\n");
            for change in &diff.safe_changes {
                output.push_str(&Self::format_change(change, "✓"));
            }
            output.push('\n');
        }

        if !diff.dataloss_changes.is_empty() {
            output.push_str(&format!(
                "⚠️  DATALOSS CHANGES ({}):\n",
                diff.dataloss_changes.len()
            ));
            output.push_str("───────────────────────────────────────────────────────────────\n");
            for change in &diff.dataloss_changes {
                output.push_str(&Self::format_change(change, "⚠"));
            }
            output.push('\n');
        }

        if !diff.incompatible_changes.is_empty() {
            output.push_str(&format!(
                "❌ INCOMPATIBLE CHANGES ({}):\n",
                diff.incompatible_changes.len()
            ));
            output.push_str("───────────────────────────────────────────────────────────────\n");
            for change in &diff.incompatible_changes {
                output.push_str(&Self::format_change(change, "✗"));
            }
            output.push('\n');
        }

        output.push_str("═══════════════════════════════════════════════════════════════\n");

        if diff.is_safe() {
            output.push_str("Result: SAFE - Migration can proceed\n");
        } else {
            output.push_str("Result: BLOCKED - Use force=true to proceed\n");
        }

        output
    }

    fn format_change(change: &SchemaChange, prefix: &str) -> String {
        let mut line = format!("  {} {:?}", prefix, change.change_type);

        if let Some(col) = &change.column {
            line.push_str(&format!(" {}.{}", change.table, col));
        } else {
            line.push_str(&format!(" {}", change.table));
        }

        if let (Some(from), Some(to)) = (&change.from_type, &change.to_type) {
            line.push_str(&format!(": {} -> {}", from, to));
        } else if let Some(to) = &change.to_type {
            line.push_str(&format!(": {}", to));
        }

        if let Some(reason) = &change.reason {
            line.push_str(&format!("\n      Reason: {}", reason));
        }

        line.push('\n');
        line
    }
}

impl Default for SchemaDiffChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_full_type() {
        let col = ColumnSchema {
            name: "test".to_string(),
            data_type: "varchar".to_string(),
            is_nullable: true,
            column_default: None,
            character_maximum_length: Some(100),
            numeric_precision: None,
            numeric_scale: None,
        };
        assert_eq!(col.full_type(), "VARCHAR(100)");

        let col2 = ColumnSchema {
            name: "amount".to_string(),
            data_type: "numeric".to_string(),
            is_nullable: false,
            column_default: None,
            character_maximum_length: None,
            numeric_precision: Some(10),
            numeric_scale: Some(2),
        };
        assert_eq!(col2.full_type(), "NUMERIC(10,2)");
    }

    #[test]
    fn test_diff_new_table() {
        let checker = SchemaDiffChecker::new();

        let mut desired = HashMap::new();
        desired.insert(
            "users".to_string(),
            TableSchema {
                name: "users".to_string(),
                columns: HashMap::new(),
            },
        );

        let current = HashMap::new();

        let diff = checker.diff_schemas(&desired, &current);

        assert!(diff.is_safe());
        assert_eq!(diff.safe_changes.len(), 1);
        assert_eq!(diff.safe_changes[0].change_type, ChangeType::CreateTable);
    }

    #[test]
    fn test_diff_drop_table() {
        let checker = SchemaDiffChecker::new();

        let desired = HashMap::new();

        let mut current = HashMap::new();
        current.insert(
            "users".to_string(),
            TableSchema {
                name: "users".to_string(),
                columns: HashMap::new(),
            },
        );

        let diff = checker.diff_schemas(&desired, &current);

        assert!(!diff.is_safe());
        assert_eq!(diff.dataloss_changes.len(), 1);
        assert_eq!(diff.dataloss_changes[0].change_type, ChangeType::DropTable);
    }

    #[test]
    fn test_diff_add_column() {
        let checker = SchemaDiffChecker::new();

        let mut desired_cols = HashMap::new();
        desired_cols.insert(
            "id".to_string(),
            ColumnSchema {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                is_nullable: false,
                column_default: Some("nextval".to_string()),
                character_maximum_length: None,
                numeric_precision: None,
                numeric_scale: None,
            },
        );
        desired_cols.insert(
            "email".to_string(),
            ColumnSchema {
                name: "email".to_string(),
                data_type: "VARCHAR".to_string(),
                is_nullable: true,
                column_default: None,
                character_maximum_length: Some(255),
                numeric_precision: None,
                numeric_scale: None,
            },
        );

        let mut current_cols = HashMap::new();
        current_cols.insert(
            "id".to_string(),
            ColumnSchema {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                is_nullable: false,
                column_default: Some("nextval".to_string()),
                character_maximum_length: None,
                numeric_precision: None,
                numeric_scale: None,
            },
        );

        let mut desired = HashMap::new();
        desired.insert(
            "users".to_string(),
            TableSchema {
                name: "users".to_string(),
                columns: desired_cols,
            },
        );

        let mut current = HashMap::new();
        current.insert(
            "users".to_string(),
            TableSchema {
                name: "users".to_string(),
                columns: current_cols,
            },
        );

        let diff = checker.diff_schemas(&desired, &current);

        assert!(diff.is_safe());
        assert_eq!(diff.safe_changes.len(), 1);
        assert_eq!(diff.safe_changes[0].change_type, ChangeType::AddColumn);
        assert_eq!(diff.safe_changes[0].column, Some("email".to_string()));
    }
}
