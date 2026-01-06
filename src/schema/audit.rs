use crate::error::Result;
use deadpool_postgres::Pool;
use std::net::IpAddr;
use tracing::warn;

/// Audit logger for admin actions
pub struct AuditLogger;

impl AuditLogger {
    /// Log an admin action to the audit table
    pub async fn log_admin_action(
        pool: &Pool,
        action: &str,
        source_ip: &IpAddr,
        request_path: &str,
        request_body: Option<&str>,
        response_status: u16,
    ) -> Result<()> {
        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to get pool connection for audit log: {}", e);
                return Ok(()); // Don't fail request if audit fails
            }
        };

        let result = client
            .execute(
                r#"
                INSERT INTO _gateway_admin_audit_log
                (action, source_ip, request_path, request_body, response_status)
                VALUES ($1, $2, $3, $4, $5)
                "#,
                &[
                    &action,
                    &source_ip.to_string(),
                    &request_path,
                    &request_body,
                    &(response_status as i32),
                ],
            )
            .await;

        // Log warning if audit fails, but don't fail the request
        if let Err(e) = result {
            warn!("Failed to write audit log: {} (action: {}, ip: {})", e, action, source_ip);
        }

        Ok(())
    }

    /// Ensure the audit table exists in the postgres database
    pub async fn ensure_audit_table(pool: &Pool) -> Result<()> {
        let client = pool.get().await?;

        // Create audit table
        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _gateway_admin_audit_log (
                    id SERIAL PRIMARY KEY,
                    action VARCHAR(255) NOT NULL,
                    source_ip INET NOT NULL,
                    request_path VARCHAR(255) NOT NULL,
                    request_body TEXT,
                    response_status INTEGER NOT NULL,
                    timestamp TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                &[],
            )
            .await?;

        // Create index on timestamp for efficient log queries
        client
            .execute(
                r#"
                CREATE INDEX IF NOT EXISTS idx_admin_audit_timestamp
                ON _gateway_admin_audit_log(timestamp DESC)
                "#,
                &[],
            )
            .await?;

        // Create index on source_ip for filtering
        client
            .execute(
                r#"
                CREATE INDEX IF NOT EXISTS idx_admin_audit_source_ip
                ON _gateway_admin_audit_log(source_ip)
                "#,
                &[],
            )
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_logger_exists() {
        // Basic smoke test to ensure module compiles
        let _ = AuditLogger;
    }
}
