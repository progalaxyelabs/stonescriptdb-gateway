# StoneScriptDB Gateway: Declarative Schema Management

**Status:** ✅ Implemented (Core features complete)
**Last Updated:** 2026-01-05

---

## Implementation Progress

| Feature | Status | Module |
|---------|--------|--------|
| Extensions support | ✅ Done | `extensions.rs` |
| Table dependency ordering | ✅ Done | `dependency.rs` |
| Schema diff validation | ✅ Done | `diff.rs` |
| Type compatibility checking | ✅ Done | `types.rs` |
| Function checksum tracking | ✅ Done | `functions.rs` |
| Seeder handling | ✅ Done | `seeder.rs` |
| Changelog table | ✅ Done | `changelog.rs` |
| Dry-run mode | ⏳ Planned | - |
| Backup on force | ❌ Not Planned | - |

---

## Summary

Replace numbered migrations with **declarative schema management**. Platforms define the desired state in `tables/`, `functions/`, `extensions/`, and `seeders/`. The gateway computes the diff and applies only what changed.

---

## Schema Structure

```
postgresql/
├── extensions/       # PostgreSQL extensions (uuid-ossp, pgvector, etc.)
│   ├── uuid-ossp.sql
│   └── pgvector.sql
├── migrations/       # SQL migrations (ordered by dependencies, not filename)
│   ├── 001_create_users.pssql
│   ├── 002_create_orders.pssql
│   └── ...
├── functions/        # Stored functions (CREATE OR REPLACE FUNCTION)
│   ├── get_user_by_id.pssql
│   ├── insert_project.pssql
│   └── ...
└── seeders/          # Initial/reference data
    ├── roles.pssql
    └── permissions.pssql
```

**Note:** The `tables/` folder approach (pure declarative) is documented in README but the current implementation uses `migrations/` with automatic dependency ordering.

---

## How It Works

### Register (First Time)

```
Platform sends tar.gz → Gateway:
1. Install extensions (extensions/)
2. Create tables in FK-dependency order (migrations/)
3. Deploy functions (functions/)
4. Seed data into empty tables (seeders/)
```

### Migrate (Updates)

```
Platform sends tar.gz → Gateway:
1. Install new extensions
2. Run new migrations (skip already-applied)
3. Redeploy functions (checksum-based skip if unchanged)
4. Validate seeder data exists
```

---

## Current Implementation Details

### Extensions (`extensions.rs`)

Extensions are installed **before** migrations, so migrations can use extension types.

**File format:** `extensions/{extension-name}.sql`

```sql
-- extensions/uuid-ossp.sql
-- UUID generation functions
-- version: 1.1
-- schema: public
```

| Feature | Status |
|---------|--------|
| Automatic skip if installed | ✅ |
| Version pinning (`-- version:`) | ✅ |
| Custom schema (`-- schema:`) | ✅ |
| Error if not available | ✅ |

### Table Dependency Ordering (`dependency.rs`)

Tables are automatically ordered by FK dependencies using topological sort.

```sql
-- No need for 001_, 002_ prefixes
-- Gateway analyzes REFERENCES and orders automatically

CREATE TABLE users (id SERIAL PRIMARY KEY);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id)  -- Detected, orders created after users
);
```

| Feature | Status |
|---------|--------|
| FK dependency detection | ✅ |
| Topological sort | ✅ |
| Circular dependency detection | ✅ |
| Cross-migration dependency validation | ✅ |

### Schema Diff Validation (`diff.rs`, `types.rs`)

Before applying changes, the gateway classifies them:

| Change | Classification | Behavior |
|--------|----------------|----------|
| Add table | Safe | ✅ Allowed |
| Drop table | DataLoss | ❌ Blocked |
| Add nullable column | Safe | ✅ Allowed |
| Add NOT NULL without DEFAULT | DataLoss | ❌ Blocked |
| Drop column | DataLoss | ❌ Blocked |
| Widen type (INT → BIGINT) | Safe | ✅ Allowed |
| Narrow type (BIGINT → INT) | DataLoss | ❌ Blocked |
| VARCHAR(50) → VARCHAR(100) | Safe | ✅ Allowed |
| VARCHAR(100) → VARCHAR(50) | DataLoss | ❌ Blocked |
| INT → TEXT | Incompatible | ❌ Blocked |

**Type Compatibility Matrix (implemented in `types.rs`):**

