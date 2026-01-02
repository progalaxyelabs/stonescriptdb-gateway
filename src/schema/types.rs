//! PostgreSQL Type Compatibility Matrix
//!
//! Defines which type changes are SAFE vs DATALOSS.
//! Used to pre-validate migrations before execution.
//!
//! Classification:
//! - SAFE: Can be done without data loss
//! - DATALOSS: May truncate or lose data
//! - INCOMPATIBLE: Cannot be cast at all

use serde::Serialize;
use std::collections::HashMap;

/// Result of a type compatibility check
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TypeCompatibility {
    /// Same type, no change needed
    Identical,
    /// Safe to change, no data loss
    Safe,
    /// May cause data loss (truncation, precision loss)
    DataLoss { reason: String },
    /// Types are incompatible, cannot cast
    Incompatible { reason: String },
}

impl TypeCompatibility {
    pub fn is_safe(&self) -> bool {
        matches!(self, TypeCompatibility::Identical | TypeCompatibility::Safe)
    }
}

/// Checks type compatibility for PostgreSQL column changes
pub struct TypeChecker {
    /// Widening rules: from_type -> list of safe target types
    safe_widenings: HashMap<&'static str, Vec<&'static str>>,
    /// Narrowing rules: from_type -> (to_type, reason)
    dataloss_narrowings: HashMap<(&'static str, &'static str), &'static str>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut safe_widenings: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        let mut dataloss_narrowings: HashMap<(&'static str, &'static str), &'static str> = HashMap::new();

        // ═══════════════════════════════════════════════════════════════
        // INTEGER TYPES - Widening is safe, narrowing may overflow
        // ═══════════════════════════════════════════════════════════════

        // SMALLINT (int2) -> larger types
        safe_widenings.insert("SMALLINT", vec!["INTEGER", "INT", "INT4", "BIGINT", "INT8", "NUMERIC", "DECIMAL", "REAL", "FLOAT4", "DOUBLE PRECISION", "FLOAT8"]);
        safe_widenings.insert("INT2", vec!["INTEGER", "INT", "INT4", "BIGINT", "INT8", "NUMERIC", "DECIMAL", "REAL", "FLOAT4", "DOUBLE PRECISION", "FLOAT8"]);

        // INTEGER (int4) -> larger types
        safe_widenings.insert("INTEGER", vec!["BIGINT", "INT8", "NUMERIC", "DECIMAL", "DOUBLE PRECISION", "FLOAT8"]);
        safe_widenings.insert("INT", vec!["BIGINT", "INT8", "NUMERIC", "DECIMAL", "DOUBLE PRECISION", "FLOAT8"]);
        safe_widenings.insert("INT4", vec!["BIGINT", "INT8", "NUMERIC", "DECIMAL", "DOUBLE PRECISION", "FLOAT8"]);

        // BIGINT -> numeric types
        safe_widenings.insert("BIGINT", vec!["NUMERIC", "DECIMAL"]);
        safe_widenings.insert("INT8", vec!["NUMERIC", "DECIMAL"]);

        // Integer narrowing = DATALOSS
        dataloss_narrowings.insert(("BIGINT", "INTEGER"), "May overflow: BIGINT max 9.2e18, INTEGER max 2.1e9");
        dataloss_narrowings.insert(("BIGINT", "INT"), "May overflow: BIGINT max 9.2e18, INTEGER max 2.1e9");
        dataloss_narrowings.insert(("BIGINT", "SMALLINT"), "May overflow: BIGINT max 9.2e18, SMALLINT max 32767");
        dataloss_narrowings.insert(("INTEGER", "SMALLINT"), "May overflow: INTEGER max 2.1e9, SMALLINT max 32767");
        dataloss_narrowings.insert(("INT", "SMALLINT"), "May overflow: INTEGER max 2.1e9, SMALLINT max 32767");
        dataloss_narrowings.insert(("INT4", "INT2"), "May overflow: INTEGER max 2.1e9, SMALLINT max 32767");

        // ═══════════════════════════════════════════════════════════════
        // STRING TYPES - Widening is safe, narrowing may truncate
        // ═══════════════════════════════════════════════════════════════

        // CHAR -> larger types
        safe_widenings.insert("CHAR", vec!["VARCHAR", "CHARACTER VARYING", "TEXT"]);
        safe_widenings.insert("CHARACTER", vec!["VARCHAR", "CHARACTER VARYING", "TEXT"]);

