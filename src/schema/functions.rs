//! Function deployer with signature tracking
//!
//! Handles function deployment with proper signature change detection.
//! When a function signature changes (parameter rename, type change, etc.),
//! the old function is dropped before deploying the new one.

use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Represents a parsed function signature
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub name: String,
    pub parameters: Vec<FunctionParameter>,
    pub return_type: String,
    pub body_checksum: String,
}

/// Represents a function parameter
#[derive(Debug, Clone)]
pub struct FunctionParameter {
    pub name: Option<String>,
    pub data_type: String,
    pub has_default: bool,
}

impl FunctionSignature {
    /// Generate a unique identifier for this signature (used for DROP)
    pub fn drop_signature(&self) -> String {
        // PostgreSQL identifies functions by name + parameter types (not names)
        let param_types: Vec<&str> = self.parameters.iter()
            .map(|p| p.data_type.as_str())
            .collect();

        if param_types.is_empty() {
            self.name.clone()
        } else {
            format!("{}({})", self.name, param_types.join(", "))
        }
    }

    /// Generate a key for tracking (name + param types)
    pub fn tracking_key(&self) -> String {
        self.drop_signature().to_lowercase()
    }
}

pub struct FunctionDeployer;

impl FunctionDeployer {
    pub fn new() -> Self {
        Self
    }

