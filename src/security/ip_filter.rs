use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::Response,
};
use ipnetwork::IpNetwork;
use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::{Layer, Service};
use tracing::warn;

#[derive(Clone)]
pub struct IpFilterLayer {
    allowed_networks: Arc<Vec<IpNetwork>>,
}

impl IpFilterLayer {
    pub fn new(allowed_networks: Vec<IpNetwork>) -> Self {
        Self {
            allowed_networks: Arc::new(allowed_networks),
        }
    }
}

impl<S> Layer<S> for IpFilterLayer {
    type Service = IpFilterService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        IpFilterService {
            inner,
            allowed_networks: self.allowed_networks.clone(),
        }
    }
}

#[derive(Clone)]
pub struct IpFilterService<S> {
    inner: S,
    allowed_networks: Arc<Vec<IpNetwork>>,
}

impl<S> Service<Request<Body>> for IpFilterService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let allowed_networks = self.allowed_networks.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Try to get client IP from ConnectInfo extension
            let client_ip = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ci| ci.0.ip());

            // Also check X-Forwarded-For header (for proxied requests)
            let forwarded_ip = req
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse::<IpAddr>().ok());

            // Prefer forwarded IP, fallback to direct connection
            let ip_to_check = forwarded_ip.or(client_ip);

            match ip_to_check {
                Some(ip) if is_allowed(&allowed_networks, ip) => {
                    // IP is allowed, proceed with request
                    inner.call(req).await
                }
                Some(ip) => {
                    // IP not allowed
                    warn!("Unauthorized access attempt from IP: {}", ip);
                    let response = Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(
                            r#"{{"error":"unauthorized","message":"Access denied for IP address: {}"}}"#,
                            ip
                        )))
                        .unwrap();
                    Ok(response)
                }
                None => {
                    // Couldn't determine IP, deny by default
                    warn!("Unauthorized access: could not determine client IP");
                    let response = Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"error":"unauthorized","message":"Could not determine client IP"}"#,
                        ))
                        .unwrap();
                    Ok(response)
                }
            }
        })
    }
}

fn is_allowed(allowed_networks: &[IpNetwork], ip: IpAddr) -> bool {
    // Always allow loopback
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_loopback_always_allowed() {
        let allowed: Vec<IpNetwork> = vec![];
        assert!(is_allowed(&allowed, "127.0.0.1".parse().unwrap()));
        assert!(is_allowed(&allowed, "::1".parse().unwrap()));
    }

    #[test]
    fn test_vnet_allowed() {
        let allowed = vec![IpNetwork::from_str("10.0.1.0/24").unwrap()];
        assert!(is_allowed(&allowed, "10.0.1.5".parse().unwrap()));
        assert!(is_allowed(&allowed, "10.0.1.254".parse().unwrap()));
        assert!(!is_allowed(&allowed, "10.0.2.1".parse().unwrap()));
        assert!(!is_allowed(&allowed, "192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_external_ip_denied() {
        let allowed = vec![IpNetwork::from_str("10.0.1.0/24").unwrap()];
        assert!(!is_allowed(&allowed, "8.8.8.8".parse().unwrap()));
        assert!(!is_allowed(&allowed, "1.1.1.1".parse().unwrap()));
    }
}
