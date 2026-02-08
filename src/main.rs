mod api;
mod auth;
mod config;
mod email;
mod error;
mod pool;
mod registry;
mod schema;
mod security;

use crate::api::{
    accept_invite, admin_create_tenant, admin_list_databases, call_function, change_password,
    create_database, delete_oauth_connection, get_jwks, get_oauth_connections, health_check,
    invite_membership, list_databases, list_memberships, list_platforms, list_schemas, login,
    logout, migrate_schema, migrate_schema_v2, oauth_callback, oauth_initiate,
    password_reset_confirm, password_reset_request, refresh, register, register_platform,
    register_platform_schema, register_schema, select_tenant, switch_tenant, update_membership,
    DatabaseState, MigrateV2State, PlatformState,
};
use crate::auth::jwt::JwtService;
use crate::auth::oauth::OAuthService;
use crate::auth::rate_limit::{AuthRateLimiters, login_rate_limit, register_rate_limit, password_reset_rate_limit};
use crate::config::Config;
use crate::email::EmailService;
use crate::pool::PoolManager;
use crate::schema::AuditLogger;
use crate::security::{admin_auth_middleware, AdminAuthConfig, IpFilterLayer};

use axum::{
    routing::{get, post},
    Router,
};
use deadpool_postgres::Pool;
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

    // Initialize audit logging table in postgres database
    info!("Initializing audit logging table...");
    let postgres_pool = pool_manager.get_pool_by_name("postgres").await?;
    if let Err(e) = AuditLogger::ensure_audit_table(&postgres_pool).await {
        warn!("Failed to create audit table (will retry on first use): {}", e);
    } else {
        info!("Audit logging table initialized successfully");
    }

    // Create platform state for schema registry
    let platform_state = Arc::new(PlatformState::new(&config.data_dir));

    // Create database state (combines pool manager and platform state)
    let database_state = Arc::new(DatabaseState {
        pool_manager: pool_manager.clone(),
        platform_state: platform_state.clone(),
    });

    // Create migrate v2 state
    let migrate_v2_state = Arc::new(MigrateV2State {
        pool_manager: pool_manager.clone(),
        platform_state: platform_state.clone(),
    });

    // Start time for uptime tracking
    let start_time = Instant::now();

    // Create IP filter middleware
    let ip_filter = IpFilterLayer::new(config.allowed_networks.clone());

    // Create admin auth config
    let admin_auth_config = Arc::new(AdminAuthConfig::new(
        config.admin_token.clone(),
        config.allowed_admin_ips.clone(),
    ));

    // Log admin endpoint status
    if admin_auth_config.is_enabled() {
        info!("Admin endpoints enabled with token authentication");
        info!("Allowed admin IPs: {:?}", config.allowed_admin_ips);
    } else {
        warn!("Admin endpoints DISABLED - ADMIN_TOKEN not configured");
    }

    // Initialize JWT service
    let jwt_service = Arc::new(
        JwtService::new_from_env()
            .or_else(|_| {
                warn!("JWT keys not found in environment, generating new keys");
                JwtService::new_with_generated_keys()
            })
            .expect("Failed to initialize JWT service")
    );
    info!("JWT service initialized");

    // Initialize OAuth service
    let oauth_service = Arc::new(OAuthService::new());
    info!("OAuth service initialized");

    // Initialize Email service
    let email_service = Arc::new(EmailService::new(config.clone())?);
    info!("Email service initialized");

    // Initialize rate limiters for auth endpoints
    let rate_limiters = AuthRateLimiters::new();
    info!("Rate limiters initialized for auth endpoints");

    // Wrap config in Arc for sharing
    let config_arc = Arc::new(config.clone());

    // Build admin routes (protected by admin auth middleware)
    // Note: Different admin endpoints need different state types
    let admin_platforms_routes = Router::new()
        .route("/platforms", get(list_platforms))
        .with_state(platform_state.clone())
        .layer(axum::middleware::from_fn_with_state(
            admin_auth_config.clone(),
            admin_auth_middleware,
        ));

    let admin_db_routes = Router::new()
        .route("/databases", get(admin_list_databases))
        .route("/create-tenant", post(admin_create_tenant))
        .with_state((pool_manager.clone(), start_time))
        .layer(axum::middleware::from_fn_with_state(
            admin_auth_config.clone(),
            admin_auth_middleware,
        ));

    // Get postgres pool for auth endpoints
    let postgres_pool = Arc::new(pool_manager.get_pool_by_name("postgres").await?);

    // Build auth routes - split by state type
    let auth_jwks_route = Router::new()
        .route("/auth/jwks", get(get_jwks))
        .with_state(jwt_service.clone());

    let auth_register_route = Router::new()
        .route("/auth/register", post(register))
        .with_state(postgres_pool.clone())
        .layer(axum::Extension(rate_limiters.register.clone()))
        .layer(axum::middleware::from_fn(register_rate_limit));

    let auth_login_route = Router::new()
        .route("/auth/login", post(login))
        .with_state((postgres_pool.clone(), jwt_service.clone()))
        .layer(axum::Extension(rate_limiters.login.clone()))
        .layer(axum::middleware::from_fn(login_rate_limit));

    let auth_refresh_route = Router::new()
        .route("/auth/refresh", post(refresh))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let auth_logout_route = Router::new()
        .route("/auth/logout", post(logout))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let auth_select_tenant_route = Router::new()
        .route("/auth/select-tenant", post(select_tenant))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let auth_switch_tenant_route = Router::new()
        .route("/auth/switch-tenant", post(switch_tenant))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let auth_oauth_initiate_route = Router::new()
        .route("/auth/oauth/initiate", post(oauth_initiate))
        .with_state((config_arc.clone(), oauth_service.clone()));

    let auth_oauth_callback_route = Router::new()
        .route("/auth/oauth/callback", post(oauth_callback))
        .with_state((postgres_pool.clone(), jwt_service.clone(), oauth_service.clone(), config_arc.clone()));

    // Build account management routes
    let account_password_reset_request_route = Router::new()
        .route(
            "/account/password-reset/request",
            axum::routing::MethodRouter::new().post(password_reset_request)
        )
        .with_state((postgres_pool.clone(), config_arc.clone(), email_service.clone()));

    let account_password_reset_confirm_route = Router::new()
        .route("/account/password-reset/confirm", post(password_reset_confirm))
        .with_state(postgres_pool.clone());

    let account_change_password_route = Router::new()
        .route("/account/password", axum::routing::put(change_password))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let account_oauth_connections_route = Router::new()
        .route("/account/oauth-connections", get(get_oauth_connections))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let account_delete_oauth_connection_route = Router::new()
        .route("/account/oauth-connections/:provider", axum::routing::delete(delete_oauth_connection))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    // Build membership routes
    let membership_list_route = Router::new()
        .route("/memberships", get(list_memberships))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let membership_invite_route = Router::new()
        .route("/memberships/invite", post(invite_membership))
        .with_state((postgres_pool.clone(), jwt_service.clone(), config_arc.clone(), email_service.clone()));

    let membership_accept_route = Router::new()
        .route("/memberships/accept-invite", post(accept_invite))
        .with_state((postgres_pool.clone(), jwt_service.clone()));

    let membership_update_route = Router::new()
        .route("/memberships/:id", axum::routing::put(update_membership))
        .with_state((postgres_pool, jwt_service));

    // Build router with legacy and new endpoints
    let app = Router::new()
        // Health check (no IP filter - for load balancer)
        .route("/health", get(health_check))
        // Auth endpoints (no IP filter - public)
        .merge(auth_jwks_route)
        .merge(auth_register_route)
        .merge(auth_login_route)
        .merge(auth_refresh_route)
        .merge(auth_logout_route)
        .merge(auth_select_tenant_route)
        .merge(auth_switch_tenant_route)
        .merge(auth_oauth_initiate_route)
        .merge(auth_oauth_callback_route)
        // Account management endpoints (no IP filter - public with auth for protected endpoints)
        .merge(account_password_reset_request_route)
        .merge(account_password_reset_confirm_route)
        .merge(account_change_password_route)
        .merge(account_oauth_connections_route)
        .merge(account_delete_oauth_connection_route)
        // Membership endpoints (no IP filter - public with auth)
        .merge(membership_list_route)
        .merge(membership_invite_route)
        .merge(membership_accept_route)
        .merge(membership_update_route)
        // Legacy endpoints (v1 - multipart form with schema upload)
        .route("/register", post(register_schema))
        .route("/migrate", post(migrate_schema))
        .route("/call", post(call_function))
        .layer(ip_filter.clone())
        .layer(TraceLayer::new_for_http())
        .with_state((pool_manager.clone(), start_time))
        // New platform management endpoints (v2 - stored schemas)
        .nest(
            "/platform",
            Router::new()
                .route("/register", post(register_platform))
                .route("/{platform}/schema", post(register_platform_schema))
                .route("/{platform}/schemas", get(list_schemas))
                .route("/{platform}/databases", get(list_databases))
                .layer(ip_filter.clone())
                .with_state(platform_state.clone()),
        )
        // Admin endpoints (protected by admin auth + IP filter)
        .nest("/admin", admin_platforms_routes)
        .nest("/admin", admin_db_routes)
        // New database creation endpoint
        .route(
            "/database/create",
            post(create_database).with_state(database_state),
        )
        // New migrate endpoint using stored schemas
        .route(
            "/v2/migrate",
            post(migrate_schema_v2).with_state(migrate_v2_state),
        );

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
