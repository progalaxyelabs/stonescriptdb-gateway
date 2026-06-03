//! Gated cross-DB-link capability (task #2908).
//!
//! Allows a platform migration to read a STRICTLY-SCOPED, ALLOW-LISTED slice of
//! another gateway-managed database (e.g. `progalaxyelabs_auth_main`) **without the
//! calling platform ever holding the foreign DB's credentials**. The gateway holds
//! a dedicated *privileged* connection on the platform's behalf.
//!
//! # Safety model (fail-closed by construction)
//!
//! 1. **Two credential sets, strictly isolated.** The privileged connection is built
//!    ONLY from [`Config::privileged_database_url`]. The regular [`crate::pool::PoolManager`]
//!    never sees that URL. The privileged pool is wrapped in a [`PrivilegedPool`] newtype
//!    so it cannot be passed where a regular pool is expected. Absent config ⇒ feature OFF.
//!
//! 2. **Gateway-mediated copy — no fdw, no dblink, no `CREATE EXTENSION`.** The gateway
//!    opens the privileged read-only connection to the foreign DB, runs a SELECT that the
//!    *gateway itself builds* (never platform-supplied SQL), and materialises the rows into
//!    a local staging table in the platform DB via the regular pool. The platform migration
//!    only ever reads the local staging table — it never touches the foreign DB.
//!
//! 3. **Forced platform-scoping.** Every generated SELECT carries a mandatory
//!    `WHERE <scope_column> = $1` whose value is the **authenticated caller platform**
//!    (from the request envelope), NOT anything the platform manifest can influence.
//!    Columns/table/scope are validated against the per-invocation `grants` (which come
//!    from the authorized `--allow-cross-db-link` contract, never from the bundle).
//!    A platform therefore cannot read another platform's rows, nor columns outside its grant.
//!
//! 4. **Authorization contract (for deploy-manager #2909).** A cross-DB read only runs
//!    when the request carries `cross_db_link.authorized = true` AND a matching grant.
//!    Absent/false ⇒ any manifest in the bundle is REJECTED (fail-closed), never skipped.
//!    `authorized:true` but no privileged URL configured ⇒ hard error, never a fallback
//!    to the regular credentials.

use crate::error::{GatewayError, Result};
use deadpool_postgres::{Config as PoolConfig, Pool, Runtime};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::time::Duration;
use tokio_postgres::NoTls;
use tracing::{info, warn};

/// Prefix for every gateway-managed table that lands in a platform DB.
///
/// Tables with this prefix are exempt from the schema-diff / drop-table guards
/// (see `SchemaDiffChecker::query_current_schema`, which filters
/// `NOT LIKE '_stonescriptdb_gateway_%'`). Staging tables and the per-platform
/// audit table both use it so this capability does not trip the very checker it
/// lives behind.
pub const GATEWAY_TABLE_PREFIX: &str = "_stonescriptdb_gateway_";

/// Deterministic staging-table name for a link. The platform migration reads from
/// exactly this name. The name is derived from the (validated) link identifier and
/// always carries [`GATEWAY_TABLE_PREFIX`] so it can never escape the diff exemption.
pub fn staging_table_name(link: &str) -> String {
    format!("{}xdb_{}", GATEWAY_TABLE_PREFIX, link)
}

/// Per-platform audit table (lives in the platform DB; prefix-exempt from diff).
pub const PLATFORM_AUDIT_TABLE: &str = "_stonescriptdb_gateway_cross_db_link_audit";
/// Central audit table (lives in the `postgres` admin DB, which is never diffed).
pub const CENTRAL_AUDIT_TABLE: &str = "_gateway_cross_db_link_audit";

// ───────────────────────── Contract types ─────────────────────────

/// Authorization block passed in the migrate request envelope.
///
/// This is the CONTRACT that deploy-manager (#2909) plumbs from its
/// `--allow-cross-db-link` flag. The gateway only consumes it; it does not define
/// how the flag is parsed on the deploy side.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CrossDbLinkAuthorization {
    /// Master switch. Absent or false ⇒ no cross-DB read may run (fail-closed).
    #[serde(default)]
    pub authorized: bool,
    /// Deploy reference (git tag / deploy id) recorded in the audit log.
    #[serde(default)]
    pub deploy_ref: Option<String>,
    /// The authoritative allow-list for THIS invocation. A manifest may only request
    /// a subset of a matching grant.
    #[serde(default)]
    pub grants: Vec<CrossDbGrant>,
}

