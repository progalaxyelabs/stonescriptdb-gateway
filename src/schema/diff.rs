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
use crate::schema::column_type_exemptions::{self, ColumnTypeExemption};
use crate::schema::dependency::DependencyAnalyzer;
use crate::schema::drop_column_exemptions::{self, DropColumnExemption};
use crate::schema::migration::MigrationRunner;
use crate::schema::rename_column_exemptions::{self, RenameColumnExemption};
use crate::schema::types::{TypeChecker, TypeCompatibility};
use deadpool_postgres::Pool;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

/// Bundle of schema-diff exemptions extracted from pending migrations.
///
/// Each variant tells the diff checker that a seemingly-unsafe change is actually
/// being handled by a gateway-wrapped developer helper:
/// - `type_changes`: `_stonescriptdb_gateway_change_column_type` — dev-written fn
///   handles type change, old-column drop, new-column rename.
/// - `renames`: `_stonescriptdb_gateway_rename_column` — dev-written fn runs
///   `ALTER TABLE ... RENAME COLUMN`. Diff collapses DropColumn(old)+AddColumn(new)
///   to a no-op.
/// - `drops`: `_stonescriptdb_gateway_drop_column` — dev-written fn runs the actual
///   DROP. Cascade, dependent views, FK constraints are ALL the developer's
///   responsibility — the gateway does not enumerate dependents or impose policy.
#[derive(Debug, Clone, Default)]
pub struct MigrationExemptions {
    pub type_changes: Vec<ColumnTypeExemption>,
    pub renames: Vec<RenameColumnExemption>,
    pub drops: Vec<DropColumnExemption>,
}

impl MigrationExemptions {
    pub fn is_empty(&self) -> bool {
        self.type_changes.is_empty() && self.renames.is_empty() && self.drops.is_empty()
    }

    fn type_change_matches(&self, table: &str, column: &str) -> bool {
        self.type_changes
            .iter()
            .any(|e| e.table == table && e.column == column)
    }

    /// Returns the matching rename exemption if `(table, column)` is the old-name side.
    fn find_rename_old(&self, table: &str, column: &str) -> Option<&RenameColumnExemption> {
        self.renames
            .iter()
            .find(|e| e.table == table && e.old_column == column)
    }

    /// Returns the matching rename exemption if `(table, column)` is the new-name side.
    fn find_rename_new(&self, table: &str, column: &str) -> Option<&RenameColumnExemption> {
        self.renames
            .iter()
            .find(|e| e.table == table && e.new_column == column)
    }

    /// Returns the matching intentional-drop exemption for `(table, column)`.
    fn find_intentional_drop(&self, table: &str, column: &str) -> Option<&DropColumnExemption> {
        self.drops
            .iter()
            .find(|e| e.table == table && e.column == column)
    }
}

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

/// Maps a guarded (potentially destructive) `ChangeType` to its allow-token and
/// the operator-facing CLI flag that unlocks it.
///
/// Returns `None` for change types that the diff never classifies as
/// DataLoss/Incompatible (e.g. `CreateTable` is always Safe; `ModifyColumnDefault`
/// is never emitted). Such changes are never blocked, so they need no token.
///
/// The five guarded operations are the only `ChangeType`s that can land in
/// `dataloss_changes` / `incompatible_changes` (see `SchemaDiffChecker::diff`).
pub fn guarded_op_token(change_type: &ChangeType) -> Option<(&'static str, &'static str)> {
    match change_type {
        ChangeType::DropTable => Some(("drop_table", "--allow-drop-table")),
        ChangeType::DropColumn => Some(("drop_column", "--allow-drop-column")),
        ChangeType::ModifyColumnType => {
            Some(("modify_column_type", "--allow-column-type-change"))
        }
        ChangeType::AddColumn => Some(("add_not_null_column", "--allow-add-not-null-column")),
        ChangeType::ModifyColumnNullable => Some(("set_not_null", "--allow-set-not-null")),
        ChangeType::CreateTable | ChangeType::ModifyColumnDefault => None,
    }
}

