# DEVELOPER.md — StoneScriptDB Gateway

**Read this file FIRST before starting any task on this repo.**

## Overview

Rust-based multi-tenant database gateway and schema orchestrator for PostgreSQL. Routes function calls to correct tenant databases, manages schema migrations, handles connection pooling.

## Architecture

```
src/
├── main.rs              # CLI + Axum server setup
├── config.rs            # Configuration from env vars
├── error.rs             # Error types + HTTP status mapping
├── api/
│   ├── call.rs          # Function call routing (GET/POST /:platform/:db/:function)
│   ├── migrate.rs       # Single DB migration (POST /v2/migrate)
│   ├── migrate_all.rs   # Bulk tenant migration (POST /v2/migrate-all)
│   └── ...              # Auth, admin, health endpoints
├── pool/                # Connection pool management (deadpool-postgres)
├── registry/            # Platform + schema registration
├── schema/
│   ├── diff.rs          # Schema diff + compatibility checking
│   ├── verifier.rs      # Post-migration schema verification
│   ├── column_type_exemptions.rs  # Migration-based type change exemptions
│   ├── table_deployer.rs
│   ├── function_deployer.rs
│   └── migration_runner.rs
└── security/            # IP filtering, admin auth
```

## Build & Test

```bash
# Development build
cargo build

# Run tests
cargo test

# Release build (local — for testing only)
cargo build --release

# Production build (cross-compile via Docker for GLIBC compat)
docker build --no-cache -f Dockerfile.build -t stonescriptdb-gateway-builder .
docker run --rm -v "$PWD/output:/output" stonescriptdb-gateway-builder
# Binary: output/stonescriptdb-gateway
```

## Configuration

All config via environment variables (see `.env.example`):

| Variable | Purpose |
|----------|---------|
| `PG_HOST` | PostgreSQL host |
| `PG_PORT` | PostgreSQL port (default: 5432) |
| `PG_USER` | PostgreSQL user |
| `PG_PASSWORD` | PostgreSQL password |
| `ADMIN_TOKEN` | Bearer token for admin endpoints |
| `ALLOWED_NETWORKS` | CIDR ranges for IP filtering |
| `MAX_POOL_SIZE` | Connection pool size (default: 200) |

## Key Concepts

- **Platform**: A registered application (e.g., "myapp") with its own schema
- **Schema**: SQL files (tables, functions, migrations) uploaded as tar.gz
- **Main DB**: `{platform}_main` — platform-level data
- **Tenant DB**: `{platform}_{uuid}` — per-tenant isolated database
- **Migration**: Numbered `.pgsql` files applied in order to existing databases
- **Function**: PostgreSQL stored functions, auto-deployed via `CREATE OR REPLACE`

## Schema Diff & Data Loss Prevention

The gateway diffs schema files against live DB before migrating. Changes are classified:
- **Safe**: ADD COLUMN, widen type, add index — applied automatically
- **DataLoss**: DROP TABLE, DROP COLUMN, narrow type — blocked unless `force: true`
- **Incompatible**: Impossible type casts — always blocked

To change a column type, use the gateway's exemption mechanism in a migration file:
```sql
SELECT _stonescriptdb_gateway_change_column_type('table', 'column', 'NEW_TYPE', 'migration_fn');
```

### Rename and Drop primitives

The same pattern exists for column renames and intentional drops:

```sql
-- Rename: diff collapses DropColumn(old) + AddColumn(new) to a no-op.
SELECT _stonescriptdb_gateway_rename_column('table', 'old_col', 'new_col', 'rename_helper');

-- Intentional drop: diff marks DropColumn as Safe.
SELECT _stonescriptdb_gateway_drop_column('table', 'col', 'drop_helper');
```

**Philosophy (all three primitives):** the gateway is a thin wrapper. Your helper
function does ALL the SQL work — the gateway only records the exemption so the
schema-diff checker accepts the resulting state.

**Cascade is the developer's responsibility.** If your drop needs
`DROP COLUMN ... CASCADE`, or you must drop dependent views / foreign keys /
functions first, write that SQL in your helper. The gateway does not enumerate
dependents, does not require a declared dependent list, and does not impose a
cascade policy. This is consistent with `_change_column_type` — the gateway makes
no assumptions about the SQL you run.

## Version Management

- Version in `Cargo.toml` — bump for every release
- Git tag format: `v{semver}` (e.g., `v2.5.5`)
- Never reuse tags — package registries reject duplicates

## Known Issues

- axum 0.7.9: path params use `/:param` not `/{param}`
- `migrate-all` with no tenant DBs returns HTTP 200 with empty results (v2.5.5+)
