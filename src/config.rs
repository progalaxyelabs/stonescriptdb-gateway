use ipnetwork::IpNetwork;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub gateway_host: String,
    pub gateway_port: u16,
    pub max_connections_per_pool: u32,
    pub max_total_connections: u32,
    pub pool_idle_timeout: Duration,
    pub pool_max_lifetime: Duration,
    pub allowed_networks: Vec<IpNetwork>,
    pub data_dir: PathBuf,
    pub admin_token: Option<String>,
    pub allowed_admin_ips: Vec<IpNetwork>,
    // SMTP configuration for email sending
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from_email: Option<String>,
    pub smtp_from_name: Option<String>,
    pub email_dev_mode: bool, // If true, log emails instead of sending
    // OAuth configuration
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    // Frontend URL for email links
    pub frontend_url: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        // Build database_url from individual fields or use DATABASE_URL if provided
        let database_url = if let Ok(url) = env::var("DATABASE_URL") {
            url
        } else {
            let db_host = env::var("DB_HOST").unwrap_or_else(|_| "localhost".to_string());
            let db_port = env::var("DB_PORT").unwrap_or_else(|_| "5432".to_string());
            let db_name = env::var("DB_NAME").unwrap_or_else(|_| "postgres".to_string());
            let db_user = env::var("DB_USER").unwrap_or_else(|_| "gateway_user".to_string());
            let db_password = env::var("DB_PASSWORD").unwrap_or_else(|_| "password".to_string());

            // URL-encode password to handle special characters
            let encoded_password = urlencoding::encode(&db_password);

            format!("postgres://{}:{}@{}:{}/{}", db_user, encoded_password, db_host, db_port, db_name)
        };

        let gateway_host = env::var("GATEWAY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());

        let gateway_port = env::var("GATEWAY_PORT")
            .unwrap_or_else(|_| "9000".to_string())
            .parse()
            .unwrap_or(9000);

        let max_connections_per_pool = env::var("MAX_CONNECTIONS_PER_POOL")
            .unwrap_or_else(|_| "10".to_string())
            .parse()
            .unwrap_or(10);

        let max_total_connections = env::var("MAX_TOTAL_CONNECTIONS")
            .unwrap_or_else(|_| "200".to_string())
            .parse()
            .unwrap_or(200);

        let pool_idle_timeout_secs: u64 = env::var("POOL_IDLE_TIMEOUT_SECS")
            .unwrap_or_else(|_| "1800".to_string())
            .parse()
            .unwrap_or(1800);

        let pool_max_lifetime_secs: u64 = env::var("POOL_MAX_LIFETIME_SECS")
            .unwrap_or_else(|_| "3600".to_string())
            .parse()
            .unwrap_or(3600);

        let allowed_networks_str =
            env::var("ALLOWED_NETWORKS").unwrap_or_else(|_| "127.0.0.0/8,::1/128,192.168.0.0/16".to_string());

        let allowed_networks = allowed_networks_str
            .split(',')
            .filter_map(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    IpNetwork::from_str(trimmed).ok()
                }
            })
            .collect();

        let data_dir = env::var("DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));

        // Admin authentication (optional)
        let admin_token = env::var("ADMIN_TOKEN").ok();

        let allowed_admin_ips_str = env::var("ALLOWED_ADMIN_IPS")
            .unwrap_or_else(|_| "192.168.0.0/16".to_string());

        let allowed_admin_ips = allowed_admin_ips_str
            .split(',')
            .filter_map(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    IpNetwork::from_str(trimmed).ok()
                }
            })
            .collect();

        // SMTP configuration (optional)
        let smtp_host = env::var("SMTP_HOST").ok();
        let smtp_port = env::var("SMTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok());
        let smtp_username = env::var("SMTP_USERNAME").ok();
        let smtp_password = env::var("SMTP_PASSWORD").ok();
        let smtp_from_email = env::var("SMTP_FROM_EMAIL").ok();
        let smtp_from_name = env::var("SMTP_FROM_NAME").ok();
        let email_dev_mode = env::var("EMAIL_DEV_MODE")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        // OAuth configuration (optional)
        let google_client_id = env::var("GOOGLE_CLIENT_ID").ok();
        let google_client_secret = env::var("GOOGLE_CLIENT_SECRET").ok();

        // Frontend URL for email links
        let frontend_url = env::var("FRONTEND_URL").ok();

        Ok(Config {
            database_url,
            gateway_host,
            gateway_port,
            max_connections_per_pool,
            max_total_connections,
            pool_idle_timeout: Duration::from_secs(pool_idle_timeout_secs),
            pool_max_lifetime: Duration::from_secs(pool_max_lifetime_secs),
            allowed_networks,
            data_dir,
            admin_token,
            allowed_admin_ips,
            smtp_host,
            smtp_port,
            smtp_username,
            smtp_password,
            smtp_from_email,
            smtp_from_name,
            email_dev_mode,
            google_client_id,
            google_client_secret,
            frontend_url,
        })
    }

    pub fn socket_addr(&self) -> anyhow::Result<SocketAddr> {
        let addr = format!("{}:{}", self.gateway_host, self.gateway_port);
        addr.parse().map_err(|e| anyhow::anyhow!("Invalid socket address: {}", e))
    }
}