/// A single authorised grant: which foreign table, which columns, and the mandatory
/// scope column the gateway will filter on.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CrossDbGrant {
    pub foreign_db: String,
    pub foreign_table: String,
    pub allowed_columns: Vec<String>,
    pub scope_column: String,
}

/// A platform-declared need, stored in the schema bundle at `cross_db/<link>.json`.
/// NEVER trusted on its own — every field is validated against an authorised grant.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CrossDbLinkManifest {
    pub link: String,
    pub foreign_db: String,
    pub foreign_table: String,
    pub columns: Vec<String>,
    pub scope_column: String,
}

// ───────────────────────── Privileged pool newtype ─────────────────────────

/// Newtype wrapper around a privileged connection pool.
///
/// The wrapper exists purely so a privileged pool can NOT be passed where a regular
/// pool is expected. The inner pool is only reachable inside this module via
/// [`PrivilegedPool::inner`]; nothing in the regular migration path can obtain one.
pub struct PrivilegedPool(Pool);

impl PrivilegedPool {
    fn inner(&self) -> &Pool {
        &self.0
    }
}

/// Builds privileged connections to foreign databases from the privileged base URL.
///
/// Construction returns `None` when no privileged URL is configured — the single
/// source of the "feature OFF by default" property.
pub struct CrossDbReader {
    privileged_base_url: String,
    max_pool_size: u32,
}

impl CrossDbReader {
    /// Build a reader from the optional privileged URL. `None` ⇒ capability disabled.
    pub fn from_privileged_url(url: Option<&str>, max_pool_size: u32) -> Option<Self> {
        url.map(|u| Self {
            privileged_base_url: u.to_string(),
            max_pool_size,
        })
    }

    /// Build a privileged pool bound to a specific foreign database by swapping the
    /// database segment of the privileged base URL. The foreign DB name must already
    /// have been validated against the grants + identifier rules by the caller.
    pub fn pool_for_db(&self, db_name: &str) -> Result<PrivilegedPool> {
        if !is_valid_identifier(db_name) {
            return Err(GatewayError::InvalidRequest {
                message: format!("Invalid foreign database name: {}", db_name),
            });
        }
        let url = swap_database_in_url(&self.privileged_base_url, db_name)?;
        let mut cfg = PoolConfig::new();
        cfg.url = Some(url);
        cfg.pool = Some(deadpool_postgres::PoolConfig {
            max_size: self.max_pool_size as usize,
            timeouts: deadpool_postgres::Timeouts {
                wait: Some(Duration::from_secs(5)),
                create: Some(Duration::from_secs(5)),
                recycle: Some(Duration::from_secs(5)),
            },
            ..Default::default()
        });
        let pool = cfg
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .map_err(|e| GatewayError::Internal(format!("Failed to create privileged pool: {}", e)))?;
        Ok(PrivilegedPool(pool))
    }
}

// ───────────────────────── Pure validation / SQL building ─────────────────────────

/// Load every `*.json` manifest from the bundle's `cross_db/` directory.
/// Missing directory ⇒ empty list (no cross-DB need declared).
pub fn load_manifests(cross_db_dir: &Path) -> Result<Vec<CrossDbLinkManifest>> {
    if !cross_db_dir.exists() {
        return Ok(Vec::new());
    }
    let mut manifests = Vec::new();
    for entry in fs::read_dir(cross_db_dir).map_err(|e| GatewayError::Internal(format!(
        "Failed to read cross_db dir: {}",
        e
    )))? {
        let entry = entry.map_err(|e| GatewayError::Internal(format!("dir entry: {}", e)))?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = fs::read_to_string(&path).map_err(|e| GatewayError::Internal(format!(
                "Failed to read manifest {:?}: {}",
                path, e
            )))?;
            let manifest: CrossDbLinkManifest = serde_json::from_str(&content).map_err(|e| {
                GatewayError::InvalidRequest {
                    message: format!("Invalid cross_db manifest {:?}: {}", path, e),
                }
            })?;
            manifests.push(manifest);
        }
    }
    manifests.sort_by(|a, b| a.link.cmp(&b.link));
    Ok(manifests)
}

