use crate::pool::PoolManager;
use axum::{extract::State, Json};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

#[derive(Serialize)]
pub struct HealthResponse {
    status: String,
    postgres_connected: bool,
    active_pools: usize,
    total_connections: u32,
    uptime_seconds: u64,
}

pub async fn health_check(
    State((pool_manager, start_time)): State<(Arc<PoolManager>, Instant)>,
) -> Json<HealthResponse> {
    // Test PostgreSQL connection
    let postgres_connected = pool_manager.admin_pool().get().await.is_ok();

    Json(HealthResponse {
        status: if postgres_connected {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        postgres_connected,
        active_pools: pool_manager.active_pools(),
        total_connections: pool_manager.total_connections(),
        uptime_seconds: start_time.elapsed().as_secs(),
    })
}
