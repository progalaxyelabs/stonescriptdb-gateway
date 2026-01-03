//! Schema Registry
//!
//! Manages platform registrations and schema storage.
//! Schemas are stored on disk in the gateway's data directory.
//!
//! Directory structure:
//! ```
//! {data_dir}/{platform}/
//!   ├── platform.json       # Platform metadata
//!   ├── main_db/
//!   │   ├── extensions/
//!   │   ├── types/
//!   │   ├── tables/
//!   │   ├── functions/
//!   │   ├── seeders/
//!   │   └── migrations/
//!   ├── tenant_db/
//!   │   └── ...
//!   └── analytics_db/
//!       └── ...
//! ```

mod platform;
mod schema;

pub use platform::{PlatformRegistry, PlatformInfo};
pub use schema::{SchemaStore, StoredSchema};
