use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use ipnetwork::IpNetwork;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminAuthConfig {
    pub admin_token: Option<String>,
    pub allowed_ips: Vec<IpNetwork>,
}

impl AdminAuthConfig {
    pub fn new(admin_token: Option<String>, allowed_ips: Vec<IpNetwork>) -> Self {
        Self {
            admin_token,
            allowed_ips,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.admin_token.is_some()
    }
}

/// Admin authentication middleware
///
/// Checks:
/// 1. Admin token is configured (returns 503 if not)
/// 2. Source IP is in allowed list (returns 403 if not)
/// 3. Bearer token is valid (returns 401 if not)
pub async fn admin_auth_middleware(
    State(config): State<Arc<AdminAuthConfig>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // 1. Check if admin endpoints are enabled
    let admin_token = match &config.admin_token {
        Some(token) => token,
        None => {
            tracing::warn!("Admin endpoint accessed but ADMIN_TOKEN not configured");
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    // 2. Extract source IP
    let source_ip = extract_client_ip(&req, addr.ip());

    // 3. Verify IP is in allowed list (fast fail)
    if !is_ip_allowed(&config.allowed_ips, source_ip) {
        tracing::warn!(
            "Admin request from unauthorized IP: {} (allowed networks: {:?})",
            source_ip,
            config.allowed_ips
        );
        return Err(StatusCode::FORBIDDEN);
    }

    // 4. Extract and validate bearer token
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!("Admin request missing Authorization header from IP: {}", source_ip);
            StatusCode::UNAUTHORIZED
        })?;

    if !auth_header.starts_with("Bearer ") {
        tracing::warn!("Admin request with invalid Authorization header format from IP: {}", source_ip);
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = &auth_header[7..]; // Skip "Bearer "

    // Use constant-time comparison to prevent timing attacks
    if !constant_time_compare(token, admin_token) {
        tracing::warn!("Invalid admin token from IP: {}", source_ip);
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 5. Log successful authentication
    tracing::info!("Admin authenticated from IP: {}", source_ip);

    // 6. Store source IP in request extensions for audit logging
    req.extensions_mut().insert(source_ip);

    // 7. Proceed with request
    Ok(next.run(req).await)
}

/// Extract client IP from request
///
/// Priority:
/// 1. X-Forwarded-For header (if behind proxy/Traefik)
/// 2. X-Real-IP header
/// 3. Connection remote address
fn extract_client_ip(req: &Request, conn_ip: IpAddr) -> IpAddr {
    // Check X-Forwarded-For first (if behind proxy)
    if let Some(forwarded) = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return forwarded;
    }

    // Check X-Real-IP
    if let Some(real_ip) = req
        .headers()
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<IpAddr>().ok())
    {
        return real_ip;
    }

    // Fallback to connection IP
    conn_ip
}

/// Check if IP is in allowed list
fn is_ip_allowed(allowed_networks: &[IpNetwork], ip: IpAddr) -> bool {
    // Always allow loopback for local admin access
    if ip.is_loopback() {
        return true;
    }

    // Check against allowed networks
    for network in allowed_networks {
        if network.contains(ip) {
            return true;
        }
    }

    false
}

/// Constant-time string comparison to prevent timing attacks
fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }

    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_constant_time_compare() {
        assert!(constant_time_compare("secret123", "secret123"));
        assert!(!constant_time_compare("secret123", "secret124"));
        assert!(!constant_time_compare("short", "longer"));
        assert!(!constant_time_compare("", "nonempty"));
    }

    #[test]
    fn test_is_ip_allowed_loopback() {
        let allowed: Vec<IpNetwork> = vec![];
        assert!(is_ip_allowed(&allowed, "127.0.0.1".parse().unwrap()));
        assert!(is_ip_allowed(&allowed, "::1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_allowed_network() {
        let allowed = vec![IpNetwork::from_str("10.0.1.0/24").unwrap()];
        assert!(is_ip_allowed(&allowed, "10.0.1.5".parse().unwrap()));
        assert!(is_ip_allowed(&allowed, "10.0.1.254".parse().unwrap()));
        assert!(!is_ip_allowed(&allowed, "10.0.2.1".parse().unwrap()));
        assert!(!is_ip_allowed(&allowed, "192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_is_ip_allowed_external_denied() {
        let allowed = vec![IpNetwork::from_str("10.0.1.0/24").unwrap()];
        assert!(!is_ip_allowed(&allowed, "8.8.8.8".parse().unwrap()));
        assert!(!is_ip_allowed(&allowed, "1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn test_admin_auth_config_is_enabled() {
        let config_enabled = AdminAuthConfig::new(Some("token".to_string()), vec![]);
        assert!(config_enabled.is_enabled());

        let config_disabled = AdminAuthConfig::new(None, vec![]);
        assert!(!config_disabled.is_enabled());
    }
}