/// Validate a manifest against the authorised grants and return the matching grant.
///
/// Fail-closed on every mismatch:
/// - invalid identifiers (link / db / table / columns / scope_column),
/// - no grant for the (foreign_db, foreign_table) pair,
/// - scope_column differing from the granted scope_column,
/// - any requested column outside the grant's allow-list,
/// - empty column request.
pub fn validate_manifest<'g>(
    manifest: &CrossDbLinkManifest,
    grants: &'g [CrossDbGrant],
) -> Result<&'g CrossDbGrant> {
    let reject = |msg: String| GatewayError::InvalidRequest { message: msg };

    if !is_valid_identifier(&manifest.link) {
        return Err(reject(format!("Invalid cross_db link name: {}", manifest.link)));
    }
    if !is_valid_identifier(&manifest.foreign_db) {
        return Err(reject(format!("Invalid foreign_db: {}", manifest.foreign_db)));
    }
    if !is_valid_identifier(&manifest.foreign_table) {
        return Err(reject(format!("Invalid foreign_table: {}", manifest.foreign_table)));
    }
    if !is_valid_identifier(&manifest.scope_column) {
        return Err(reject(format!("Invalid scope_column: {}", manifest.scope_column)));
    }
    if manifest.columns.is_empty() {
        return Err(reject(format!(
            "cross_db link '{}' requests no columns",
            manifest.link
        )));
    }
    for c in &manifest.columns {
        if !is_valid_identifier(c) {
            return Err(reject(format!("Invalid column '{}' in link '{}'", c, manifest.link)));
        }
    }

    // Locate the grant authorising this exact (foreign_db, foreign_table).
    let grant = grants
        .iter()
        .find(|g| g.foreign_db == manifest.foreign_db && g.foreign_table == manifest.foreign_table)
        .ok_or_else(|| {
            reject(format!(
                "cross_db link '{}' targets {}.{} which is NOT authorised by any grant",
                manifest.link, manifest.foreign_db, manifest.foreign_table
            ))
        })?;

    // The scope column is authoritative from the grant. The manifest must match it —
    // it can never choose a different (or no) scope column.
    if manifest.scope_column != grant.scope_column {
        return Err(reject(format!(
            "cross_db link '{}' scope_column '{}' does not match the authorised scope_column '{}'",
            manifest.link, manifest.scope_column, grant.scope_column
        )));
    }

    // Every requested column must be inside the grant's allow-list.
    for c in &manifest.columns {
        if !grant.allowed_columns.contains(c) {
            return Err(reject(format!(
                "cross_db link '{}' requests column '{}' not in the authorised allow-list {:?}",
                manifest.link, c, grant.allowed_columns
            )));
        }
    }

    Ok(grant)
}

/// Build the gateway-owned, strictly-scoped SELECT for a validated manifest.
///
/// Shape: `SELECT "c1"::text AS "c1", ... FROM "table" WHERE "scope_column" = $1`.
/// The caller platform is bound as `$1` at execution — it is NEVER interpolated into
/// the SQL string. All identifiers are validated and double-quoted. Values are cast to
/// text so the staging table is a robust, type-agnostic read slice.
///
/// Pre-condition: `manifest` has already passed [`validate_manifest`] against `grant`.
pub fn build_scoped_select(manifest: &CrossDbLinkManifest, grant: &CrossDbGrant) -> Result<String> {
    // Defensive re-validation — never build SQL from unvalidated identifiers.
    validate_manifest(manifest, std::slice::from_ref(grant))?;

    let select_list = manifest
        .columns
        .iter()
        .map(|c| format!("\"{}\"::text AS \"{}\"", c, c))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!(
        "SELECT {} FROM \"{}\" WHERE \"{}\" = $1",
        select_list, manifest.foreign_table, grant.scope_column
    ))
}

// ───────────────────────── Orchestration (DB-touching) ─────────────────────────

/// Outcome of materialising one link (for audit + response).
#[derive(Debug, Clone)]
pub struct MaterializedLink {
    pub link: String,
    pub foreign_db: String,
    pub foreign_table: String,
    pub columns: Vec<String>,
    pub scope_column: String,
    pub rows: usize,
}