/// Operator-granted permissions for guarded (destructive) schema operations.
///
/// Two independent gates are controlled here:
///   1. The per-operation diff dataloss/incompatible gate → `allow` tokens.
///   2. The holistic post-migration schema-verification gate → `skip_verification`.
///
/// `force` is the back-compat "allow everything" escape hatch: it permits ALL
/// guarded operations AND bypasses verification (the legacy `force=true` behavior).
#[derive(Debug, Clone, Default)]
pub struct MigrationGuards {
    /// Allow-tokens for specific guarded diff operations (e.g. `"drop_column"`).
    pub allow: Vec<String>,
    /// Bypass the holistic post-migration verification gate only.
    pub skip_verification: bool,
    /// Legacy allow-all: permits every guarded op and skips verification.
    pub force: bool,
}

impl MigrationGuards {
    pub fn new(allow: Vec<String>, skip_verification: bool, force: bool) -> Self {
        Self { allow, skip_verification, force }
    }

    /// Allow-all back-compat constructor (equivalent to the old `force=true`).
    pub fn force_all() -> Self {
        Self { allow: Vec::new(), skip_verification: false, force: true }
    }

    /// Is this specific guarded operation permitted — by an explicit allow token
    /// or by the allow-all `force` escape hatch?
    pub fn allows_token(&self, token: &str) -> bool {
        self.force || self.allow.iter().any(|a| a == token)
    }

    /// Should the post-migration verification gate be bypassed?
    pub fn skip_verification(&self) -> bool {
        self.force || self.skip_verification
    }
}

