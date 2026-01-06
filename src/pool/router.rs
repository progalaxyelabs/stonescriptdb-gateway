/// Database naming and routing logic
pub struct DatabaseRouter;

impl DatabaseRouter {
    pub fn new() -> Self {
        Self
    }

    /// Generate database name from platform and optional tenant_id
    /// - Main DB: `{platform}_main` (e.g., `myapp_main`)
    /// - Tenant DB: `{platform}_{tenant_id}` (e.g., `myapp_clinic_001`)
    pub fn database_name(&self, platform: &str, tenant_id: Option<&str>) -> String {
        let sanitized_platform = sanitize_identifier(platform);

        match tenant_id {
            Some(tid) => {
                let sanitized_tenant = sanitize_identifier(tid);
                format!("{}_{}", sanitized_platform, sanitized_tenant)
            }
            None => format!("{}_main", sanitized_platform),
        }
    }

    /// Extract platform from database name
    pub fn platform_from_database(&self, db_name: &str) -> Option<String> {
        // Split on _ and take everything before the last segment
        // e.g., "myapp_clinic_001" -> "myapp"
        // e.g., "myapp_main" -> "myapp"
        let parts: Vec<&str> = db_name.split('_').collect();
        if parts.len() >= 2 {
            // The last part is either "main" or the tenant suffix
            // The first part(s) are the platform
            Some(parts[0].to_string())
        } else {
            None
        }
    }

    /// Check if a database belongs to a platform
    pub fn belongs_to_platform(&self, db_name: &str, platform: &str) -> bool {
        let prefix = format!("{}_", sanitize_identifier(platform));
        db_name.starts_with(&prefix)
    }

    /// Determine if a database is the main database for a platform
    pub fn is_main_database(&self, db_name: &str) -> bool {
        db_name.ends_with("_main")
    }

    /// Extract tenant_id from database name (returns None for main databases)
    pub fn tenant_id_from_database(&self, db_name: &str, platform: &str) -> Option<String> {
        let prefix = format!("{}_", sanitize_identifier(platform));

        if !db_name.starts_with(&prefix) {
            return None;
        }

        let suffix = &db_name[prefix.len()..];

        if suffix == "main" {
            None
        } else {
            Some(suffix.to_string())
        }
    }

    /// Parse database type from name
    pub fn database_type(&self, db_name: &str) -> DatabaseType {
        if db_name.ends_with("_main") {
            DatabaseType::Main
        } else {
            DatabaseType::Tenant
        }
    }
}

impl Default for DatabaseRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseType {
    Main,
    Tenant,
}

/// Sanitize identifier for PostgreSQL (lowercase, alphanumeric, underscore)
fn sanitize_identifier(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == '-' || c == ' ' {
                '_'
            } else if c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_name_main() {
        let router = DatabaseRouter::new();
        assert_eq!(router.database_name("myapp", None), "myapp_main");
        assert_eq!(router.database_name("institute-app", None), "institute_app_main");
    }

    #[test]
    fn test_database_name_tenant() {
        let router = DatabaseRouter::new();
        assert_eq!(
            router.database_name("myapp", Some("clinic_001")),
            "myapp_clinic_001"
        );
        assert_eq!(
            router.database_name("myapp", Some("clinic-002")),
            "myapp_clinic_002"
        );
    }

    #[test]
    fn test_belongs_to_platform() {
        let router = DatabaseRouter::new();
        assert!(router.belongs_to_platform("myapp_main", "myapp"));
        assert!(router.belongs_to_platform("myapp_clinic_001", "myapp"));
        assert!(!router.belongs_to_platform("platformb_main", "myapp"));
    }

    #[test]
    fn test_is_main_database() {
        let router = DatabaseRouter::new();
        assert!(router.is_main_database("myapp_main"));
        assert!(!router.is_main_database("myapp_clinic_001"));
    }

    #[test]
    fn test_tenant_id_from_database() {
        let router = DatabaseRouter::new();
        assert_eq!(
            router.tenant_id_from_database("myapp_clinic_001", "myapp"),
            Some("clinic_001".to_string())
        );
        assert_eq!(
            router.tenant_id_from_database("myapp_main", "myapp"),
            None
        );
    }

    #[test]
    fn test_sanitize_identifier() {
        assert_eq!(sanitize_identifier("MedStoreApp"), "myapp");
        assert_eq!(sanitize_identifier("clinic-001"), "clinic_001");
        assert_eq!(sanitize_identifier("test app"), "test_app");
        assert_eq!(sanitize_identifier("__test__"), "test");
    }
}