/// Pure fail-closed gate decision, independent of any DB pool so it is unit-testable.
///
/// Returns:
/// - `Ok(false)` ⇒ nothing declared, caller should no-op.
/// - `Ok(true)`  ⇒ authorised AND privileged reader present, proceed.
/// - `Err(..)`   ⇒ fail-closed:
///   - (i) manifests present but `authorized != true`,
///   - (ii) authorised but no privileged reader configured (never falls back).
pub fn check_authorization_gates(
    manifests_len: usize,
    authz: &CrossDbLinkAuthorization,
    reader_present: bool,
) -> Result<bool> {
    if manifests_len == 0 {
        return Ok(false);
    }
    if !authz.authorized {
        return Err(GatewayError::InvalidRequest {
            message: format!(
                "{} cross_db link(s) declared but the request is NOT authorised \
                 (cross_db_link.authorized=false/absent). Re-run with --allow-cross-db-link.",
                manifests_len
            ),
        });
    }
    if !reader_present {
        return Err(GatewayError::Internal(
            "cross_db_link authorised but PRIVILEGED_DATABASE_URL is not configured on the gateway — \
             refusing to fall back to regular credentials".to_string(),
        ));
    }
    Ok(true)
}

/// Materialise every declared cross-DB link into the platform DB.
///
/// Fail-closed gates (in order):
/// 1. No manifests ⇒ no-op (nothing declared, nothing to authorise).
/// 2. Manifests present but `authz.authorized != true` ⇒ hard error.
/// 3. Authorised but no privileged reader configured ⇒ hard error (never falls back).
///
/// `caller_platform` is the authenticated platform from the request envelope and is the
/// ONLY source of the scope filter value.
#[allow(clippy::too_many_arguments)]
pub async fn materialize_links(
    manifests: &[CrossDbLinkManifest],
    authz: &CrossDbLinkAuthorization,
    reader: Option<&CrossDbReader>,
    platform_pool: &Pool,
    admin_pool: &Pool,
    platform_db: &str,
    caller_platform: &str,
) -> Result<Vec<MaterializedLink>> {
    // Gates 1–3 (pure, fail-closed).
    if !check_authorization_gates(manifests.len(), authz, reader.is_some())? {
        return Ok(Vec::new());
    }
    let reader = reader.expect("gate guarantees reader present");

    if !is_valid_identifier(caller_platform) {
        return Err(GatewayError::InvalidRequest {
            message: format!("Invalid caller platform code: {}", caller_platform),
        });
    }

    ensure_platform_audit_table(platform_pool, platform_db).await?;
    ensure_central_audit_table(admin_pool).await.ok();

    let mut results = Vec::new();

    for manifest in manifests {
        let grant = validate_manifest(manifest, &authz.grants)?;
        let sql = build_scoped_select(manifest, grant)?;

        // Privileged read against the FOREIGN db (read-only by role; gateway-built SQL).
        let priv_pool = reader.pool_for_db(&manifest.foreign_db)?;
        let client = priv_pool.inner().get().await.map_err(|e| GatewayError::ConnectionFailed {
            database: manifest.foreign_db.clone(),
            cause: e.to_string(),
        })?;
        let rows = client
            .query(&sql, &[&caller_platform])
            .await
            .map_err(|e| GatewayError::QueryFailed {
                database: manifest.foreign_db.clone(),
                function: format!("cross_db_link:{}", manifest.link),
                cause: e.to_string(),
            })?;

        // Collect the values as text (matches the `::text` projection).
        let mut values: Vec<Vec<Option<String>>> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut v = Vec::with_capacity(manifest.columns.len());
            for i in 0..manifest.columns.len() {
                v.push(row.get::<_, Option<String>>(i));
            }
            values.push(v);
        }

        // Recreate + populate the staging table in the PLATFORM db via the regular pool.
        let staging = staging_table_name(&manifest.link);
        write_staging_table(platform_pool, platform_db, &staging, &manifest.columns, &values).await?;

        info!(
            "cross_db_link '{}' for platform '{}': {} row(s) from {}.{} → {}",
            manifest.link, caller_platform, values.len(),
            manifest.foreign_db, manifest.foreign_table, staging
        );

        let materialized = MaterializedLink {
            link: manifest.link.clone(),
            foreign_db: manifest.foreign_db.clone(),
            foreign_table: manifest.foreign_table.clone(),
            columns: manifest.columns.clone(),
            scope_column: grant.scope_column.clone(),
            rows: values.len(),
        };

        write_audit(platform_pool, admin_pool, caller_platform, &materialized,
            authz.deploy_ref.as_deref()).await;

        results.push(materialized);
    }

    Ok(results)
}