| From | To | Compatibility |
|------|-----|---------------|
| INT | BIGINT | Safe (widen) |
| SMALLINT | INT | Safe (widen) |
| REAL | DOUBLE PRECISION | Safe (widen) |
| VARCHAR(N) | VARCHAR(M) where M > N | Safe (widen) |
| VARCHAR(N) | TEXT | Safe |
| TIMESTAMP | TIMESTAMPTZ | Safe |
| Any | Same type | Safe |

### Function Deployment (`functions.rs`)

Functions are tracked with checksums to avoid unnecessary redeployment.

| Scenario | Action |
|----------|--------|
| New function | CREATE FUNCTION |
| Unchanged (same checksum) | Skip |
| Body changed, same signature | CREATE OR REPLACE |
| Signature changed | DROP old + CREATE new |
| Function removed from files | DROP (orphan cleanup) |

**Checksum normalization:**
- Whitespace changes → Same checksum
- Comment changes → Same checksum
- Case changes (`BEGIN` vs `begin`) → Same checksum

Tracking table:
```sql
_stonescriptdb_gateway_functions (
    function_name TEXT,
    signature TEXT,      -- param types for overload detection
    checksum TEXT,
    deployed_at TIMESTAMPTZ
)
```

### Seeder Handling (`seeder.rs`)

| Endpoint | Behavior |
|----------|----------|
| `/register` | Run seeders only if table is empty |
| `/migrate` | Validate seeder data exists, warn if missing |

**Conflict detection:** Gateway parses INSERT to find table name and primary key for `ON CONFLICT DO NOTHING`.

### Changelog Tracking (`changelog.rs`)

All schema changes are logged to `_stonescriptdb_gateway_changelog` for audit and debugging.

**Tracked events:**
| Event | Description |
|-------|-------------|
| `migration_applied` | Migration file executed |
| `function_deployed` | Function created/updated |
| `function_dropped` | Function dropped (signature change) |
| `extension_installed` | Extension installed |
| `seeder_run` | Seeder data inserted |
| `seeder_validated` | Seeder data validated on migrate |

**Table schema:**
```sql
_stonescriptdb_gateway_changelog (
    id SERIAL PRIMARY KEY,
    change_type TEXT NOT NULL,
    object_name TEXT NOT NULL,
    change_detail JSONB,
    forced BOOLEAN DEFAULT FALSE,
    executed_at TIMESTAMPTZ DEFAULT NOW()
)
```

**Example entries:**
```sql
SELECT * FROM _stonescriptdb_gateway_changelog ORDER BY executed_at DESC LIMIT 5;

-- id | change_type         | object_name           | change_detail                    | forced | executed_at
-- 1  | migration_applied   | 3 migrations applied  | {"checksum": "batch"}            | false  | 2026-01-02 10:30:00
-- 2  | function_deployed   | 10 functions          | {"signature": "batch", ...}      | false  | 2026-01-02 10:30:01
-- 3  | seeder_run          | roles                 | {"inserted": 3, "skipped": 0}    | false  | 2026-01-02 10:30:02
```

---

## API

### POST /register

**Request:** Multipart form
- `platform`: Platform identifier
- `tenant_id`: Optional tenant
- `schema`: tar.gz file

**Response:**
```json
{
  "status": "ready",
  "database": "myapp_main",
  "extensions_installed": 2,
  "migrations_applied": 15,
  "functions_deployed": 42,
  "seeders": [
    {"table": "roles", "inserted": 3, "skipped": 0},
    {"table": "permissions", "inserted": 10, "skipped": 0}
  ],
  "execution_time_ms": 850
}
```

### POST /migrate

**Request:** Multipart form
- `platform`: Platform identifier
- `tenant_id`: Optional tenant (omit to migrate all tenants)
- `schema`: tar.gz file
- `force`: Optional, set to `true` to allow DATALOSS changes
- `dry_run`: Optional, set to `true` to preview without applying (⏳ planned)

**Response (safe changes):**
```json
{
  "status": "completed",
  "databases_updated": ["myapp_main", "myapp_tenant_001"],
  "migrations_applied": 2,
  "functions_updated": 5,
  "functions_skipped": 37,
  "execution_time_ms": 250
}
```

**Response (blocked - dataloss detected):**
```json
{
  "status": "blocked",
  "reason": "potential_dataloss",
  "schema_validation": {
    "safe_changes": [
      {"table": "users", "change": "add_column", "column": "bio", "type": "TEXT"}
    ],
    "dataloss_changes": [
      {"table": "users", "change": "drop_column", "column": "legacy_field"}
    ],
    "incompatible_changes": []
  },
  "hint": "Resubmit with force=true to proceed"
}
```