        // VARCHAR -> TEXT (always safe)
        safe_widenings.insert("VARCHAR", vec!["TEXT"]);
        safe_widenings.insert("CHARACTER VARYING", vec!["TEXT"]);

        // TEXT -> VARCHAR = DATALOSS (may truncate)
        dataloss_narrowings.insert(("TEXT", "VARCHAR"), "May truncate: TEXT has no limit, VARCHAR has limit");
        dataloss_narrowings.insert(("TEXT", "CHAR"), "May truncate: TEXT has no limit, CHAR is fixed length");

        // ═══════════════════════════════════════════════════════════════
        // FLOATING POINT - Precision considerations
        // ═══════════════════════════════════════════════════════════════

        // REAL -> DOUBLE PRECISION (safe, more precision)
        safe_widenings.insert("REAL", vec!["DOUBLE PRECISION", "FLOAT8", "NUMERIC", "DECIMAL"]);
        safe_widenings.insert("FLOAT4", vec!["DOUBLE PRECISION", "FLOAT8", "NUMERIC", "DECIMAL"]);

        // DOUBLE PRECISION -> NUMERIC (safe)
        safe_widenings.insert("DOUBLE PRECISION", vec!["NUMERIC", "DECIMAL"]);
        safe_widenings.insert("FLOAT8", vec!["NUMERIC", "DECIMAL"]);

        // Float narrowing = DATALOSS
        dataloss_narrowings.insert(("DOUBLE PRECISION", "REAL"), "May lose precision: DOUBLE has 15 digits, REAL has 6");
        dataloss_narrowings.insert(("FLOAT8", "FLOAT4"), "May lose precision: DOUBLE has 15 digits, REAL has 6");
        dataloss_narrowings.insert(("NUMERIC", "REAL"), "May lose precision: NUMERIC is exact, REAL is approximate");
        dataloss_narrowings.insert(("NUMERIC", "DOUBLE PRECISION"), "May lose precision: NUMERIC is exact, DOUBLE is approximate");

        // ═══════════════════════════════════════════════════════════════
        // DATE/TIME TYPES
        // ═══════════════════════════════════════════════════════════════

        // DATE -> TIMESTAMP (safe, adds time component as 00:00:00)
        safe_widenings.insert("DATE", vec!["TIMESTAMP", "TIMESTAMP WITHOUT TIME ZONE", "TIMESTAMPTZ", "TIMESTAMP WITH TIME ZONE"]);

        // TIMESTAMP -> TIMESTAMPTZ (safe in most cases, but depends on timezone)
        // We mark it safe because PostgreSQL handles it, but user should be aware
        safe_widenings.insert("TIMESTAMP", vec!["TIMESTAMPTZ", "TIMESTAMP WITH TIME ZONE"]);
        safe_widenings.insert("TIMESTAMP WITHOUT TIME ZONE", vec!["TIMESTAMP WITH TIME ZONE", "TIMESTAMPTZ"]);

        // TIME -> TIME WITH TIME ZONE
        safe_widenings.insert("TIME", vec!["TIME WITH TIME ZONE", "TIMETZ"]);
        safe_widenings.insert("TIME WITHOUT TIME ZONE", vec!["TIME WITH TIME ZONE", "TIMETZ"]);

        // TIMESTAMP -> DATE = DATALOSS (loses time)
        dataloss_narrowings.insert(("TIMESTAMP", "DATE"), "Loses time component");
        dataloss_narrowings.insert(("TIMESTAMPTZ", "DATE"), "Loses time and timezone");
        dataloss_narrowings.insert(("TIMESTAMP WITH TIME ZONE", "DATE"), "Loses time and timezone");

        // ═══════════════════════════════════════════════════════════════
        // BOOLEAN
        // ═══════════════════════════════════════════════════════════════

        // BOOLEAN -> INTEGER (safe: true=1, false=0)
        safe_widenings.insert("BOOLEAN", vec!["INTEGER", "INT", "SMALLINT", "BIGINT"]);
        safe_widenings.insert("BOOL", vec!["INTEGER", "INT", "SMALLINT", "BIGINT"]);

        // INTEGER -> BOOLEAN = DATALOSS (only 0/1 preserved correctly)
        dataloss_narrowings.insert(("INTEGER", "BOOLEAN"), "Only 0 and 1 map to FALSE/TRUE, other values become TRUE");
        dataloss_narrowings.insert(("INT", "BOOLEAN"), "Only 0 and 1 map to FALSE/TRUE, other values become TRUE");

