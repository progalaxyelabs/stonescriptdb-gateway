use crate::config::Config;
use crate::error::GatewayError;
use lettre::message::{header::ContentType, Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use std::str::FromStr;
use tracing::{error, info};

/// Email service for sending invites, password resets, and email verification
pub struct EmailService {
    config: Config,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
}

impl EmailService {
    /// Create a new email service from configuration
    pub fn new(config: Config) -> Result<Self, GatewayError> {
        let transport = if !config.email_dev_mode {
            // Only create SMTP transport if not in dev mode
            if let (Some(host), Some(port), Some(username), Some(password)) = (
                &config.smtp_host,
                config.smtp_port,
                &config.smtp_username,
                &config.smtp_password,
            ) {
                let creds = Credentials::new(username.clone(), password.clone());

                let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(host)
                    .map_err(|e| GatewayError::Internal(format!("Failed to create SMTP transport: {}", e)))?
                    .port(port)
                    .credentials(creds)
                    .build();

                Some(transport)
            } else {
                info!("SMTP configuration incomplete - emails will be logged instead of sent");
                None
            }
        } else {
            None
        };

        Ok(Self { config, transport })
    }

    /// Get the "from" mailbox for emails
    fn from_mailbox(&self) -> Result<Mailbox, GatewayError> {
        let email = self
            .config
            .smtp_from_email
            .as_ref()
            .ok_or_else(|| GatewayError::Internal("SMTP_FROM_EMAIL not configured".to_string()))?;

        let name = self.config.smtp_from_name.as_deref();

        if let Some(name) = name {
            Mailbox::from_str(&format!("{} <{}>", name, email))
                .map_err(|e| GatewayError::Internal(format!("Invalid from address: {}", e)))
        } else {
            Mailbox::from_str(email)
                .map_err(|e| GatewayError::Internal(format!("Invalid from address: {}", e)))
        }
    }

    /// Send an email message (or log it in dev mode)
    async fn send_email(&self, to: &str, subject: &str, body: &str, message: Message) -> Result<(), GatewayError> {
        if self.config.email_dev_mode || self.transport.is_none() {
            // Dev mode: log the email instead of sending
            info!("📧 [DEV MODE] Email would be sent:");
            info!("  To: {}", to);
            info!("  Subject: {}", subject);
            info!("  Body:\n{}", body);
            Ok(())
        } else {
            // Production mode: actually send the email
            let transport = self.transport.as_ref()
                .ok_or_else(|| GatewayError::Internal("SMTP transport not configured".to_string()))?;

            transport
                .send(message)
                .await
                .map_err(|e| {
                    error!("Failed to send email: {}", e);
                    GatewayError::Internal(format!("Failed to send email: {}", e))
                })?;

            info!("📧 Email sent successfully to {}", to);
            Ok(())
        }
    }

    /// Send an invitation email with a link to join a tenant
    ///
    /// # Arguments
    /// * `to` - Recipient email address
    /// * `invite_link` - Full URL to accept the invitation
    /// * `tenant_name` - Name of the tenant being invited to
    /// * `inviter_name` - Name of the person who sent the invitation
    pub async fn invitation_email(
        &self,
        to: &str,
        invite_link: &str,
        tenant_name: &str,
        inviter_name: &str,
    ) -> Result<(), GatewayError> {
        let from = self.from_mailbox()?;
        let to_mailbox = Mailbox::from_str(to)
            .map_err(|e| GatewayError::Internal(format!("Invalid recipient address: {}", e)))?;

        let subject = format!("You've been invited to join {}", tenant_name);

        let body = format!(
            r#"Hello,

{} has invited you to join the {} workspace on StoneScriptDB Gateway.

Click the link below to accept your invitation:

{}

This invitation link will expire after 7 days.

If you didn't expect this invitation, you can safely ignore this email.

---
StoneScriptDB Gateway
"#,
            inviter_name, tenant_name, invite_link
        );

        let message = Message::builder()
            .from(from)
            .to(to_mailbox)
            .subject(subject.as_str())
            .header(ContentType::TEXT_PLAIN)
            .body(body.clone())
            .map_err(|e| GatewayError::Internal(format!("Failed to build email: {}", e)))?;

        self.send_email(to, &subject, &body, message).await
    }

    /// Send a password reset email with a secure reset link
    ///
    /// # Arguments
    /// * `to` - Recipient email address
    /// * `reset_link` - Full URL to reset the password
    pub async fn password_reset_email(
        &self,
        to: &str,
        reset_link: &str,
    ) -> Result<(), GatewayError> {
        let from = self.from_mailbox()?;
        let to_mailbox = Mailbox::from_str(to)
            .map_err(|e| GatewayError::Internal(format!("Invalid recipient address: {}", e)))?;

        let subject = "Password Reset Request";

        let body = format!(
            r#"Hello,

We received a request to reset your password for your StoneScriptDB Gateway account.

Click the link below to reset your password:

{}

This link will expire after 1 hour.

If you didn't request a password reset, you can safely ignore this email. Your password will remain unchanged.

---
StoneScriptDB Gateway
"#,
            reset_link
        );

        let message = Message::builder()
            .from(from)
            .to(to_mailbox)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.clone())
            .map_err(|e| GatewayError::Internal(format!("Failed to build email: {}", e)))?;

        self.send_email(to, subject, &body, message).await
    }

    /// Send an email verification link to confirm the user's email address
    ///
    /// # Arguments
    /// * `to` - Recipient email address
    /// * `verify_link` - Full URL to verify the email address
    pub async fn email_verification(
        &self,
        to: &str,
        verify_link: &str,
    ) -> Result<(), GatewayError> {
        let from = self.from_mailbox()?;
        let to_mailbox = Mailbox::from_str(to)
            .map_err(|e| GatewayError::Internal(format!("Invalid recipient address: {}", e)))?;

        let subject = "Verify your email address";

        let body = format!(
            r#"Hello,

Thank you for signing up with StoneScriptDB Gateway!

Please verify your email address by clicking the link below:

{}

This verification link will expire after 24 hours.

If you didn't create an account with us, you can safely ignore this email.

---
StoneScriptDB Gateway
"#,
            verify_link
        );

        let message = Message::builder()
            .from(from)
            .to(to_mailbox)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.clone())
            .map_err(|e| GatewayError::Internal(format!("Failed to build email: {}", e)))?;

        self.send_email(to, subject, &body, message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_email_service_dev_mode() {
        // Create a config in dev mode
        let mut config = Config {
            database_url: "postgres://test".to_string(),
            gateway_host: "127.0.0.1".to_string(),
            gateway_port: 9000,
            max_connections_per_pool: 10,
            max_total_connections: 100,
            pool_idle_timeout: std::time::Duration::from_secs(1800),
            pool_max_lifetime: std::time::Duration::from_secs(3600),
            allowed_networks: vec![],
            data_dir: std::path::PathBuf::from("./data"),
            admin_token: None,
            allowed_admin_ips: vec![],
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_from_email: Some("test@example.com".to_string()),
            smtp_from_name: Some("Test Gateway".to_string()),
            email_dev_mode: true,
        };

        let service = EmailService::new(config).unwrap();

        // These should succeed in dev mode without actual SMTP
        service
            .invitation_email(
                "user@example.com",
                "https://example.com/invite/abc123",
                "Test Tenant",
                "John Doe",
            )
            .await
            .unwrap();

        service
            .password_reset_email("user@example.com", "https://example.com/reset/xyz789")
            .await
            .unwrap();

        service
            .email_verification("user@example.com", "https://example.com/verify/qwe456")
            .await
            .unwrap();
    }
}