    /// Ensure the function tracking table exists
    pub async fn ensure_tracking_table(&self, pool: &Pool, database: &str) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _stonescriptdb_gateway_functions (
                    id SERIAL PRIMARY KEY,
                    function_name TEXT NOT NULL,
                    signature TEXT NOT NULL,
                    param_types TEXT[] NOT NULL,
                    return_type TEXT NOT NULL,
                    body_checksum TEXT NOT NULL,
                    source_file TEXT NOT NULL,
                    deployed_at TIMESTAMPTZ DEFAULT NOW(),
                    UNIQUE(function_name, param_types)
                )
                "#,
                &[],
            )
            .await
            .map_err(|e| GatewayError::FunctionDeployFailed {
                database: database.to_string(),
                function: "_stonescriptdb_gateway_functions table creation".to_string(),
                cause: e.to_string(),
            })?;

        Ok(())
    }

    pub fn find_function_files(&self, functions_dir: &Path) -> Result<Vec<PathBuf>> {
        if !functions_dir.exists() {
            debug!(
                "Functions directory {:?} does not exist, returning empty list",
                functions_dir
            );
            return Ok(Vec::new());
        }

        let mut files = Vec::new();

        for entry in fs::read_dir(functions_dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read functions directory: {}", e),
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

    /// Parse function signature from SQL
    pub fn parse_signature(&self, sql: &str) -> Option<FunctionSignature> {
        // Remove comments
        let sql = self.remove_comments(sql);

        // Match CREATE [OR REPLACE] FUNCTION name(params) RETURNS type
        let re = regex::Regex::new(
            r"(?is)CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(\w+)\s*\(([^)]*)\)\s*RETURNS\s+((?:TABLE\s*\([^)]+\)|\S+))"
        ).unwrap();

        let caps = re.captures(&sql)?;

        let name = caps[1].to_lowercase();
        let params_str = &caps[2];
        let return_type = caps[3].trim().to_uppercase();

        // Parse parameters
        let parameters = self.parse_parameters(params_str);

        // Compute body checksum
        let body_checksum = self.compute_body_checksum(&sql);

        Some(FunctionSignature {
            name,
            parameters,
            return_type,
            body_checksum,
        })
    }

    /// Parse function parameters
    fn parse_parameters(&self, params_str: &str) -> Vec<FunctionParameter> {
        if params_str.trim().is_empty() {
            return Vec::new();
        }

        let mut parameters = Vec::new();

        // Split by comma, handling nested parentheses
        let parts = self.split_params(params_str);

        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            // Parse parameter: [name] type [DEFAULT value]
            let param = self.parse_single_parameter(part);
            if let Some(p) = param {
                parameters.push(p);
            }
        }

        parameters
    }

    /// Split parameter string by commas, respecting parentheses
    fn split_params(&self, s: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut paren_depth = 0;

        for ch in s.chars() {
            match ch {
                '(' => {
                    paren_depth += 1;
                    current.push(ch);
                }
                ')' => {
                    paren_depth -= 1;
                    current.push(ch);
                }
                ',' if paren_depth == 0 => {
                    parts.push(current.trim().to_string());
                    current = String::new();
                }
                _ => {
                    current.push(ch);
                }
            }
        }

        if !current.trim().is_empty() {
            parts.push(current.trim().to_string());
        }

        parts
    }

    /// Parse a single parameter definition
    fn parse_single_parameter(&self, param: &str) -> Option<FunctionParameter> {
        let has_default = param.to_uppercase().contains("DEFAULT");

        // Remove DEFAULT clause for parsing
        let param_clean = regex::Regex::new(r"(?i)\s+DEFAULT\s+.*$")
            .unwrap()
            .replace(param, "")
            .to_string();

        let parts: Vec<&str> = param_clean.split_whitespace().collect();

        if parts.is_empty() {
            return None;
        }

        // Could be: "type" or "name type" or "IN name type" etc.
        let (name, data_type) = if parts.len() == 1 {
            // Just type
            (None, parts[0].to_uppercase())
        } else if parts[0].to_uppercase() == "IN" || parts[0].to_uppercase() == "OUT" || parts[0].to_uppercase() == "INOUT" {
            // Mode name type
            if parts.len() >= 3 {
                (Some(parts[1].to_lowercase()), parts[2..].join(" ").to_uppercase())
            } else {
                (None, parts[1..].join(" ").to_uppercase())
            }
        } else {
            // name type
            (Some(parts[0].to_lowercase()), parts[1..].join(" ").to_uppercase())
        };

        Some(FunctionParameter {
            name,
            data_type,
            has_default,
        })
    }

    /// Remove SQL comments
    fn remove_comments(&self, sql: &str) -> String {
        let single_line_re = regex::Regex::new(r"--[^\n]*").unwrap();
        let sql = single_line_re.replace_all(sql, "");

        let multi_line_re = regex::Regex::new(r"/\*[\s\S]*?\*/").unwrap();
        multi_line_re.replace_all(&sql, "").to_string()
    }

    /// Normalize SQL for checksum comparison
    /// - Removes comments (already done before this is called)
    /// - Collapses all whitespace (spaces, tabs, newlines) to single space
    /// - Trims leading/trailing whitespace
    /// - Lowercases for case-insensitive comparison
    fn normalize_for_checksum(&self, sql: &str) -> String {
        let whitespace_re = regex::Regex::new(r"\s+").unwrap();
        whitespace_re.replace_all(sql, " ").trim().to_lowercase()
    }

    /// Compute checksum of function body (normalized)
    fn compute_body_checksum(&self, sql: &str) -> String {
        let normalized = self.normalize_for_checksum(sql);
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub async fn deploy_functions(
        &self,
        pool: &Pool,
        database: &str,
        functions_dir: &Path,
    ) -> Result<usize> {
        // Ensure tracking table exists
        self.ensure_tracking_table(pool, database).await?;

        let function_files = self.find_function_files(functions_dir)?;
        debug!(
            "Found {} function files in {:?}",
            function_files.len(),
            functions_dir
        );

        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        let mut deployed = 0;
        let mut skipped = 0;

        for file_path in &function_files {
            let file_name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            let sql = fs::read_to_string(file_path).map_err(|e| {
                GatewayError::FunctionDeployFailed {
                    database: database.to_string(),
                    function: file_name.to_string(),
                    cause: format!("Failed to read file: {}", e),
                }
            })?;

            // Parse the function signature
            let signature = match self.parse_signature(&sql) {
                Some(sig) => sig,
                None => {
                    warn!(
                        "Could not parse function signature from {}, deploying without tracking",
                        file_name
                    );
                    // Fall back to simple deployment
                    client.batch_execute(&sql).await.map_err(|e| {
                        GatewayError::FunctionDeployFailed {
                            database: database.to_string(),
                            function: file_name.to_string(),
                            cause: e.to_string(),
                        }
                    })?;
                    deployed += 1;
                    continue;
                }
            };

            // Check if we need to deploy (checksum changed)
            let needs_deploy = self
                .check_needs_deploy(&client, database, &signature, file_name)
                .await?;

            if !needs_deploy {
                debug!(
                    "Skipping {} - unchanged (checksum match)",
                    signature.name
                );
                skipped += 1;
                continue;
            }

            debug!("Deploying function: {} to {}", signature.name, database);

            // Check for signature changes that require DROP
            self.handle_signature_change(&client, database, &signature, file_name)
                .await?;

            // Deploy the function
            match client.batch_execute(&sql).await {
                Ok(_) => {
                    // Update tracking
                    self.update_tracking(&client, database, &signature, file_name)
                        .await?;
                    deployed += 1;
                }
                Err(e) => {
                    warn!(
                        "Failed to deploy function {} to {}: {}",
                        file_name, database, e
                    );
                    return Err(GatewayError::FunctionDeployFailed {
                        database: database.to_string(),
                        function: file_name.to_string(),
                        cause: e.to_string(),
                    });
                }
            }
        }

        info!(
            "Deployed {} functions to database {} ({} unchanged)",
            deployed, database, skipped
        );

        Ok(deployed)
    }

    /// Check if function needs to be deployed (checksum changed)
    async fn check_needs_deploy(
        &self,
        client: &deadpool_postgres::Object,
        _database: &str,
        signature: &FunctionSignature,
        _file_name: &str,
    ) -> Result<bool> {
        let param_types: Vec<String> = signature
            .parameters
            .iter()
            .map(|p| p.data_type.clone())
            .collect();

        let row = client
            .query_opt(
                "SELECT body_checksum FROM _stonescriptdb_gateway_functions
                 WHERE function_name = $1 AND param_types = $2",
                &[&signature.name, &param_types],
            )
            .await
            .unwrap_or(None);

        match row {
            Some(row) => {
                let stored_checksum: String = row.get(0);
                Ok(stored_checksum != signature.body_checksum)
            }
            None => Ok(true), // Not tracked yet, needs deploy
        }
    }

    /// Handle signature changes - drop old function if signature changed
    async fn handle_signature_change(
        &self,
        client: &deadpool_postgres::Object,
        database: &str,
        new_signature: &FunctionSignature,
        file_name: &str,
    ) -> Result<()> {
        // Find existing functions with same source file but different signature
        let rows = client
            .query(
                "SELECT function_name, param_types FROM _stonescriptdb_gateway_functions
                 WHERE source_file = $1",
                &[&file_name],
            )
            .await
            .unwrap_or_default();

        let new_param_types: Vec<String> = new_signature
            .parameters
            .iter()
            .map(|p| p.data_type.clone())
            .collect();

        for row in rows {
            let old_name: String = row.get(0);
            let old_param_types: Vec<String> = row.get(1);

            // Check if signature changed
            if old_name != new_signature.name || old_param_types != new_param_types {
                // Signature changed - need to drop old function
                let old_sig = if old_param_types.is_empty() {
                    format!("{}()", old_name)
                } else {
                    format!("{}({})", old_name, old_param_types.join(", "))
                };

                info!(
                    "Function signature changed in {}: dropping old signature {}",
                    file_name, old_sig
                );

                let drop_sql = format!("DROP FUNCTION IF EXISTS {}", old_sig);
                client.execute(&drop_sql, &[]).await.map_err(|e| {
                    GatewayError::FunctionDeployFailed {
                        database: database.to_string(),
                        function: file_name.to_string(),
                        cause: format!("Failed to drop old function {}: {}", old_sig, e),
                    }
                })?;

                // Remove old tracking record
                client
                    .execute(
                        "DELETE FROM _stonescriptdb_gateway_functions
                         WHERE function_name = $1 AND param_types = $2",
                        &[&old_name, &old_param_types],
                    )
                    .await
                    .ok();
            }
        }

        Ok(())
    }

    /// Update function tracking record
    async fn update_tracking(
        &self,
        client: &deadpool_postgres::Object,
        _database: &str,
        signature: &FunctionSignature,
        file_name: &str,
    ) -> Result<()> {
        let param_types: Vec<String> = signature
            .parameters
            .iter()
            .map(|p| p.data_type.clone())
            .collect();

        client
            .execute(
                r#"
                INSERT INTO _stonescriptdb_gateway_functions
                    (function_name, signature, param_types, return_type, body_checksum, source_file, deployed_at)
                VALUES ($1, $2, $3, $4, $5, $6, NOW())
                ON CONFLICT (function_name, param_types)
                DO UPDATE SET
                    signature = EXCLUDED.signature,
                    return_type = EXCLUDED.return_type,
                    body_checksum = EXCLUDED.body_checksum,
                    source_file = EXCLUDED.source_file,
                    deployed_at = NOW()
                "#,
                &[
                    &signature.name,
                    &signature.drop_signature(),
                    &param_types,
                    &signature.return_type,
                    &signature.body_checksum,
                    &file_name,
                ],
            )
            .await
            .ok();

        Ok(())
    }

    pub async fn deploy_single_function(
        &self,
        pool: &Pool,
        database: &str,
        function_name: &str,
        sql: &str,
    ) -> Result<()> {
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: database.to_string(),
            cause: e.to_string(),
        })?;

        client
            .batch_execute(sql)
            .await
            .map_err(|e| GatewayError::FunctionDeployFailed {
                database: database.to_string(),
                function: function_name.to_string(),
                cause: e.to_string(),
            })?;

        Ok(())
    }
}

