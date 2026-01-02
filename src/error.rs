use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("Database not found: platform={platform}, tenant_id={tenant_id:?}")]
    DatabaseNotFound {
        platform: String,
        tenant_id: Option<String>,
    },

    #[error("Database already exists: {database}")]
    DatabaseAlreadyExists { database: String },

    #[error("Migration failed in {database}: {migration} - {cause}")]
    MigrationFailed {
        database: String,
        migration: String,
        cause: String,
    },

    #[error("Function deployment failed in {database}: {function} - {cause}")]
    FunctionDeployFailed {
        database: String,
        function: String,
        cause: String,
    },

    #[error("Query failed for {function} in {database}: {cause}")]
    QueryFailed {
        database: String,
        function: String,
        cause: String,
    },

    #[error("Extension {extension} not available: {cause}")]
    ExtensionNotAvailable { extension: String, cause: String },

    #[error("Extension installation failed in {database}: {extension} - {cause}")]
    ExtensionInstallFailed {
        database: String,
        extension: String,
        cause: String,
    },

    #[error("Schema extraction failed: {cause}")]
    SchemaExtractionFailed { cause: String },

    #[error("Connection failed to {database}: {cause}")]
    ConnectionFailed { database: String, cause: String },

    #[error("Connection pool exhausted for {database}")]
    PoolExhausted { database: String },

    #[error("Unauthorized access from IP: {ip}")]
    Unauthorized { ip: String },

    #[error("Invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("Platform isolation violation: cannot access {target_platform} databases from {requesting_platform}")]
    PlatformIsolationViolation {
        requesting_platform: String,
        target_platform: String,
    },

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, error_response) = match &self {
            GatewayError::DatabaseNotFound { platform, tenant_id } => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    error: "database_not_found".to_string(),
                    message: format!(
                        "Database for platform '{}' with tenant {:?} not found",
                        platform, tenant_id
                    ),
                    database: Some(format_database_name(platform, tenant_id.as_deref())),
                    cause: None,
                },
            ),
            GatewayError::DatabaseAlreadyExists { database } => (
                StatusCode::CONFLICT,
                ErrorResponse {
                    error: "database_already_exists".to_string(),
                    message: format!("Database '{}' already exists", database),
                    database: Some(database.clone()),
                    cause: None,
                },
            ),
            GatewayError::MigrationFailed {
                database,
                migration,
                cause,
            } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "migration_failed".to_string(),
                    message: format!("Migration {} failed", migration),
                    database: Some(database.clone()),
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::FunctionDeployFailed {
                database,
                function,
                cause,
            } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "function_deploy_failed".to_string(),
                    message: format!("Function {} deployment failed", function),
                    database: Some(database.clone()),
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::QueryFailed { database, function, cause } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "query_failed".to_string(),
                    message: format!("Query for function '{}' failed", function),
                    database: Some(database.clone()),
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::ExtensionNotAvailable { extension, cause } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "extension_not_available".to_string(),
                    message: format!("PostgreSQL extension '{}' is not available on this server", extension),
                    database: None,
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::ExtensionInstallFailed { database, extension, cause } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "extension_install_failed".to_string(),
                    message: format!("Failed to install extension '{}'", extension),
                    database: Some(database.clone()),
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::SchemaExtractionFailed { cause } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "schema_extraction_failed".to_string(),
                    message: "Failed to extract schema from uploaded archive".to_string(),
                    database: None,
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::ConnectionFailed { database, cause } => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "connection_failed".to_string(),
                    message: format!("Failed to connect to database '{}'", database),
                    database: Some(database.clone()),
                    cause: Some(cause.clone()),
                },
            ),
            GatewayError::PoolExhausted { database } => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "pool_exhausted".to_string(),
                    message: format!("Connection pool exhausted for database '{}'", database),
                    database: Some(database.clone()),
                    cause: None,
                },
            ),
            GatewayError::Unauthorized { ip } => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    error: "unauthorized".to_string(),
                    message: format!("Access denied for IP address: {}", ip),
                    database: None,
                    cause: None,
                },
            ),
            GatewayError::InvalidRequest { message } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_request".to_string(),
                    message: message.clone(),
                    database: None,
                    cause: None,
                },
            ),
            GatewayError::PlatformIsolationViolation {
                requesting_platform,
                target_platform,
            } => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    error: "platform_isolation_violation".to_string(),
                    message: format!(
                        "Platform '{}' cannot access databases belonging to '{}'",
                        requesting_platform, target_platform
                    ),
                    database: None,
                    cause: None,
                },
            ),
            GatewayError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "internal_error".to_string(),
                    message: msg.clone(),
                    database: None,
                    cause: None,
                },
            ),
        };

        (status, Json(error_response)).into_response()
    }
}

fn format_database_name(platform: &str, tenant_id: Option<&str>) -> String {
    match tenant_id {
        Some(tid) => format!("{}_{}", platform, tid),
        None => format!("{}_main", platform),
    }
}

impl From<tokio_postgres::Error> for GatewayError {
    fn from(err: tokio_postgres::Error) -> Self {
        GatewayError::Internal(err.to_string())
    }
}

impl From<deadpool_postgres::PoolError> for GatewayError {
    fn from(err: deadpool_postgres::PoolError) -> Self {
        GatewayError::Internal(format!("Pool error: {}", err))
    }
}

impl From<std::io::Error> for GatewayError {
    fn from(err: std::io::Error) -> Self {
        GatewayError::Internal(format!("IO error: {}", err))
    }
}

impl From<anyhow::Error> for GatewayError {
    fn from(err: anyhow::Error) -> Self {
        GatewayError::Internal(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, GatewayError>;
