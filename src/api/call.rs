use crate::error::{GatewayError, Result};
use crate::pool::PoolManager;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

#[derive(Debug, Deserialize)]
pub struct CallRequest {
    pub platform: String,
    pub tenant_id: Option<String>,
    pub function: String,
    pub params: Vec<Value>,
}

#[derive(Serialize)]
pub struct CallResponse {
    pub rows: Vec<serde_json::Map<String, Value>>,
    pub row_count: usize,
    pub execution_time_ms: u64,
}

pub async fn call_function(
    State((pool_manager, _)): State<(Arc<PoolManager>, Instant)>,
    Json(request): Json<CallRequest>,
) -> Result<impl IntoResponse> {
    let start_time = Instant::now();

    let db_name = pool_manager.database_name(&request.platform, request.tenant_id.as_deref());

    debug!(
        "Calling function {} on database {} with {} params",
        request.function,
        db_name,
        request.params.len()
    );

    // Validate function name (prevent SQL injection)
    if !is_valid_function_name(&request.function) {
        return Err(GatewayError::InvalidRequest {
            message: format!("Invalid function name: {}", request.function),
        });
    }

    // Get connection pool
    let pool = pool_manager
        .get_pool(&request.platform, request.tenant_id.as_deref())
        .await?;

    let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
        database: db_name.clone(),
        cause: e.to_string(),
    })?;

    // Build query using JSON parameter passing for type flexibility
    // We pass all params as a single JSONB array and use jsonb_array_elements to extract them
    // This allows PostgreSQL to handle type coercion naturally

    let param_count = request.params.len();

    let rows = if param_count == 0 {
        // No parameters - simple call
        let query = format!("SELECT * FROM {}()", request.function);
        client
            .query(&query, &[])
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: db_name.clone(),
                function: request.function.clone(),
                cause: e.to_string(),
            })?
    } else {
        // Build inline SQL with properly escaped/typed values
        // This is safe because we validate the function name and use proper JSON serialization
        let param_values: Vec<String> = request
            .params
            .iter()
            .map(|v| match v {
                Value::Null => "NULL".to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                Value::String(s) => {
                    // Escape single quotes for SQL
                    let escaped = s.replace('\'', "''");
                    format!("'{}'", escaped)
                }
                Value::Array(_) | Value::Object(_) => {
                    // For complex types, pass as JSONB
                    let json_str = serde_json::to_string(v).unwrap_or_default();
                    let escaped = json_str.replace('\'', "''");
                    format!("'{}'::jsonb", escaped)
                }
            })
            .collect();

        let query = format!(
            "SELECT * FROM {}({})",
            request.function,
            param_values.join(", ")
        );

        debug!("Executing query: {}", query);

        client
            .query(&query, &[])
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: db_name.clone(),
                function: request.function.clone(),
                cause: e.to_string(),
            })?
    };

    // Convert rows to JSON
    let row_count = rows.len();
    let mut result_rows: Vec<serde_json::Map<String, Value>> = Vec::with_capacity(row_count);

    for row in rows {
        let mut map = serde_json::Map::new();

        for (i, column) in row.columns().iter().enumerate() {
            let name = column.name().to_string();
            let value = row_to_json_value(&row, i);
            map.insert(name, value);
        }

        result_rows.push(map);
    }

    let execution_time_ms = start_time.elapsed().as_millis() as u64;

    debug!(
        "Function {} returned {} rows in {}ms",
        request.function, row_count, execution_time_ms
    );

    Ok((
        StatusCode::OK,
        Json(CallResponse {
            rows: result_rows,
            row_count,
            execution_time_ms,
        }),
    ))
}

fn is_valid_function_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 63 {
        return false;
    }

    let first_char = name.chars().next().unwrap();
    if !first_char.is_ascii_lowercase() && first_char != '_' {
        return false;
    }

    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn row_to_json_value(row: &tokio_postgres::Row, idx: usize) -> Value {
    use postgres_types::Type;

    let column = &row.columns()[idx];
    let col_type = column.type_();

    // Handle NULL values
    if let Ok(opt) = row.try_get::<_, Option<String>>(idx) {
        if opt.is_none() {
            return Value::Null;
        }
    }

    // Try to get the appropriate type based on column type
    match *col_type {
        Type::BOOL => row
            .try_get::<_, bool>(idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),

        Type::INT2 => row
            .try_get::<_, i16>(idx)
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),

        Type::INT4 => row
            .try_get::<_, i32>(idx)
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),

        Type::INT8 => row
            .try_get::<_, i64>(idx)
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),

        Type::FLOAT4 => row
            .try_get::<_, f32>(idx)
            .ok()
            .and_then(|v| serde_json::Number::from_f64(v as f64).map(Value::Number))
            .unwrap_or(Value::Null),

        Type::FLOAT8 => row
            .try_get::<_, f64>(idx)
            .ok()
            .and_then(|v| serde_json::Number::from_f64(v).map(Value::Number))
            .unwrap_or(Value::Null),

        Type::JSON | Type::JSONB => row
            .try_get::<_, Value>(idx)
            .unwrap_or(Value::Null),

        Type::TIMESTAMPTZ | Type::TIMESTAMP => row
            .try_get::<_, chrono::DateTime<chrono::Utc>>(idx)
            .map(|v| Value::String(v.to_rfc3339()))
            .or_else(|_| {
                row.try_get::<_, chrono::NaiveDateTime>(idx)
                    .map(|v| Value::String(v.to_string()))
            })
            .unwrap_or(Value::Null),

        Type::DATE => row
            .try_get::<_, chrono::NaiveDate>(idx)
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null),

        Type::TIME => row
            .try_get::<_, chrono::NaiveTime>(idx)
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null),

        _ => {
            // Default: try to get as string
            row.try_get::<_, String>(idx)
                .map(Value::String)
                .unwrap_or_else(|_| {
                    // If not a string, try Option<String>
                    row.try_get::<_, Option<String>>(idx)
                        .map(|opt| opt.map(Value::String).unwrap_or(Value::Null))
                        .unwrap_or(Value::Null)
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_function_name() {
        assert!(is_valid_function_name("get_patient_by_id"));
        assert!(is_valid_function_name("list_appointments"));
        assert!(is_valid_function_name("_internal_fn"));

        assert!(!is_valid_function_name("")); // Empty
        assert!(!is_valid_function_name("DROP TABLE users; --")); // SQL injection
        assert!(!is_valid_function_name("Get_Patient")); // Contains uppercase
        assert!(!is_valid_function_name("123_fn")); // Starts with number
    }
}
