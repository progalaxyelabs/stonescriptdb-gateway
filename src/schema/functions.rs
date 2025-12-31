use crate::error::{GatewayError, Result};
use deadpool_postgres::Pool;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

pub struct FunctionDeployer;

impl FunctionDeployer {
    pub fn new() -> Self {
        Self
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
                    if ext == "pssql" {
                        files.push(path);
                    }
                }
            }
        }

        // Sort for consistent ordering
        files.sort();

        Ok(files)
    }

    pub async fn deploy_functions(
        &self,
        pool: &Pool,
        database: &str,
        functions_dir: &Path,
    ) -> Result<usize> {
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

        for file_path in &function_files {
            let file_name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            debug!("Deploying function: {} to {}", file_name, database);

            let sql = fs::read_to_string(file_path).map_err(|e| {
                GatewayError::FunctionDeployFailed {
                    database: database.to_string(),
                    function: file_name.to_string(),
                    cause: format!("Failed to read file: {}", e),
                }
            })?;

            // Execute the function definition (should be CREATE OR REPLACE FUNCTION)
            match client.batch_execute(&sql).await {
                Ok(_) => {
                    deployed += 1;
                }
                Err(e) => {
                    // Log warning but continue with other functions
                    // This allows partial deployments
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
            "Deployed {} functions to database {}",
            deployed, database
        );

        Ok(deployed)
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