/// Pure least-privilege gate decision: given a computed diff and the operator's
/// granted permissions, return the list of guarded operations that are NOT
/// permitted (each line names the exact `--allow-*` flag that unlocks it).
///
/// An empty result means the migration may proceed. This is extracted from
/// `validate_migration` so the gating logic is unit-testable without a DB pool.
pub fn evaluate_guarded_changes(diff: &SchemaDiff, guards: &MigrationGuards) -> Vec<String> {
    let mut blocked: Vec<String> = Vec::new();
    if diff.is_safe() {
        return blocked;
    }

    for change in diff
        .dataloss_changes
        .iter()
        .chain(diff.incompatible_changes.iter())
    {
        let token_and_flag = guarded_op_token(&change.change_type);
        let permitted = match token_and_flag {
            Some((token, _flag)) => guards.allows_token(token),
            // A guarded change with no mapped token can only be bypassed by the
            // allow-all `force` escape hatch — fail closed otherwise.
            None => guards.force,
        };

        if !permitted {
            let flag = token_and_flag.map(|(_, f)| f).unwrap_or("--force");
            blocked.push(format!(
                "{:?} {}.{}: {} [unlock with {}]",
                change.change_type,
                change.table,
                change.column.as_deref().unwrap_or("*"),
                change
                    .reason
                    .as_deref()
                    .unwrap_or("potential data loss / incompatible change"),
                flag,
            ));
        }
    }

    blocked
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
                    c.udt_name,
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
            let udt_name: String = row.get(3);
            let is_nullable_str: String = row.get(4);
            let column_default: Option<String> = row.get(5);
            let char_max_len: Option<i32> = row.get(6);
            let numeric_precision: Option<i32> = row.get(7);
            let numeric_scale: Option<i32> = row.get(8);

            let is_nullable = is_nullable_str.to_uppercase() == "YES";

            // Normalize data_type from information_schema:
            // - ARRAY with udt_name _text -> TEXT[]
            // - USER-DEFINED (enums, composites) -> use udt_name (e.g., stock_transaction_source)
            let normalized_data_type = if data_type.to_uppercase() == "ARRAY" {
                if let Some(element_type) = udt_name.strip_prefix('_') {
                    format!("{}[]", element_type.to_uppercase())
                } else {
                    data_type.to_uppercase()
                }
            } else if data_type.to_uppercase() == "USER-DEFINED" {
                udt_name.to_uppercase()
            } else {
                data_type.to_uppercase()
            };

            let column = ColumnSchema {
                name: column_name.clone(),
                data_type: normalized_data_type,
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

    /// Compare desired schema against current schema (no exemptions)
    pub fn diff_schemas(
        &self,
        desired: &HashMap<String, TableSchema>,
        current: &HashMap<String, TableSchema>,
    ) -> SchemaDiff {
        self.diff_schemas_with_exemptions(desired, current, &MigrationExemptions::default())
    }

    /// Compare desired schema against current schema, honoring migration exemptions.
    ///
    /// Exemptions are extracted from pending migration files that call the
    /// gateway-provided wrapper functions (`_stonescriptdb_gateway_change_column_type`,
    /// `_stonescriptdb_gateway_rename_column`, `_stonescriptdb_gateway_drop_column`).
    /// The diff checker treats the corresponding changes as Safe.
    pub fn diff_schemas_with_exemptions(
        &self,
        desired: &HashMap<String, TableSchema>,
        current: &HashMap<String, TableSchema>,
        exemptions: &MigrationExemptions,
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
                    self.diff_table_columns(&mut diff, table_name, desired_table, current_table, exemptions);
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
        exemptions: &MigrationExemptions,
    ) {
        // Check for new and modified columns
        for (col_name, desired_col) in &desired.columns {
            match current.columns.get(col_name) {
                None => {
                    // New column — but check if an exemption covers this column.
                    // When _stonescriptdb_gateway_change_column_type runs, the old column
                    // gets dropped and the new one renamed. The diff sees the desired column
                    // as "new" because the old column has a different type. If an exemption
                    // exists for this (table, column), the migration will handle it.
                    let exempt_reason: Option<String> = if exemptions.type_change_matches(table_name, col_name) {
                        Some("Column type change handled by _stonescriptdb_gateway_change_column_type in pending migration".to_string())
                    } else if let Some(r) = exemptions.find_rename_new(table_name, col_name) {
                        info!(
                            "Diff: AddColumn {}.{} marked Safe — rename from '{}' via helper '{}' (migration: {})",
                            table_name, col_name, r.old_column, r.helper_fn, r.migration_file
                        );
                        Some(format!(
                            "Column rename from '{}' handled by _stonescriptdb_gateway_rename_column (helper '{}', migration: {})",
                            r.old_column, r.helper_fn, r.migration_file
                        ))
                    } else {
                        None
                    };

                    if let Some(reason) = exempt_reason {
                        diff.add_change(SchemaChange {
                            table: table_name.to_string(),
                            change_type: ChangeType::AddColumn,
                            column: Some(col_name.clone()),
                            from_type: None,
                            to_type: Some(desired_col.full_type()),
                            compatibility: ChangeCompatibility::Safe,
                            reason: Some(reason),
                        });
                    } else {
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
                }
                Some(current_col) => {
                    // Check type change
                    self.diff_column_type(diff, table_name, col_name, desired_col, current_col, exemptions);

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

        // Check for dropped columns — but exclude columns that have an exemption.
        // When _stonescriptdb_gateway_change_column_type runs, the old column gets dropped
        // and replaced. The diff would see the old column as "dropped" if its type doesn't
        // match what's in the desired schema. If an exemption covers this column, skip it.
        for col_name in current.columns.keys() {
            if !desired.columns.contains_key(col_name) {
                let drop_exempt_reason: Option<String> = if exemptions.type_change_matches(table_name, col_name) {
                    Some("Column drop handled by _stonescriptdb_gateway_change_column_type in pending migration".to_string())
                } else if let Some(r) = exemptions.find_rename_old(table_name, col_name) {
                    info!(
                        "Diff: DropColumn {}.{} marked Safe — rename to '{}' via helper '{}' (migration: {})",
                        table_name, col_name, r.new_column, r.helper_fn, r.migration_file
                    );
                    Some(format!(
                        "Column rename to '{}' handled by _stonescriptdb_gateway_rename_column (helper '{}', migration: {})",
                        r.new_column, r.helper_fn, r.migration_file
                    ))
                } else if let Some(d) = exemptions.find_intentional_drop(table_name, col_name) {
                    info!(
                        "Diff: DropColumn {}.{} marked Safe — intentional drop via helper '{}' (migration: {}; cascade is developer's responsibility)",
                        table_name, col_name, d.helper_fn, d.migration_file
                    );
                    Some(format!(
                        "Intentional drop handled by _stonescriptdb_gateway_drop_column (helper '{}', migration: {}; cascade is the developer's responsibility)",
                        d.helper_fn, d.migration_file
                    ))
                } else {
                    None
                };

                if let Some(reason) = drop_exempt_reason {
                    diff.add_change(SchemaChange {
                        table: table_name.to_string(),
                        change_type: ChangeType::DropColumn,
                        column: Some(col_name.clone()),
                        from_type: Some(current.columns[col_name].full_type()),
                        to_type: None,
                        compatibility: ChangeCompatibility::Safe,
                        reason: Some(reason),
                    });
                } else {
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
    }

    /// Compare column types and check compatibility
    fn diff_column_type(
        &self,
        diff: &mut SchemaDiff,
        table_name: &str,
        col_name: &str,
        desired: &ColumnSchema,
        current: &ColumnSchema,
        exemptions: &MigrationExemptions,
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
            TypeCompatibility::DataLoss { ref reason } | TypeCompatibility::Incompatible { ref reason } => {
                // Check if this column type change is exempted by a pending migration
                let is_exempted = exemptions.type_change_matches(table_name, col_name);

                if is_exempted {
                    info!(
                        "Column type change {}.{} ({} -> {}) exempted by _stonescriptdb_gateway_change_column_type",
                        table_name, col_name, current_type, desired_type
                    );
                    diff.add_change(SchemaChange {
                        table: table_name.to_string(),
                        change_type: ChangeType::ModifyColumnType,
                        column: Some(col_name.to_string()),
                        from_type: Some(current_type),
                        to_type: Some(desired_type),
                        compatibility: ChangeCompatibility::Safe,
                        reason: Some(format!(
                            "Column type change handled by _stonescriptdb_gateway_change_column_type in pending migration (original: {})",
                            reason
                        )),
                    });
                } else {
                    let compatibility = if matches!(compat, TypeCompatibility::Incompatible { .. }) {
                        ChangeCompatibility::Incompatible
                    } else {
                        ChangeCompatibility::DataLoss
                    };
                    diff.add_change(SchemaChange {
                        table: table_name.to_string(),
                        change_type: ChangeType::ModifyColumnType,
                        column: Some(col_name.to_string()),
                        from_type: Some(current_type),
                        to_type: Some(desired_type),
                        compatibility,
                        reason: Some(reason.clone()),
                    });
                }
            }
        }
    }

    /// Validate schema changes before migration.
    ///
    /// Returns Ok if every dataloss/incompatible change is individually permitted
    /// by `guards` (or `force`), Err naming each un-permitted guarded operation and
    /// the exact `--allow-*` flag that would unlock it.
    pub async fn validate_migration(
        &self,
        pool: &Pool,
        database: &str,
        tables_dir: &Path,
        migrations_dir: &Path,
        guards: &MigrationGuards,
    ) -> Result<SchemaDiff> {
        // Parse desired schema
        let desired = self.parse_desired_schema(tables_dir)?;

        if desired.is_empty() {
            debug!("No tables found in {:?}, skipping schema validation", tables_dir);
            return Ok(SchemaDiff::new());
        }

        // Query current schema
        let current = self.query_current_schema(pool, database).await?;

        // Scan pending migrations for exemptions (type changes, renames, drops)
        let migration_runner = MigrationRunner::new();
        migration_runner.ensure_migrations_table(pool, database).await?;
        let applied = migration_runner.get_applied_migrations(pool, database).await?;
        let exemptions = MigrationExemptions {
            type_changes: column_type_exemptions::scan_migrations_for_exemptions(migrations_dir, &applied)?,
            renames: rename_column_exemptions::scan_migrations_for_exemptions(migrations_dir, &applied)?,
            drops: drop_column_exemptions::scan_migrations_for_exemptions(migrations_dir, &applied)?,
        };

        if !exemptions.is_empty() {
            info!(
                "Migration exemptions for {}: {} type-change, {} rename, {} drop",
                database,
                exemptions.type_changes.len(),
                exemptions.renames.len(),
                exemptions.drops.len()
            );
            for ex in &exemptions.type_changes {
                info!(
                    "  type-change: {}.{} -> {} (migration: {})",
                    ex.table, ex.column, ex.new_type, ex.migration_file
                );
            }
            for ex in &exemptions.renames {
                info!(
                    "  rename: {}.{} -> {} (migration: {})",
                    ex.table, ex.old_column, ex.new_column, ex.migration_file
                );
            }
            for ex in &exemptions.drops {
                info!(
                    "  drop: {}.{} (migration: {})",
                    ex.table, ex.column, ex.migration_file
                );
            }
        }

        // Compute diff with exemptions
        let diff = self.diff_schemas_with_exemptions(&desired, &current, &exemptions);

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
            for change in &diff.incompatible_changes {
                warn!(
                    "  - {:?} on {}.{}: {} -> {} ({})",
                    change.change_type,
                    change.table,
                    change.column.as_deref().unwrap_or("*"),
                    change.from_type.as_deref().unwrap_or("-"),
                    change.to_type.as_deref().unwrap_or("-"),
                    change.reason.as_deref().unwrap_or("incompatible types")
                );
            }
        }

        // Per-operation least-privilege gate. Pure decision logic lives in
        // `evaluate_guarded_changes` so it can be unit-tested without a DB pool.
        let blocked = evaluate_guarded_changes(&diff, guards);
        if !blocked.is_empty() {
            return Err(GatewayError::MigrationFailed {
                database: database.to_string(),
                migration: "schema validation".to_string(),
                cause: format!(
                    "Schema changes blocked: {} guarded operation(s) not permitted:\n  - {}\n\nGrant the specific --allow-* flag(s) shown above, or use --force to allow ALL guarded operations.",
                    blocked.len(),
                    blocked.join("\n  - ")
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

    fn col(name: &str, ty: &str) -> ColumnSchema {
        ColumnSchema {
            name: name.to_string(),
            data_type: ty.to_string(),
            is_nullable: true,
            column_default: None,
            character_maximum_length: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn table_with(cols: Vec<(&str, &str)>) -> TableSchema {
        let mut m = HashMap::new();
        for (n, t) in cols {
            m.insert(n.to_string(), col(n, t));
        }
        TableSchema {
            name: "t".to_string(),
            columns: m,
        }
    }

    #[test]
    fn test_rename_exemption_collapses_drop_and_add_to_safe() {
        let checker = SchemaDiffChecker::new();

        let mut desired = HashMap::new();
        desired.insert("t".to_string(), table_with(vec![("new_id", "INTEGER")]));

        let mut current = HashMap::new();
        current.insert("t".to_string(), table_with(vec![("old_id", "INTEGER")]));

        // Without exemption: drop + add would be flagged (drop is DataLoss).
        let diff_plain = checker.diff_schemas(&desired, &current);
        assert!(!diff_plain.is_safe(), "expected plain diff to flag drop");

        // With rename exemption, both sides should be Safe.
        let exemptions = MigrationExemptions {
            renames: vec![RenameColumnExemption {
                table: "t".to_string(),
                old_column: "old_id".to_string(),
                new_column: "new_id".to_string(),
                helper_fn: "_rename_helper".to_string(),
                migration_file: "001.pgsql".to_string(),
            }],
            ..Default::default()
        };
        let diff = checker.diff_schemas_with_exemptions(&desired, &current, &exemptions);
        assert!(diff.is_safe(), "rename exemption should make diff safe");
        assert_eq!(diff.safe_changes.len(), 2);
    }

    #[test]
    fn test_drop_exemption_marks_drop_safe() {
        let checker = SchemaDiffChecker::new();

        let mut desired = HashMap::new();
        desired.insert("t".to_string(), table_with(vec![("keep", "INTEGER")]));

        let mut current = HashMap::new();
        current.insert(
            "t".to_string(),
            table_with(vec![("keep", "INTEGER"), ("drop_me", "TEXT")]),
        );

        let diff_plain = checker.diff_schemas(&desired, &current);
        assert!(!diff_plain.is_safe());

        let exemptions = MigrationExemptions {
            drops: vec![DropColumnExemption {
                table: "t".to_string(),
                column: "drop_me".to_string(),
                helper_fn: "_drop_helper".to_string(),
                migration_file: "001.pgsql".to_string(),
            }],
            ..Default::default()
        };
        let diff = checker.diff_schemas_with_exemptions(&desired, &current, &exemptions);
        assert!(diff.is_safe(), "drop exemption should make diff safe");
        assert_eq!(diff.safe_changes.len(), 1);
        assert_eq!(diff.safe_changes[0].change_type, ChangeType::DropColumn);
    }

    #[test]
    fn test_unrelated_drop_still_blocked_when_only_one_column_exempted() {
        let checker = SchemaDiffChecker::new();

        let mut desired = HashMap::new();
        desired.insert("t".to_string(), table_with(vec![("keep", "INTEGER")]));

        let mut current = HashMap::new();
        current.insert(
            "t".to_string(),
            table_with(vec![
                ("keep", "INTEGER"),
                ("drop_me", "TEXT"),
                ("also_drop", "TEXT"),
            ]),
        );

        let exemptions = MigrationExemptions {
            drops: vec![DropColumnExemption {
                table: "t".to_string(),
                column: "drop_me".to_string(),
                helper_fn: "_drop_helper".to_string(),
                migration_file: "001.pgsql".to_string(),
            }],
            ..Default::default()
        };
        let diff = checker.diff_schemas_with_exemptions(&desired, &current, &exemptions);
        assert!(!diff.is_safe(), "unlisted drop must still be flagged");
        assert_eq!(diff.dataloss_changes.len(), 1);
        assert_eq!(diff.dataloss_changes[0].column, Some("also_drop".to_string()));
    }

    // ─── Granular per-operation --allow-* gate (#2810) ──────────────────────

    /// Build a SchemaDiff containing a single guarded (DataLoss) change of the
    /// given type — exercises the gate directly, independent of type classification.
    fn guarded_diff(change_type: ChangeType) -> SchemaDiff {
        let mut d = SchemaDiff::new();
        d.add_change(SchemaChange {
            table: "t".to_string(),
            change_type,
            column: Some("c".to_string()),
            from_type: None,
            to_type: None,
            compatibility: ChangeCompatibility::DataLoss,
            reason: Some("test".to_string()),
        });
        d
    }

    fn allow_only(tokens: &[&str]) -> MigrationGuards {
        MigrationGuards::new(tokens.iter().map(|s| s.to_string()).collect(), false, false)
    }

    /// The five guarded ops paired with their allow-token.
    fn guarded_cases() -> Vec<(ChangeType, &'static str)> {
        vec![
            (ChangeType::DropTable, "drop_table"),
            (ChangeType::DropColumn, "drop_column"),
            (ChangeType::ModifyColumnType, "modify_column_type"),
            (ChangeType::AddColumn, "add_not_null_column"),
            (ChangeType::ModifyColumnNullable, "set_not_null"),
        ]
    }

    #[test]
    fn test_each_op_blocked_without_its_allow() {
        for (ct, _token) in guarded_cases() {
            let diff = guarded_diff(ct.clone());
            let blocked = evaluate_guarded_changes(&diff, &MigrationGuards::default());
            assert_eq!(blocked.len(), 1, "{:?} must be blocked with no allow", ct);
        }
    }

    #[test]
    fn test_each_op_passes_with_its_allow() {
        for (ct, token) in guarded_cases() {
            let diff = guarded_diff(ct.clone());
            let blocked = evaluate_guarded_changes(&diff, &allow_only(&[token]));
            assert!(blocked.is_empty(), "{:?} must pass with --allow {}", ct, token);
        }
    }

    #[test]
    fn test_allow_does_not_unlock_a_different_op() {
        // --allow-drop-column must NOT let a DropTable through (least-privilege).
        let diff = guarded_diff(ChangeType::DropTable);
        let blocked = evaluate_guarded_changes(&diff, &allow_only(&["drop_column"]));
        assert_eq!(blocked.len(), 1, "drop_column allow must not unlock DropTable");
        assert!(
            blocked[0].contains("--allow-drop-table"),
            "error must name the correct flag, got: {}",
            blocked[0]
        );
    }

    #[test]
    fn test_error_message_names_exact_flag_per_op() {
        let expected = [
            (ChangeType::DropTable, "--allow-drop-table"),
            (ChangeType::DropColumn, "--allow-drop-column"),
            (ChangeType::ModifyColumnType, "--allow-column-type-change"),
            (ChangeType::AddColumn, "--allow-add-not-null-column"),
            (ChangeType::ModifyColumnNullable, "--allow-set-not-null"),
        ];
        for (ct, flag) in expected {
            let diff = guarded_diff(ct.clone());
            let blocked = evaluate_guarded_changes(&diff, &MigrationGuards::default());
            assert!(
                blocked[0].contains(flag),
                "{:?} error must mention {}, got: {}",
                ct,
                flag,
                blocked[0]
            );
        }
    }

    #[test]
    fn test_force_bypasses_all_guarded_ops() {
        for (ct, _token) in guarded_cases() {
            let diff = guarded_diff(ct.clone());
            let blocked = evaluate_guarded_changes(&diff, &MigrationGuards::force_all());
            assert!(blocked.is_empty(), "--force must bypass {:?}", ct);
        }
    }

    #[test]
    fn test_skip_verification_does_not_unlock_diff_gate() {
        // --dangerously-skip-verification governs gate #2 only; it must NOT grant
        // any diff-gate allow.
        let guards = MigrationGuards::new(vec![], true, false);
        let diff = guarded_diff(ChangeType::DropColumn);
        let blocked = evaluate_guarded_changes(&diff, &guards);
        assert_eq!(blocked.len(), 1, "skip_verification must not unlock diff-gate ops");
        assert!(guards.skip_verification(), "but it does bypass the verification gate");
        assert!(!guards.allows_token("drop_column"));
    }

    #[test]
    fn test_force_implies_skip_verification_and_all_allows() {
        let g = MigrationGuards::force_all();
        assert!(g.skip_verification());
        assert!(g.allows_token("drop_table"));
        assert!(g.allows_token("set_not_null"));
        assert!(g.allows_token("anything_at_all"));
    }

    #[test]
    fn test_safe_diff_never_blocked() {
        let d = SchemaDiff::new(); // empty diff is safe
        assert!(evaluate_guarded_changes(&d, &MigrationGuards::default()).is_empty());
    }

    #[test]
    fn test_multiple_blocked_ops_each_listed() {
        let mut d = SchemaDiff::new();
        d.add_change(SchemaChange {
            table: "t".to_string(),
            change_type: ChangeType::DropTable,
            column: None,
            from_type: None,
            to_type: None,
            compatibility: ChangeCompatibility::DataLoss,
            reason: None,
        });
        d.add_change(SchemaChange {
            table: "t".to_string(),
            change_type: ChangeType::DropColumn,
            column: Some("c".to_string()),
            from_type: None,
            to_type: None,
            compatibility: ChangeCompatibility::DataLoss,
            reason: None,
        });
        // Permit only drop_table → drop_column remains blocked and is named.
        let blocked = evaluate_guarded_changes(&d, &allow_only(&["drop_table"]));
        assert_eq!(blocked.len(), 1);
        assert!(blocked[0].contains("--allow-drop-column"));
    }
}