        // ═══════════════════════════════════════════════════════════════
        // UUID
        // ═══════════════════════════════════════════════════════════════

        // UUID -> TEXT/VARCHAR (safe, just string representation)
        safe_widenings.insert("UUID", vec!["TEXT", "VARCHAR", "CHARACTER VARYING"]);

        // TEXT -> UUID = may fail if not valid UUID format
        dataloss_narrowings.insert(("TEXT", "UUID"), "May fail: TEXT must contain valid UUID format");
        dataloss_narrowings.insert(("VARCHAR", "UUID"), "May fail: VARCHAR must contain valid UUID format");

        // ═══════════════════════════════════════════════════════════════
        // JSON/JSONB
        // ═══════════════════════════════════════════════════════════════

        // JSON -> JSONB (safe, just different storage)
        safe_widenings.insert("JSON", vec!["JSONB", "TEXT"]);

        // JSONB -> JSON/TEXT (safe)
        safe_widenings.insert("JSONB", vec!["JSON", "TEXT"]);

        // TEXT -> JSON/JSONB = may fail if not valid JSON
        dataloss_narrowings.insert(("TEXT", "JSON"), "May fail: TEXT must contain valid JSON");
        dataloss_narrowings.insert(("TEXT", "JSONB"), "May fail: TEXT must contain valid JSON");

        // ═══════════════════════════════════════════════════════════════
        // SERIAL types (just aliases for INTEGER + sequence)
        // ═══════════════════════════════════════════════════════════════

        safe_widenings.insert("SERIAL", vec!["BIGSERIAL", "INTEGER", "BIGINT"]);
        safe_widenings.insert("SMALLSERIAL", vec!["SERIAL", "BIGSERIAL", "SMALLINT", "INTEGER", "BIGINT"]);
        safe_widenings.insert("BIGSERIAL", vec!["BIGINT", "NUMERIC"]);

