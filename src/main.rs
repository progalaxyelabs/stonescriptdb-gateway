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
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,db_gateway=debug")),
        )
        .init();

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