### POST /call

**Request:**
```json
{
  "platform": "myapp",
  "tenant_id": "optional",
  "function": "get_user_by_id",
  "params": [123]
}
```

**Response:**
```json
{
  "rows": [{"user_id": 123, "email": "user@example.com"}],
  "row_count": 1,
  "execution_time_ms": 2
}
```

### GET /health

```json
{
  "status": "healthy",
  "postgres_connected": true,
  "active_pools": 3,
  "total_connections": 25,
  "uptime_seconds": 86400
}
```

---

## Internal Tracking Tables

```sql
-- Track applied migrations
CREATE TABLE _stonescriptdb_gateway_migrations (
    id SERIAL PRIMARY KEY,
    migration_file TEXT NOT NULL UNIQUE,
    checksum TEXT NOT NULL,
    applied_at TIMESTAMPTZ DEFAULT NOW()
);

-- Track deployed functions
CREATE TABLE _stonescriptdb_gateway_functions (
    id SERIAL PRIMARY KEY,
    function_name TEXT NOT NULL,
    signature TEXT,
    checksum TEXT NOT NULL,
    deployed_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(function_name, signature)
);

-- Track all schema changes for audit
CREATE TABLE _stonescriptdb_gateway_changelog (
    id SERIAL PRIMARY KEY,
    change_type TEXT NOT NULL,
    object_name TEXT NOT NULL,
    change_detail JSONB,
    forced BOOLEAN DEFAULT FALSE,
    executed_at TIMESTAMPTZ DEFAULT NOW()
);
```

---

## Remaining Work

### High Priority

1. **Dry-run mode** (`dry_run=true`)
   - Return diff without applying
   - Show what would change
   - Effort: Low

2. **Wire up diff validation in /migrate endpoint**
   - `diff.rs` exists but may not be fully integrated
   - Block on dataloss unless `force=true`
   - Effort: Medium

### Lower Priority

3. **Pure declarative `tables/` folder**
   - Alternative to `migrations/`
   - Gateway generates ALTER statements from diff
   - Effort: High (most complex)

4. **Views, Triggers, Types support**
   - Additional folders: `views/`, `triggers/`, `types/`
   - Effort: Medium each

### Not Planned

5. **Backup on force**
   - Originally proposed: Create `{table}_backup_{timestamp}` before destructive changes
   - **Decision:** Not implementing - too risky
   - **Rationale:** Automatic backup creation during schema changes introduces more risk than it mitigates:
     - Backup tables can accumulate and consume disk space unexpectedly
     - Partial backup failures could leave the database in an inconsistent state
     - Restoring from auto-backups is error-prone and rarely tested
     - Teams should use proper database backup strategies (pg_dump, point-in-time recovery) instead of relying on ad-hoc table copies
   - **Recommendation:** Use `force=true` only when you have verified backups and understand the data loss implications

---

## Design Decisions

### Why migrations/ instead of pure tables/?

The current approach uses `migrations/` with automatic dependency ordering. This was chosen because:

1. **Safer** - Explicit control over schema changes
2. **Familiar** - Developers know migration patterns
3. **Incremental** - Can be enhanced to pure declarative later

Pure `tables/` is planned as an optional alternative, not a replacement.

### Why no auto-detect column renames?

Column rename detection is fragile. If you rename `user_name` → `username` AND change the type, the heuristic fails.

**Current approach:** Treat as DROP + ADD (blocked as DATALOSS). Use explicit migration if needed.

### Why checksum functions?

Redeploying 75 unchanged functions on every migrate is wasteful. Checksum comparison:
- Skips unchanged functions (no SQL executed)
- Detects signature changes (DROP + CREATE)
- Normalizes whitespace/comments to avoid false positives

---

## Migration Path from Proposal to Reality

The original proposal envisioned:
- Pure declarative `tables/` folder
- No numbered migrations

What we implemented:
- Hybrid approach: `migrations/` + automatic ordering
- All the safety features (diff, type checking, checksums)
- Extensions support (not in original proposal)

This gives us the safety benefits while maintaining compatibility with existing workflows.

---

## References

- [README.md](../README.md) - User-facing documentation
- [INTEGRATION.md](./INTEGRATION.md) - Platform integration guide
- [src/schema/](../src/schema/) - Implementation modules