        Self {
            safe_widenings,
            dataloss_narrowings,
        }
    }

    /// Check if a type change is compatible
    pub fn check_compatibility(&self, from_type: &str, to_type: &str) -> TypeCompatibility {
        let from_normalized = self.normalize_type(from_type);
        let to_normalized = self.normalize_type(to_type);

        // Same type
        if from_normalized == to_normalized {
            return TypeCompatibility::Identical;
        }

        // Check for VARCHAR length changes
        if let Some(result) = self.check_varchar_change(&from_normalized, &to_normalized) {
            return result;
        }

        // Check for NUMERIC precision changes
        if let Some(result) = self.check_numeric_change(&from_normalized, &to_normalized) {
            return result;
        }

        // Check safe widenings
        let from_base = self.extract_base_type(&from_normalized);
        let to_base = self.extract_base_type(&to_normalized);

        if let Some(safe_targets) = self.safe_widenings.get(from_base.as_str()) {
            if safe_targets.iter().any(|t| *t == to_base) {
                return TypeCompatibility::Safe;
            }
        }

        // Check known dataloss narrowings
        if let Some(reason) = self.dataloss_narrowings.get(&(from_base.as_str(), to_base.as_str())) {
            return TypeCompatibility::DataLoss {
                reason: reason.to_string(),
            };
        }

        // Check reverse (if to->from is safe, then from->to is dataloss)
        if let Some(safe_targets) = self.safe_widenings.get(to_base.as_str()) {
            if safe_targets.iter().any(|t| *t == from_base) {
                return TypeCompatibility::DataLoss {
                    reason: format!("Narrowing from {} to {} may lose data", from_type, to_type),
                };
            }
        }

        // Unknown combination - treat as incompatible for safety
        TypeCompatibility::Incompatible {
            reason: format!("Unknown type change: {} -> {}. Add to compatibility matrix if this should be allowed.", from_type, to_type),
        }
    }

    /// Normalize type name for comparison
    fn normalize_type(&self, type_name: &str) -> String {
        type_name
            .trim()
            .to_uppercase()
            .replace("CHARACTER VARYING", "VARCHAR")
            .replace("INT4", "INTEGER")
            .replace("INT8", "BIGINT")
            .replace("INT2", "SMALLINT")
            .replace("FLOAT4", "REAL")
            .replace("FLOAT8", "DOUBLE PRECISION")
            .replace("BOOL", "BOOLEAN")
            .replace("TIMESTAMP WITHOUT TIME ZONE", "TIMESTAMP")
            .replace("TIMESTAMP WITH TIME ZONE", "TIMESTAMPTZ")
    }

    /// Extract base type without parameters (e.g., VARCHAR(100) -> VARCHAR)
    fn extract_base_type(&self, type_name: &str) -> String {
        if let Some(paren_pos) = type_name.find('(') {
            type_name[..paren_pos].trim().to_string()
        } else {
            type_name.to_string()
        }
    }

    /// Extract length from VARCHAR(n) or CHAR(n)
    fn extract_length(&self, type_name: &str) -> Option<usize> {
        let re = regex::Regex::new(r"\((\d+)\)").unwrap();
        re.captures(type_name)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse().ok())
    }

    /// Check VARCHAR length changes
    fn check_varchar_change(&self, from: &str, to: &str) -> Option<TypeCompatibility> {
        let from_base = self.extract_base_type(from);
        let to_base = self.extract_base_type(to);

        // Both must be VARCHAR or CHAR
        let is_string_type = |t: &str| t == "VARCHAR" || t == "CHAR" || t == "CHARACTER";

        if !is_string_type(&from_base) || !is_string_type(&to_base) {
            return None;
        }

        let from_len = self.extract_length(from);
        let to_len = self.extract_length(to);

        match (from_len, to_len) {
            (Some(from_l), Some(to_l)) => {
                if to_l >= from_l {
                    Some(TypeCompatibility::Safe)
                } else {
                    Some(TypeCompatibility::DataLoss {
                        reason: format!("May truncate: reducing from {} to {} characters", from_l, to_l),
                    })
                }
            }
            (Some(_), None) => {
                // Going to unlimited (TEXT behavior) - safe
                if to_base == "VARCHAR" {
                    Some(TypeCompatibility::Safe)
                } else {
                    None
                }
            }
            (None, Some(_)) => {
                // Going from unlimited to limited - dataloss
                Some(TypeCompatibility::DataLoss {
                    reason: "May truncate: adding length limit".to_string(),
                })
            }
            (None, None) => Some(TypeCompatibility::Safe),
        }
    }

    /// Check NUMERIC/DECIMAL precision changes
    fn check_numeric_change(&self, from: &str, to: &str) -> Option<TypeCompatibility> {
        let from_base = self.extract_base_type(from);
        let to_base = self.extract_base_type(to);

        if from_base != "NUMERIC" && from_base != "DECIMAL" {
            return None;
        }
        if to_base != "NUMERIC" && to_base != "DECIMAL" {
            return None;
        }

        // Parse precision and scale: NUMERIC(precision, scale)
        let parse_precision_scale = |t: &str| -> Option<(usize, usize)> {
            let re = regex::Regex::new(r"\((\d+)(?:,\s*(\d+))?\)").unwrap();
            re.captures(t).map(|caps| {
                let precision: usize = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
                let scale: usize = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
                (precision, scale)
            })
        };

        let from_ps = parse_precision_scale(from);
        let to_ps = parse_precision_scale(to);

        match (from_ps, to_ps) {
            (Some((from_p, from_s)), Some((to_p, to_s))) => {
                if to_p >= from_p && to_s >= from_s {
                    Some(TypeCompatibility::Safe)
                } else {
                    Some(TypeCompatibility::DataLoss {
                        reason: format!(
                            "May lose precision: NUMERIC({},{}) to NUMERIC({},{})",
                            from_p, from_s, to_p, to_s
                        ),
                    })
                }
            }
            (Some(_), None) => Some(TypeCompatibility::Safe), // Going to unlimited precision
            (None, Some(_)) => Some(TypeCompatibility::DataLoss {
                reason: "May lose precision: adding precision limit".to_string(),
            }),
            (None, None) => Some(TypeCompatibility::Identical),
        }
    }

    /// Format the compatibility matrix as a readable string
    pub fn format_matrix(&self) -> String {
        let mut output = String::new();

        output.push_str("═══════════════════════════════════════════════════════════════\n");
        output.push_str("              POSTGRESQL TYPE COMPATIBILITY MATRIX\n");
        output.push_str("═══════════════════════════════════════════════════════════════\n\n");

        output.push_str("SAFE WIDENINGS (no data loss):\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");

        let mut safe_entries: Vec<_> = self.safe_widenings.iter().collect();
        safe_entries.sort_by_key(|(k, _)| *k);

        for (from, to_list) in safe_entries {
            output.push_str(&format!("  {} → {}\n", from, to_list.join(", ")));
        }

        output.push_str("\nDATALOSS NARROWINGS (may lose data):\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");

        let mut dataloss_entries: Vec<_> = self.dataloss_narrowings.iter().collect();
        dataloss_entries.sort_by_key(|((from, to), _)| (*from, *to));

        for ((from, to), reason) in dataloss_entries {
            output.push_str(&format!("  {} → {}\n    Reason: {}\n", from, to, reason));
        }

        output.push_str("\n═══════════════════════════════════════════════════════════════\n");

        output
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_types() {
        let checker = TypeChecker::new();
        assert_eq!(
            checker.check_compatibility("INTEGER", "INTEGER"),
            TypeCompatibility::Identical
        );
        assert_eq!(
            checker.check_compatibility("VARCHAR(100)", "VARCHAR(100)"),
            TypeCompatibility::Identical
        );
    }

    #[test]
    fn test_safe_widenings() {
        let checker = TypeChecker::new();

        // Integer widenings
        assert!(checker.check_compatibility("SMALLINT", "INTEGER").is_safe());
        assert!(checker.check_compatibility("INTEGER", "BIGINT").is_safe());
        assert!(checker.check_compatibility("INT", "BIGINT").is_safe());

        // String widenings
        assert!(checker.check_compatibility("VARCHAR", "TEXT").is_safe());
        assert!(checker.check_compatibility("CHAR(10)", "VARCHAR(100)").is_safe());

        // Date/time widenings
        assert!(checker.check_compatibility("DATE", "TIMESTAMP").is_safe());
        assert!(checker.check_compatibility("TIMESTAMP", "TIMESTAMPTZ").is_safe());
    }

    #[test]
    fn test_varchar_length_changes() {
        let checker = TypeChecker::new();

        // Widening is safe
        assert!(checker.check_compatibility("VARCHAR(50)", "VARCHAR(100)").is_safe());
        assert!(checker.check_compatibility("VARCHAR(50)", "TEXT").is_safe());

        // Narrowing is dataloss
        let result = checker.check_compatibility("VARCHAR(100)", "VARCHAR(50)");
        assert!(matches!(result, TypeCompatibility::DataLoss { .. }));
    }

    #[test]
    fn test_numeric_precision_changes() {
        let checker = TypeChecker::new();

        // Widening precision is safe
        assert!(checker.check_compatibility("NUMERIC(10,2)", "NUMERIC(15,4)").is_safe());

        // Narrowing precision is dataloss
        let result = checker.check_compatibility("NUMERIC(15,4)", "NUMERIC(10,2)");
        assert!(matches!(result, TypeCompatibility::DataLoss { .. }));
    }

    #[test]
    fn test_dataloss_narrowings() {
        let checker = TypeChecker::new();

        // Integer narrowings
        let result = checker.check_compatibility("BIGINT", "INTEGER");
        assert!(matches!(result, TypeCompatibility::DataLoss { .. }));

        // String narrowings
        let result = checker.check_compatibility("TEXT", "VARCHAR(100)");
        assert!(matches!(result, TypeCompatibility::DataLoss { .. }));

        // Timestamp -> Date loses time
        let result = checker.check_compatibility("TIMESTAMP", "DATE");
        assert!(matches!(result, TypeCompatibility::DataLoss { .. }));
    }

    #[test]
    fn test_type_normalization() {
        let checker = TypeChecker::new();

        // INT4 = INTEGER
        assert!(checker.check_compatibility("INT4", "BIGINT").is_safe());

        // BOOL = BOOLEAN
        assert!(checker.check_compatibility("BOOL", "INTEGER").is_safe());

        // CHARACTER VARYING = VARCHAR
        assert!(checker.check_compatibility("CHARACTER VARYING(50)", "TEXT").is_safe());
    }

    #[test]
    fn test_incompatible_types() {
        let checker = TypeChecker::new();

        // UUID and INTEGER are incompatible
        let result = checker.check_compatibility("UUID", "INTEGER");
        assert!(matches!(result, TypeCompatibility::Incompatible { .. }));

        // BOOLEAN and TEXT
        let result = checker.check_compatibility("BOOLEAN", "TEXT");
        assert!(matches!(result, TypeCompatibility::Incompatible { .. }));
    }
}
