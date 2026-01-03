mod admin;
mod call;
mod database;
mod health;
mod migrate;
mod migrate_v2;
mod platform;
mod register;

pub use admin::{admin_create_tenant, admin_list_databases};
pub use call::call_function;
pub use database::{create_database, DatabaseState};
pub use health::health_check;
pub use migrate::migrate_schema;
pub use migrate_v2::{migrate_schema_v2, MigrateV2State};
pub use platform::{
    list_databases, list_platforms, list_schemas, register_platform, register_schema as register_platform_schema,
    PlatformState,
};
pub use register::register_schema;
