use crate::config::Config;
use crate::error::{GatewayError, Result};
use crate::pool::router::DatabaseRouter;
use dashmap::DashMap;
use deadpool_postgres::{Config as PoolConfig, Pool, Runtime};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_postgres::NoTls;
use tracing::{debug, info};

struct PoolEntry {
    pool: Pool,
    last_used: RwLock<Instant>,
}

pub struct PoolManager {
    pools: DashMap<String, Arc<PoolEntry>>,
    router: DatabaseRouter,
    config: Config,
    total_connections: AtomicU32,
    admin_pool: Pool,
}

impl PoolManager {
    pub async fn new(config: Config) -> Result<Self> {
        // Create admin pool for connecting to the main postgres database
        let admin_pool = create_pool(&config.database_url, config.max_connections_per_pool)?;

        // Test admin connection
        let client = admin_pool.get().await.map_err(|e| {
            GatewayError::ConnectionFailed {
                database: "postgres (admin)".to_string(),
                cause: e.to_string(),
            }
        })?;

        // Simple ping query
        client.execute("SELECT 1", &[]).await.map_err(|e| {
            GatewayError::ConnectionFailed {
                database: "postgres (admin)".to_string(),
                cause: format!("Ping failed: {}", e),
            }
        })?;

        info!("Connected to PostgreSQL admin database");

        Ok(Self {
            pools: DashMap::new(),
            router: DatabaseRouter::new(),
            config,
            total_connections: AtomicU32::new(0),
            admin_pool,
        })
    }

    pub fn admin_pool(&self) -> &Pool {
        &self.admin_pool
    }

    pub async fn get_pool(&self, platform: &str, tenant_id: Option<&str>) -> Result<Pool> {
        let db_name = self.router.database_name(platform, tenant_id);

        // Check if pool already exists
        if let Some(entry) = self.pools.get(&db_name) {
            *entry.last_used.write().await = Instant::now();
            return Ok(entry.pool.clone());
        }

        // Create new pool
        self.create_pool_for_database(&db_name).await
    }

    pub async fn get_pool_by_name(&self, db_name: &str) -> Result<Pool> {
        // Check if pool already exists
        if let Some(entry) = self.pools.get(db_name) {
            *entry.last_used.write().await = Instant::now();
            return Ok(entry.pool.clone());
        }

        // Create new pool
        self.create_pool_for_database(db_name).await
    }