impl Default for FunctionDeployer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let deployer = FunctionDeployer::new();

        let sql = r#"
            CREATE OR REPLACE FUNCTION get_user(p_id INT)
            RETURNS TABLE (id INT, name TEXT) AS $$
            BEGIN
                RETURN QUERY SELECT * FROM users WHERE id = p_id;
            END;
            $$ LANGUAGE plpgsql;
        "#;

        let sig = deployer.parse_signature(sql).unwrap();
        assert_eq!(sig.name, "get_user");
        assert_eq!(sig.parameters.len(), 1);
        assert_eq!(sig.parameters[0].name, Some("p_id".to_string()));
        assert_eq!(sig.parameters[0].data_type, "INT");
    }

    #[test]
    fn test_parse_no_params() {
        let deployer = FunctionDeployer::new();

        let sql = r#"
            CREATE FUNCTION get_all_users()
            RETURNS TABLE (id INT) AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        let sig = deployer.parse_signature(sql).unwrap();
        assert_eq!(sig.name, "get_all_users");
        assert!(sig.parameters.is_empty());
    }

    #[test]
    fn test_parse_with_defaults() {
        let deployer = FunctionDeployer::new();

        let sql = r#"
            CREATE OR REPLACE FUNCTION get_todos(
                p_user_id INT,
                p_include_completed BOOLEAN DEFAULT TRUE
            )
            RETURNS SETOF todos AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        let sig = deployer.parse_signature(sql).unwrap();
        assert_eq!(sig.name, "get_todos");
        assert_eq!(sig.parameters.len(), 2);
        assert!(!sig.parameters[0].has_default);
        assert!(sig.parameters[1].has_default);
    }

    #[test]
    fn test_drop_signature() {
        let sig = FunctionSignature {
            name: "get_user".to_string(),
            parameters: vec![
                FunctionParameter {
                    name: Some("p_id".to_string()),
                    data_type: "INT".to_string(),
                    has_default: false,
                },
            ],
            return_type: "TABLE".to_string(),
            body_checksum: "abc".to_string(),
        };

        assert_eq!(sig.drop_signature(), "get_user(INT)");
    }

    #[test]
    fn test_param_rename_same_signature() {
        let deployer = FunctionDeployer::new();

        // Before: p_id INT
        let sql_before = r#"
            CREATE OR REPLACE FUNCTION get_user(p_id INT)
            RETURNS TABLE (id INT) AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        // After: p_user_id INT (renamed parameter)
        let sql_after = r#"
            CREATE OR REPLACE FUNCTION get_user(p_user_id INT)
            RETURNS TABLE (id INT) AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        let sig_before = deployer.parse_signature(sql_before).unwrap();
        let sig_after = deployer.parse_signature(sql_after).unwrap();

        // Same function identity (name + param types)
        assert_eq!(sig_before.name, sig_after.name);
        assert_eq!(sig_before.parameters.len(), sig_after.parameters.len());
        assert_eq!(sig_before.parameters[0].data_type, sig_after.parameters[0].data_type);
        assert_eq!(sig_before.drop_signature(), sig_after.drop_signature());

        // But different checksums (will trigger redeploy)
        assert_ne!(sig_before.body_checksum, sig_after.body_checksum);
    }

    #[test]
    fn test_added_param_different_signature() {
        let deployer = FunctionDeployer::new();

        // Before: just INT
        let sql_before = r#"
            CREATE OR REPLACE FUNCTION get_user(p_id INT)
            RETURNS TABLE (id INT) AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        // After: INT + BOOLEAN DEFAULT
        let sql_after = r#"
            CREATE OR REPLACE FUNCTION get_user(p_id INT, p_include_deleted BOOLEAN DEFAULT FALSE)
            RETURNS TABLE (id INT) AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        let sig_before = deployer.parse_signature(sql_before).unwrap();
        let sig_after = deployer.parse_signature(sql_after).unwrap();

        // Different param types = different PostgreSQL function identity
        assert_eq!(sig_before.parameters.len(), 1);
        assert_eq!(sig_after.parameters.len(), 2);
        assert_ne!(sig_before.drop_signature(), sig_after.drop_signature());

        // This means: old get_user(INT) must be DROPPED before CREATE OR REPLACE get_user(INT, BOOLEAN)
        // Otherwise both functions will exist in PostgreSQL!
        assert_eq!(sig_before.drop_signature(), "get_user(INT)");
        assert_eq!(sig_after.drop_signature(), "get_user(INT, BOOLEAN)");
    }

    #[test]
    fn test_whitespace_normalization() {
        let deployer = FunctionDeployer::new();

        // Same function with different formatting
        let sql_compact = r#"CREATE OR REPLACE FUNCTION get_user(p_id INT) RETURNS TABLE (id INT) AS $$ BEGIN END; $$ LANGUAGE plpgsql;"#;

        let sql_formatted = r#"
            CREATE OR REPLACE FUNCTION get_user(p_id INT)
            RETURNS TABLE (id INT)
            AS $$
                BEGIN
                    END;
            $$
            LANGUAGE plpgsql;
        "#;

        let sig_compact = deployer.parse_signature(sql_compact).unwrap();
        let sig_formatted = deployer.parse_signature(sql_formatted).unwrap();

        // Both should have identical checksums (whitespace normalized)
        assert_eq!(sig_compact.body_checksum, sig_formatted.body_checksum);
    }

    #[test]
    fn test_comment_removal() {
        let deployer = FunctionDeployer::new();

        let sql_no_comments = r#"
            CREATE OR REPLACE FUNCTION get_user(p_id INT)
            RETURNS TABLE (id INT) AS $$
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        let sql_with_comments = r#"
            -- This is a comment
            CREATE OR REPLACE FUNCTION get_user(p_id INT)
            RETURNS TABLE (id INT) AS $$
            /* Multi-line
               comment */
            BEGIN END;
            $$ LANGUAGE plpgsql;
        "#;

        let sig_no_comments = deployer.parse_signature(sql_no_comments).unwrap();
        let sig_with_comments = deployer.parse_signature(sql_with_comments).unwrap();

        // Both should have identical checksums (comments removed)
        assert_eq!(sig_no_comments.body_checksum, sig_with_comments.body_checksum);
    }

    #[test]
    fn test_case_insensitive_checksum() {
        let deployer = FunctionDeployer::new();

        let sql_upper = r#"
            CREATE OR REPLACE FUNCTION GET_USER(P_ID INT)
            RETURNS TABLE (ID INT) AS $$
            BEGIN END;
            $$ LANGUAGE PLPGSQL;
        "#;

        let sql_lower = r#"
            create or replace function get_user(p_id int)
            returns table (id int) as $$
            begin end;
            $$ language plpgsql;
        "#;

        let sig_upper = deployer.parse_signature(sql_upper).unwrap();
        let sig_lower = deployer.parse_signature(sql_lower).unwrap();

        // Both should have identical checksums (case normalized)
        assert_eq!(sig_upper.body_checksum, sig_lower.body_checksum);
    }
}
