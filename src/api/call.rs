use crate::error::{extract_db_error, GatewayError, Result};
use crate::pool::PoolManager;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};

// Cache for function param types to avoid per-call DB lookups.
// Key: "{db_name}:{function_name}", Value: (param_types, cached_at)
static PARAM_TYPE_CACHE: LazyLock<DashMap<String, (Vec<String>, Instant)>> =
    LazyLock::new(DashMap::new);

const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

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
            .map_err(|e| {
                let detailed_error = extract_db_error(&e);
                error!(
                    "Function call failed: {} on {} - {}",
                    request.function, db_name, detailed_error
                );
                GatewayError::QueryFailed {
                    database: db_name.clone(),
                    function: request.function.clone(),
                    cause: detailed_error,
                }
            })?
    } else {
        // Look up registered param types for type-aware casting.
        // Cache key includes db_name to avoid cross-platform collisions.
        let cache_key = format!("{}:{}", db_name, request.function);
        let param_types: Option<Vec<String>> = {
            let cached = PARAM_TYPE_CACHE
                .get(&cache_key)
                .and_then(|entry| {
                    if entry.1.elapsed() < CACHE_TTL {
                        Some(entry.0.clone())
                    } else {
                        None
                    }
                });

            if let Some(types) = cached {
                Some(types)
            } else {
                // Query the gateway functions registry.
                match client
                    .query_opt(
                        "SELECT param_types FROM _stonescriptdb_gateway_functions \
                         WHERE function_name = $1 LIMIT 1",
                        &[&request.function],
                    )
                    .await
                {
                    Ok(Some(row)) => {
                        let types: Vec<String> = row.get(0);
                        PARAM_TYPE_CACHE.insert(cache_key, (types.clone(), Instant::now()));
                        Some(types)
                    }
                    Ok(None) => {
                        warn!(
                            "No type registry entry for function '{}' in database '{}'. \
                             Parameters will use naive type inference. \
                             Run migrate to update the function registry.",
                            request.function, db_name
                        );
                        None
                    }
                    Err(e) => {
                        warn!(
                            "Type lookup failed for function '{}' in database '{}': {}. \
                             Parameters will use naive type inference. \
                             Run migrate to update the function registry.",
                            request.function, db_name, e
                        );
                        None
                    }
                }
            }
        };

        // Build inline SQL with properly escaped/typed values.
        // This is safe because we validate the function name and use proper JSON serialization.
        let param_values: Vec<String> = request
            .params
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let registered_type = param_types
                    .as_ref()
                    .and_then(|types| types.get(i))
                    .map(|t| t.as_str());

                match v {
                    Value::Null => "NULL".to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => {
                        // Escape single quotes for SQL
                        let escaped = s.replace('\'', "''");
                        match registered_type {
                            // Cast non-text types explicitly (e.g. timestamptz, uuid)
                            Some(t) if t != "text" && t != "varchar" && !t.is_empty() => {
                                format!("'{}'::{}", escaped, t)
                            }
                            _ => format!("'{}'", escaped),
                        }
                    }
                    Value::Array(arr) => {
                        match registered_type {
                            Some("bytea") => {
                                // JSON array of u8 values → PostgreSQL bytea hex literal
                                let bytes: Vec<u8> = arr
                                    .iter()
                                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                                    .collect();
                                let hex = bytes
                                    .iter()
                                    .map(|b| format!("{:02x}", b))
                                    .collect::<String>();
                                format!("'\\x{}'::bytea", hex)
                            }
                            Some(t) => {
                                // Registered array type (text[], integer[], etc.) or other type
                                let json_str = serde_json::to_string(v).unwrap_or_default();
                                let escaped = json_str.replace('\'', "''");
                                format!("'{}'::{}", escaped, t)
                            }
                            None => {
                                // Fallback: jsonb (existing behavior for unregistered functions)
                                let json_str = serde_json::to_string(v).unwrap_or_default();
                                let escaped = json_str.replace('\'', "''");
                                format!("'{}'::jsonb", escaped)
                            }
                        }
                    }
                    Value::Object(_) => {
                        let json_str = serde_json::to_string(v).unwrap_or_default();
                        let escaped = json_str.replace('\'', "''");
                        match registered_type {
                            Some(t) => format!("'{}'::{}", escaped, t),
                            None => format!("'{}'::jsonb", escaped),
                        }
                    }
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
            .map_err(|e| {
                let detailed_error = extract_db_error(&e);
                error!(
                    "Function call failed: {} on {} - Query: {} - Error: {}",
                    request.function, db_name, query, detailed_error
                );
                GatewayError::QueryFailed {
                    database: db_name.clone(),
                    function: request.function.clone(),
                    cause: detailed_error,
                }
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

    // Try to get the appropriate type based on column type
    // Each branch uses Option<T> to handle NULLs natively
    match *col_type {
        Type::BOOL => row
            .try_get::<_, Option<bool>>(idx)
            .ok()
            .flatten()
            .map(Value::Bool)
            .unwrap_or(Value::Null),

        Type::INT2 => row
            .try_get::<_, Option<i16>>(idx)
            .ok()
            .flatten()
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),

        Type::INT4 => row
            .try_get::<_, Option<i32>>(idx)
            .ok()
            .flatten()
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),

        Type::INT8 => row
            .try_get::<_, Option<i64>>(idx)
            .ok()
            .flatten()
            .map(|v| Value::Number(v.into()))
            .unwrap_or(Value::Null),

        Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(idx)
            .ok()
            .flatten()
            .and_then(|v| serde_json::Number::from_f64(v as f64).map(Value::Number))
            .unwrap_or(Value::Null),

        Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(idx)
            .ok()
            .flatten()
            .and_then(|v| serde_json::Number::from_f64(v).map(Value::Number))
            .unwrap_or(Value::Null),

        Type::NUMERIC => row
            .try_get::<_, Option<rust_decimal::Decimal>>(idx)
            .ok()
            .flatten()
            .and_then(|v| {
                use rust_decimal::prelude::ToPrimitive;
                v.to_f64().and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            })
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

    #[test]
    fn test_bytea_hex_encoding() {
        // Simulate what the bytea casting code does for [72, 101, 108, 108, 111] → "Hello"
        let bytes: Vec<u8> = vec![72, 101, 108, 108, 111];
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "48656c6c6f");
        assert_eq!(format!("'\\x{}'::bytea", hex), "'\\x48656c6c6f'::bytea");
    }
}
