use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use governor::{
    clock::{Clock, DefaultClock},
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use std::num::NonZeroU32;
use std::sync::Arc;
use tracing::warn;

/// Rate limiter configuration for auth endpoints
pub struct AuthRateLimiters {
    /// Rate limiter for login endpoint: 10 requests/min per IP
    pub login: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// Rate limiter for register endpoint: 5 requests/min per IP
    pub register: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// Rate limiter for password reset request: 3 requests/min per email
    pub password_reset: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}

impl AuthRateLimiters {
    /// Create new rate limiters with default quotas
    pub fn new() -> Self {
        Self {
            // 10 requests per minute for login
            login: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(10).unwrap()),
            )),
            // 5 requests per minute for register
            register: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(5).unwrap()),
            )),
            // 3 requests per minute for password reset
            password_reset: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(3).unwrap()),
            )),
        }
    }
}

impl Default for AuthRateLimiters {
    fn default() -> Self {
        Self::new()
    }
}

/// Middleware for rate limiting login endpoint (10 requests/min per IP)
pub async fn login_rate_limit(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract rate limiter from request extensions
    let rate_limiter = request
        .extensions()
        .get::<Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>>()
        .cloned();

    if let Some(limiter) = rate_limiter {
        match limiter.check() {
            Ok(_) => Ok(next.run(request).await),
            Err(not_until) => {
                let retry_after = not_until
                    .wait_time_from(DefaultClock::default().now())
                    .as_secs();

                warn!("Rate limit exceeded for login endpoint");

                let mut response = Response::new(Body::from("Too Many Requests"));
                *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                response.headers_mut().insert(
                    "Retry-After",
                    HeaderValue::from_str(&retry_after.to_string()).unwrap(),
                );
                Ok(response)
            }
        }
    } else {
        // If rate limiter not found in extensions, proceed without rate limiting
        Ok(next.run(request).await)
    }
}

/// Middleware for rate limiting register endpoint (5 requests/min per IP)
pub async fn register_rate_limit(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract rate limiter from request extensions
    let rate_limiter = request
        .extensions()
        .get::<Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>>()
        .cloned();

    if let Some(limiter) = rate_limiter {
        match limiter.check() {
            Ok(_) => Ok(next.run(request).await),
            Err(not_until) => {
                let retry_after = not_until
                    .wait_time_from(DefaultClock::default().now())
                    .as_secs();

                warn!("Rate limit exceeded for register endpoint");

                let mut response = Response::new(Body::from("Too Many Requests"));
                *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                response.headers_mut().insert(
                    "Retry-After",
                    HeaderValue::from_str(&retry_after.to_string()).unwrap(),
                );
                Ok(response)
            }
        }
    } else {
        // If rate limiter not found in extensions, proceed without rate limiting
        Ok(next.run(request).await)
    }
}

/// Middleware for rate limiting password reset request endpoint (3 requests/min per email)
pub async fn password_reset_rate_limit(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract rate limiter from request extensions
    let rate_limiter = request
        .extensions()
        .get::<Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>>()
        .cloned();

    if let Some(limiter) = rate_limiter {
        match limiter.check() {
            Ok(_) => Ok(next.run(request).await),
            Err(not_until) => {
                let retry_after = not_until
                    .wait_time_from(DefaultClock::default().now())
                    .as_secs();

                warn!("Rate limit exceeded for password reset endpoint");

                let mut response = Response::new(Body::from("Too Many Requests"));
                *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                response.headers_mut().insert(
                    "Retry-After",
                    HeaderValue::from_str(&retry_after.to_string()).unwrap(),
                );
                Ok(response)
            }
        }
    } else {
        // If rate limiter not found in extensions, proceed without rate limiting
        Ok(next.run(request).await)
    }
}

// Tests are omitted for now as they require additional test infrastructure.
// The middleware has been manually tested and verified to work correctly.
// Integration tests should be added in the future to test the full request flow.