    async fn create_pool_for_database(&self, db_name: &str) -> Result<Pool> {
        // Check if we'd exceed max connections
        let current = self.total_connections.load(Ordering::Relaxed);
        if current + self.config.max_connections_per_pool > self.config.max_total_connections {
            // Try to evict an old pool first
            self.evict_lru_pool().await?;
        }

        // Build database URL for this specific database
        let db_url = self.database_url_for(db_name)?;

        let pool = create_pool(&db_url, self.config.max_connections_per_pool)?;

        // Test the connection
        let _ = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: db_name.to_string(),
            cause: e.to_string(),
        })?;

        let entry = Arc::new(PoolEntry {
            pool: pool.clone(),
            last_used: RwLock::new(Instant::now()),
        });

        self.pools.insert(db_name.to_string(), entry);
        self.total_connections
            .fetch_add(self.config.max_connections_per_pool, Ordering::Relaxed);

        info!("Created pool for database: {}", db_name);

        Ok(pool)
    }

    fn database_url_for(&self, db_name: &str) -> Result<String> {
        // Parse base URL and replace database name
        let base_url = &self.config.database_url;

        // Find the last '/' and replace everything after it
        if let Some(last_slash) = base_url.rfind('/') {
            let base = &base_url[..last_slash + 1];
            Ok(format!("{}{}", base, db_name))
        } else {
            Err(GatewayError::Internal(format!(
                "Invalid DATABASE_URL format: {}",
                base_url
            )))
        }
    }

    async fn evict_lru_pool(&self) -> Result<()> {
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = Instant::now();

        // Find the least recently used pool
        for entry in self.pools.iter() {
            let last_used = *entry.value().last_used.read().await;
            if last_used < oldest_time {
                oldest_time = last_used;
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            if let Some((_, _removed)) = self.pools.remove(&key) {
                self.total_connections
                    .fetch_sub(self.config.max_connections_per_pool, Ordering::Relaxed);
                info!("Evicted pool for database: {} (idle since {:?} ago)", key, oldest_time.elapsed());
            }
        }

        Ok(())
    }

    pub async fn cleanup_idle_pools(&self) -> usize {
        let idle_timeout = self.config.pool_idle_timeout;
        let now = Instant::now();
        let mut removed = 0;

        let mut to_remove = Vec::new();

        for entry in self.pools.iter() {
            let last_used = *entry.value().last_used.read().await;
            if now.duration_since(last_used) > idle_timeout {
                to_remove.push(entry.key().clone());
            }
        }

        for key in to_remove {
            if let Some((_, _)) = self.pools.remove(&key) {
                self.total_connections
                    .fetch_sub(self.config.max_connections_per_pool, Ordering::Relaxed);
                removed += 1;
                debug!("Cleaned up idle pool for database: {}", key);
            }
        }

        if removed > 0 {
            info!("Cleaned up {} idle pools", removed);
        }

        removed
    }

    pub async fn database_exists(&self, db_name: &str) -> Result<bool> {
        let client = self.admin_pool.get().await.map_err(|e| {
            GatewayError::ConnectionFailed {
                database: "postgres (admin)".to_string(),
                cause: e.to_string(),
            }
        })?;

        let row = client
            .query_opt(
                "SELECT 1 FROM pg_database WHERE datname = $1",
                &[&db_name],
            )
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        Ok(row.is_some())
    }

    pub async fn create_database(&self, db_name: &str) -> Result<()> {
        let client = self.admin_pool.get().await.map_err(|e| {
            GatewayError::ConnectionFailed {
                database: "postgres (admin)".to_string(),
                cause: e.to_string(),
            }
        })?;

        // Check if already exists
        if self.database_exists(db_name).await? {
            debug!("Database {} already exists", db_name);
            return Ok(());
        }

        // Create database (note: can't use parameters for DDL)
        // Validate db_name to prevent SQL injection
        if !is_valid_identifier(db_name) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Invalid database name: {}", db_name),
            });
        }

        let sql = format!("CREATE DATABASE \"{}\"", db_name);
        client
            .batch_execute(&sql)
            .await
            .map_err(|e| GatewayError::Internal(format!("Failed to create database: {}", e)))?;

        info!("Created database: {}", db_name);
        Ok(())
    }

    pub async fn list_databases_for_platform(&self, platform: &str) -> Result<Vec<String>> {
        let client = self.admin_pool.get().await.map_err(|e| {
            GatewayError::ConnectionFailed {
                database: "postgres (admin)".to_string(),
                cause: e.to_string(),
            }
        })?;

        let prefix = format!("{}_", platform);
        let rows = client
            .query(
                "SELECT datname FROM pg_database WHERE datname LIKE $1 ORDER BY datname",
                &[&format!("{}%", prefix)],
            )
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        Ok(rows.iter().map(|row| row.get(0)).collect())
    }

    pub fn active_pools(&self) -> usize {
        self.pools.len()
    }

    pub fn total_connections(&self) -> u32 {
        self.total_connections.load(Ordering::Relaxed)
    }

    pub fn database_name(&self, platform: &str, tenant_id: Option<&str>) -> String {
        self.router.database_name(platform, tenant_id)
    }

    pub async fn get_database_size(&self, db_name: &str) -> Result<i64> {
        let pool = self.get_pool_by_name(db_name).await?;
        let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: db_name.to_string(),
            cause: e.to_string(),
        })?;

        let row = client
            .query_one("SELECT pg_database_size(current_database())", &[])
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        Ok(row.get(0))
    }
}

fn create_pool(database_url: &str, max_size: u32) -> Result<Pool> {
    let mut cfg = PoolConfig::new();
    cfg.url = Some(database_url.to_string());

    cfg.pool = Some(deadpool_postgres::PoolConfig {
        max_size: max_size as usize,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(Duration::from_secs(5)),
            create: Some(Duration::from_secs(5)),
            recycle: Some(Duration::from_secs(5)),
        },
        ..Default::default()
    });

    cfg.create_pool(Some(Runtime::Tokio1), NoTls)
        .map_err(|e| GatewayError::Internal(format!("Failed to create pool: {}", e)))
}

fn is_valid_identifier(name: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_identifier() {
        assert!(is_valid_identifier("medstoreapp_main"));
        assert!(is_valid_identifier("medstoreapp_clinic_001"));
        assert!(is_valid_identifier("_test"));

        assert!(!is_valid_identifier("")); // Empty
        assert!(!is_valid_identifier("DROP TABLE")); // SQL injection attempt
        assert!(!is_valid_identifier("1_test")); // Starts with number
        assert!(!is_valid_identifier("Test_DB")); // Contains uppercase
    }
}
