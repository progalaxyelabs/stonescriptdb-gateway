mod api;
mod config;
mod error;
mod pool;
mod schema;
mod security;

use crate::api::{
    admin_create_tenant, admin_list_databases, call_function, health_check, migrate_schema,
    register_schema,
};
use crate::config::Config;
use crate::pool::PoolManager;
use crate::security::IpFilterLayer;

use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::signal;
use tokio::time::{interval, Duration};
use tower_http::trace::TraceLayer;
use tracing::{debug, info, warn};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup log directory
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "/var/log/stonescriptdb-gateway".to_string());

    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir).unwrap_or_else(|e| {
        eprintln!("Warning: Could not create log directory {}: {}", log_dir, e);
    });

    // Create file appender with daily rotation
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "stonescriptdb-gateway.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Initialize logging - both stdout and file
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("debug,stonescriptdb_gateway=trace")),
        )
        // Console output
        .with(fmt::layer().with_target(true).with_thread_ids(true))
        // File output with JSON format for easy parsing
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(true)
                .with_ansi(false)
                .json()
                .with_writer(non_blocking),
        )
        .init();

    debug!("Logging initialized - log directory: {}", log_dir);

    // Load environment from .env file if present
    if let Err(e) = dotenvy::dotenv() {
        warn!("No .env file found or error loading it: {}", e);
    }

    // Load configuration
    let config = Config::from_env()?;
    let socket_addr = config.socket_addr()?;

    info!("Starting DB Gateway on {}", socket_addr);
    info!("Max connections per pool: {}", config.max_connections_per_pool);
    info!("Max total connections: {}", config.max_total_connections);
    info!(
        "Pool idle timeout: {:?}",
        config.pool_idle_timeout
    );
    info!("Allowed networks: {:?}", config.allowed_networks);

    // Create pool manager
    let pool_manager = Arc::new(PoolManager::new(config.clone()).await?);

    // Start time for uptime tracking
    let start_time = Instant::now();

    // Create IP filter middleware
    let ip_filter = IpFilterLayer::new(config.allowed_networks.clone());

    // Build router
    let app = Router::new()
        // Health check (no IP filter - for load balancer)
        .route("/health", get(health_check))
        // Protected routes
        .route("/register", post(register_schema))
        .route("/migrate", post(migrate_schema))
        .route("/call", post(call_function))
        .route("/admin/databases", get(admin_list_databases))
        .route("/admin/create-tenant", post(admin_create_tenant))
        .layer(ip_filter)
        .layer(TraceLayer::new_for_http())
        .with_state((pool_manager.clone(), start_time));

    // Spawn cleanup task for idle pools
    let cleanup_pool_manager = pool_manager.clone();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;
            let removed = cleanup_pool_manager.cleanup_idle_pools().await;
            if removed > 0 {
                info!("Cleanup task removed {} idle pools", removed);
            }
        }
    });

    // Create listener
    let listener = tokio::net::TcpListener::bind(&socket_addr).await?;
    info!("Server listening on {}", socket_addr);

    // Run server with graceful shutdown
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    info!("Server shutdown complete");

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Received shutdown signal");
}