/// Drop+recreate the staging table fresh each run (no stale rows leak across runs),
/// then INSERT the materialised rows. All-TEXT columns. Identifiers are validated.
async fn write_staging_table(
    platform_pool: &Pool,
    platform_db: &str,
    staging: &str,
    columns: &[String],
    values: &[Vec<Option<String>>],
) -> Result<()> {
    let client = platform_pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
        database: platform_db.to_string(),
        cause: e.to_string(),
    })?;

    let col_defs = columns
        .iter()
        .map(|c| format!("\"{}\" TEXT", c))
        .collect::<Vec<_>>()
        .join(", ");

    // Gateway-generated DDL on a gateway-prefixed (diff-exempt) table. Fresh each run.
    let ddl = format!(
        "DROP TABLE IF EXISTS \"{0}\"; CREATE TABLE \"{0}\" ({1});",
        staging, col_defs
    );
    client.batch_execute(&ddl).await.map_err(|e| GatewayError::MigrationFailed {
        database: platform_db.to_string(),
        migration: format!("cross_db staging {}", staging),
        cause: e.to_string(),
    })?;

    if values.is_empty() {
        return Ok(());
    }

    let col_list = columns
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");

    for row in values {
        let placeholders = (1..=columns.len())
            .map(|i| format!("${}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let insert = format!(
            "INSERT INTO \"{}\" ({}) VALUES ({})",
            staging, col_list, placeholders
        );
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            row.iter().map(|v| v as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
        client.execute(&insert, &params).await.map_err(|e| GatewayError::MigrationFailed {
            database: platform_db.to_string(),
            migration: format!("cross_db staging insert {}", staging),
            cause: e.to_string(),
        })?;
    }

    Ok(())
}

async fn ensure_platform_audit_table(pool: &Pool, database: &str) -> Result<()> {
    let client = pool.get().await.map_err(|e| GatewayError::ConnectionFailed {
        database: database.to_string(),
        cause: e.to_string(),
    })?;
    client
        .batch_execute(&format!(
            r#"CREATE TABLE IF NOT EXISTS "{}" (
                id SERIAL PRIMARY KEY,
                caller_platform TEXT NOT NULL,
                foreign_db TEXT NOT NULL,
                foreign_table TEXT NOT NULL,
                columns TEXT[] NOT NULL,
                scope_column TEXT NOT NULL,
                scope_value TEXT NOT NULL,
                rows_returned INTEGER NOT NULL,
                deploy_ref TEXT,
                created_at TIMESTAMPTZ DEFAULT NOW()
            );"#,
            PLATFORM_AUDIT_TABLE
        ))
        .await
        .map_err(|e| GatewayError::Internal(format!("Failed to ensure platform audit table: {}", e)))?;
    Ok(())
}

async fn ensure_central_audit_table(admin_pool: &Pool) -> Result<()> {
    let client = admin_pool.get().await?;
    client
        .batch_execute(&format!(
            r#"CREATE TABLE IF NOT EXISTS "{}" (
                id SERIAL PRIMARY KEY,
                caller_platform TEXT NOT NULL,
                foreign_db TEXT NOT NULL,
                foreign_table TEXT NOT NULL,
                columns TEXT[] NOT NULL,
                scope_column TEXT NOT NULL,
                scope_value TEXT NOT NULL,
                rows_returned INTEGER NOT NULL,
                deploy_ref TEXT,
                created_at TIMESTAMPTZ DEFAULT NOW()
            );"#,
            CENTRAL_AUDIT_TABLE
        ))
        .await
        .map_err(|e| GatewayError::Internal(format!("Failed to ensure central audit table: {}", e)))?;
    Ok(())
}

/// Best-effort audit write to both sinks. Never fails the migration (logs a warning).
async fn write_audit(
    platform_pool: &Pool,
    admin_pool: &Pool,
    caller_platform: &str,
    m: &MaterializedLink,
    deploy_ref: Option<&str>,
) {
    let insert = format!(
        r#"INSERT INTO "{{TABLE}}"
           (caller_platform, foreign_db, foreign_table, columns, scope_column,
            scope_value, rows_returned, deploy_ref)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"#
    );
    let rows = m.rows as i32;

    if let Ok(c) = platform_pool.get().await {
        let q = insert.replace("{TABLE}", PLATFORM_AUDIT_TABLE);
        if let Err(e) = c
            .execute(
                &q,
                &[
                    &caller_platform, &m.foreign_db, &m.foreign_table, &m.columns,
                    &m.scope_column, &caller_platform, &rows, &deploy_ref,
                ],
            )
            .await
        {
            warn!("cross_db_link platform audit write failed: {}", e);
        }
    }

    if let Ok(c) = admin_pool.get().await {
        let q = insert.replace("{TABLE}", CENTRAL_AUDIT_TABLE);
        if let Err(e) = c
            .execute(
                &q,
                &[
                    &caller_platform, &m.foreign_db, &m.foreign_table, &m.columns,
                    &m.scope_column, &caller_platform, &rows, &deploy_ref,
                ],
            )
            .await
        {
            warn!("cross_db_link central audit write failed: {}", e);
        }
    }
}

// ───────────────────────── helpers ─────────────────────────

/// Strict PostgreSQL identifier: starts with a lowercase letter or underscore,
/// then lowercase letters / digits / underscores. Mirrors the rules already used
/// for database names in `pool::manager`.
fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() || name.len() > 63 {
        return false;
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_lowercase() && first != '_' {
        return false;
    }
    name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Replace the database segment (after the last `/`) of a postgres URL.
fn swap_database_in_url(base_url: &str, db_name: &str) -> Result<String> {
    if let Some(last_slash) = base_url.rfind('/') {
        Ok(format!("{}{}", &base_url[..last_slash + 1], db_name))
    } else {
        Err(GatewayError::Internal(format!(
            "Invalid PRIVILEGED_DATABASE_URL format: {}",
            base_url
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> CrossDbGrant {
        CrossDbGrant {
            foreign_db: "progalaxyelabs_auth_main".to_string(),
            foreign_table: "identities".to_string(),
            allowed_columns: vec!["id".to_string(), "email".to_string()],
            scope_column: "platform_code".to_string(),
        }
    }

    fn manifest() -> CrossDbLinkManifest {
        CrossDbLinkManifest {
            link: "auth_identities".to_string(),
            foreign_db: "progalaxyelabs_auth_main".to_string(),
            foreign_table: "identities".to_string(),
            columns: vec!["id".to_string(), "email".to_string()],
            scope_column: "platform_code".to_string(),
        }
    }

    // ── staging-table naming is always diff-exempt ──
    #[test]
    fn test_staging_table_is_prefix_exempt() {
        let name = staging_table_name("auth_identities");
        assert_eq!(name, "_stonescriptdb_gateway_xdb_auth_identities");
        // Must match the diff checker's exemption pattern `_stonescriptdb_gateway_%`.
        assert!(name.starts_with(GATEWAY_TABLE_PREFIX));
        assert!(PLATFORM_AUDIT_TABLE.starts_with(GATEWAY_TABLE_PREFIX));
    }

    // ── happy path ──
    #[test]
    fn test_valid_manifest_matches_grant() {
        let g = grant();
        let grants = vec![g.clone()];
        let matched = validate_manifest(&manifest(), &grants).unwrap();
        assert_eq!(matched, &g);
    }

    #[test]
    fn test_build_scoped_select_forces_where_scope() {
        let g = grant();
        let sql = build_scoped_select(&manifest(), &g).unwrap();
        assert!(sql.contains("WHERE \"platform_code\" = $1"), "must force scope: {}", sql);
        assert!(sql.contains("\"id\"::text"), "must project requested cols: {}", sql);
        assert!(sql.contains("\"email\"::text"));
        assert!(sql.contains("FROM \"identities\""));
        // Caller value is a bound param, never interpolated.
        assert!(!sql.contains("platform_code = '"), "scope must be a bound param");
    }

    // ── (iii) column / table / scope outside grant ⇒ fail-closed ──
    #[test]
    fn test_column_outside_grant_rejected() {
        let mut m = manifest();
        m.columns.push("password_hash".to_string()); // not allow-listed
        let err = validate_manifest(&m, &[grant()]).unwrap_err();
        assert!(format!("{:?}", err).contains("password_hash"));
    }

    #[test]
    fn test_unauthorised_table_rejected() {
        let mut m = manifest();
        m.foreign_table = "secrets".to_string();
        let err = validate_manifest(&m, &[grant()]).unwrap_err();
        assert!(format!("{:?}", err).contains("NOT authorised"));
    }

    #[test]
    fn test_unauthorised_db_rejected() {
        let mut m = manifest();
        m.foreign_db = "other_platform_main".to_string();
        assert!(validate_manifest(&m, &[grant()]).is_err());
    }

    // ── (iv) malicious manifest cannot change the scope column ──
    #[test]
    fn test_scope_column_cannot_be_overridden() {
        let mut m = manifest();
        m.scope_column = "id".to_string(); // try to scope on something else
        let err = validate_manifest(&m, &[grant()]).unwrap_err();
        assert!(format!("{:?}", err).contains("scope_column"));
    }

    #[test]
    fn test_empty_columns_rejected() {
        let mut m = manifest();
        m.columns.clear();
        assert!(validate_manifest(&m, &[grant()]).is_err());
    }

    #[test]
    fn test_no_grants_rejects_everything() {
        assert!(validate_manifest(&manifest(), &[]).is_err());
    }

    // ── identifier hardening (SQL-injection attempts in manifest) ──
    #[test]
    fn test_injection_identifiers_rejected() {
        let mut m = manifest();
        m.foreign_table = "identities; DROP TABLE users".to_string();
        assert!(validate_manifest(&m, &[grant()]).is_err());

        let mut m2 = manifest();
        m2.columns = vec!["email\"; --".to_string()];
        assert!(validate_manifest(&m2, &[grant()]).is_err());
    }

    #[test]
    fn test_build_select_revalidates_defensively() {
        // A grant/manifest pair that does NOT match must not yield SQL.
        let mut m = manifest();
        m.foreign_table = "secrets".to_string();
        assert!(build_scoped_select(&m, &grant()).is_err());
    }

    // ── (ii) feature OFF by default: no privileged URL ⇒ no reader ──
    #[test]
    fn test_reader_disabled_without_url() {
        assert!(CrossDbReader::from_privileged_url(None, 5).is_none());
        assert!(CrossDbReader::from_privileged_url(Some("postgres://p:p@h:5432/postgres"), 5).is_some());
    }

    #[test]
    fn test_swap_database_in_url() {
        let u = swap_database_in_url("postgres://priv:pw@db:5432/postgres", "auth_main").unwrap();
        assert_eq!(u, "postgres://priv:pw@db:5432/auth_main");
    }

    // ── (i) declared-but-unauthorised ⇒ rejected, never skipped ──
    #[test]
    fn test_gate_rejects_declared_but_unauthorised() {
        let authz = CrossDbLinkAuthorization { authorized: false, ..Default::default() };
        let err = check_authorization_gates(1, &authz, true).unwrap_err();
        assert!(format!("{:?}", err).contains("NOT authorised"));
    }

    #[test]
    fn test_gate_absent_authz_default_is_unauthorised() {
        // serde default for a missing block ⇒ authorized=false ⇒ fail-closed.
        let authz = CrossDbLinkAuthorization::default();
        assert!(check_authorization_gates(2, &authz, true).is_err());
    }

    // ── (ii) authorised but privileged URL unconfigured ⇒ hard error, no fallback ──
    #[test]
    fn test_gate_authorised_without_privileged_reader_is_hard_error() {
        let authz = CrossDbLinkAuthorization { authorized: true, ..Default::default() };
        let err = check_authorization_gates(1, &authz, false).unwrap_err();
        let msg = format!("{:?}", err);
        assert!(msg.contains("PRIVILEGED_DATABASE_URL"));
        assert!(msg.contains("refusing to fall back"));
    }

    #[test]
    fn test_gate_no_manifests_is_noop() {
        let authz = CrossDbLinkAuthorization::default();
        assert_eq!(check_authorization_gates(0, &authz, false).unwrap(), false);
    }

    #[test]
    fn test_gate_authorised_with_reader_proceeds() {
        let authz = CrossDbLinkAuthorization { authorized: true, ..Default::default() };
        assert_eq!(check_authorization_gates(1, &authz, true).unwrap(), true);
    }

    #[test]
    fn test_load_manifests_missing_dir_is_empty() {
        let dir = std::path::Path::new("/nonexistent/cross_db/dir/xyz");
        assert!(load_manifests(dir).unwrap().is_empty());
    }

    #[test]
    fn test_load_manifests_parses_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("cross_db");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("auth_identities.json"),
            r#"{"link":"auth_identities","foreign_db":"progalaxyelabs_auth_main",
                "foreign_table":"identities","columns":["id","email"],
                "scope_column":"platform_code"}"#,
        )
        .unwrap();
        let manifests = load_manifests(&dir).unwrap();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0], manifest());
    }
}
