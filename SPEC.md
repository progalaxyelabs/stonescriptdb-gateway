# StoneScriptDB Gateway — User-Facing Contract Specification

This document describes the complete user-facing contract for the StoneScriptDB Gateway: all HTTP endpoints, request/response shapes, error codes, migration behavior, function call mechanics, and tenancy architecture. It is the authoritative reference for consumers building against the gateway.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Authentication & Security](#2-authentication--security)
3. [Request / Response Format](#3-request--response-format)
4. [Error Codes](#4-error-codes)
5. [Function Calls](#5-function-calls)
6. [Platform & Schema Management (V2)](#6-platform--schema-management-v2)
7. [Database Creation](#7-database-creation)
8. [Migration](#8-migration)
9. [Tenancy Architecture](#9-tenancy-architecture)
10. [Auth Endpoints](#10-auth-endpoints)
11. [Admin Endpoints](#11-admin-endpoints)
12. [Health Check](#12-health-check)
13. [Internal Tracking Tables](#13-internal-tracking-tables)
14. [Configuration Reference](#14-configuration-reference)
15. [Connection Pooling](#15-connection-pooling)

---

## 1. Overview

The StoneScriptDB Gateway is an HTTP proxy that:
- Stores PostgreSQL schemas (tables, functions, migrations, types, extensions, seeders)
- Provisions new tenant databases from stored schemas
- Migrates existing databases when schemas change
- Routes function call requests to the correct database connection pool

**Base URL**: configurable via `GATEWAY_HOST` + `GATEWAY_PORT` (default: `http://127.0.0.1:9000`)

**All requests/responses use JSON** except schema upload endpoints, which use `multipart/form-data`.

---

## 2. Authentication & Security

### IP Filter (Database Endpoints)
The following endpoints are protected by an IP allowlist (`ALLOWED_NETWORKS`):
- `POST /call`
- `POST /platform/*`
- `POST /v2/migrate`
- `POST /v2/migrate-all`

Default allowed networks: `127.0.0.0/8`, `::1/128`, `192.168.0.0/16`

Requests from unlisted IPs receive:
```json
HTTP 403
{
  "error": "unauthorized",
  "message": "Access denied for IP address: <ip>"
}
```

### Admin Token (Admin Endpoints)
Admin endpoints (`/admin/*`, `/admin/database/*`) require an `Authorization: Bearer <ADMIN_TOKEN>` header. If `ADMIN_TOKEN` is not configured, all admin endpoints return 503.

### Public Endpoints (No Auth Required)
- `GET /health`
- `POST /auth/*` — user-facing auth endpoints
- `POST /account/*` — account management

---

## 3. Request / Response Format

### Content Types
| Endpoint type | Request | Response |
|---|---|---|
| All standard endpoints | `application/json` | `application/json` |
| Schema upload (`POST /platform/{platform}/schema`) | `multipart/form-data` | `application/json` |

### HTTP Status Codes

| Code | Meaning |
|---|---|
| 200 | Success (GET, function call, migrate) |
| 201 | Created (register, create database) |
| 400 | Bad request (invalid input) |
| 401 | Unauthorized (missing/invalid auth token) |
| 403 | Forbidden (IP blocked, platform isolation violation) |
| 404 | Not found (database/platform not found) |
| 409 | Conflict (database or platform already exists) |
| 500 | Internal server error (migration failed, function error) |
| 503 | Service unavailable (connection failed, pool exhausted) |

### Success Response Shape
Each endpoint has its own success response (documented per-endpoint below).

### Error Response Shape
All errors return a consistent JSON body:

```json
{
  "error": "error_code",
  "message": "Human-readable description",
  "database": "database_name",   // optional — included when error is DB-specific
  "cause": "detailed cause"      // optional — PostgreSQL error detail, HINT, position, etc.
}
```

---

## 4. Error Codes

| `error` field | HTTP Status | Description |
|---|---|---|
| `database_not_found` | 404 | Platform/tenant combination has no database |
| `database_already_exists` | 409 | Attempted to create a database that already exists |
| `platform_already_registered` | 409 | Platform name already registered |
| `migration_failed` | 500 | A migration file or schema step failed |
| `function_deploy_failed` | 500 | A PostgreSQL function could not be deployed |
| `query_failed` | 500 | Function call execution failed |
| `extension_not_available` | 400 | Required PostgreSQL extension not installed on the server |
| `extension_install_failed` | 500 | Extension was found but could not be installed |
| `schema_extraction_failed` | 400 | Uploaded `tar.gz` could not be extracted |
| `connection_failed` | 503 | Could not open a connection to a database |
| `pool_exhausted` | 503 | Connection pool for a database is at capacity |
| `unauthorized` | 403 | Request IP not in `ALLOWED_NETWORKS` |
| `invalid_request` | 400 | Missing/invalid field or precondition not met |
| `platform_isolation_violation` | 403 | Function call targeted a database that belongs to a different platform |
| `signature_verification_failed` | 401 | HMAC signature check failed |
| `timestamp_expired` | 401 | Request timestamp is too old (replay protection) |
| `unauthorized_function` | 403 | Client not permitted to call that function |
| `invalid_client_id` | 401 | Unknown client ID in request |
| `internal_error` | 500 | Unexpected internal error |

---

## 5. Function Calls

### Endpoint

```
POST /call
```

Protected by IP filter. No auth token required (IP allowlist is the security boundary).

### Request

```json
{
  "platform": "myapp",
  "tenant_id": "clinic_001",
  "function": "get_appointments",
  "params": ["2024-01-01", null, 10]
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `platform` | string | Yes | Platform name (determines the PostgreSQL database prefix) |
| `tenant_id` | string \| null | No | Tenant identifier. Omit or null to use the main database |
| `function` | string | Yes | PostgreSQL function name to call |
| `params` | array | Yes | Ordered list of parameters. Use `null` for SQL NULL |

### Function Name Rules

Function names are validated for SQL injection prevention:
- Must start with a lowercase ASCII letter (`a-z`) or underscore (`_`)
- Maximum length: 63 characters
- Only lowercase ASCII letters, digits, and underscores allowed

**Valid**: `get_patient_by_id`, `list_appointments`, `_internal_fn`

**Invalid**: `DROP TABLE users; --`, `GetPatient` (uppercase), `123fn` (starts with digit)

Returns `400 invalid_request` if violated.

### Parameter Type Handling

The gateway looks up registered parameter types from `_stonescriptdb_gateway_functions` and casts each parameter accordingly. Cache TTL is 5 minutes.

| JSON type | Registered PostgreSQL type | Emitted SQL |
|---|---|---|
| `null` | any | `NULL` |
| `true`/`false` | boolean | `true` / `false` |
| number | any | raw number literal |
| string | `text` / `varchar` | `'escaped_value'` |
| string | other (e.g., `uuid`, `timestamptz`) | `'escaped_value'::uuid` |
| array of integers | `bytea` | `'\xhexbytes'::bytea` |
| array | `text[]`, `integer[]`, etc. | `'["json"]'::text[]` |
| array | unregistered | `'["json"]'::jsonb` |
| object | registered type | `'{"json"}'::custom_type` |
| object | unregistered | `'{"json"}'::jsonb` |

> **Note**: If a function has no registry entry (not yet migrated), the gateway falls back to naive type inference and logs a warning. Run `/v2/migrate` to populate the registry.

### Response

```json
{
  "rows": [
    { "id": "uuid-value", "name": "Alice", "created_at": "2024-01-15T10:00:00Z" }
  ],
  "row_count": 1,
  "execution_time_ms": 12
}
```

| Field | Type | Description |
|---|---|---|
| `rows` | array of objects | Each object maps column names to values |
| `row_count` | integer | Number of rows returned |
| `execution_time_ms` | integer | Total wall-clock time for the call |

### PostgreSQL → JSON Type Conversion

| PostgreSQL Type | JSON Type | Format |
|---|---|---|
| `bool` | boolean | `true` / `false` |
| `int2`, `int4`, `int8` | number | integer |
| `float4`, `float8` | number | float |
| `numeric` | number | converted via f64 |
| `json`, `jsonb` | object / array | native JSON |
| `timestamptz` | string | RFC 3339 (e.g., `2024-01-15T10:00:00Z`) |
| `timestamp` | string | ISO 8601 (e.g., `2024-01-15 10:00:00`) |
| `date` | string | `YYYY-MM-DD` |
| `time` | string | `HH:MM:SS` |
| `NULL` (any type) | `null` | JSON null |
| all others | string | `.to_string()` representation |

### Example: Zero-parameter call

```json
POST /call
{
  "platform": "hospital",
  "function": "list_departments",
  "params": []
}
```

```json
200 OK
{
  "rows": [
    { "id": 1, "name": "Cardiology" },
    { "id": 2, "name": "Neurology" }
  ],
  "row_count": 2,
  "execution_time_ms": 8
}
```

---

## 6. Platform & Schema Management (V2)

The V2 API stores schemas server-side so databases can be provisioned and migrated without re-uploading schemas.

### 6.1 Register Platform

```
POST /platform/register
```

**Request**:
```json
{
  "platform": "myapp",
  "db_user": "optional_pg_user",
  "db_password": "optional_pg_password"
}
```

| Field | Required | Description |
|---|---|---|
| `platform` | Yes | Unique platform identifier |
| `db_user` | No | Dedicated PostgreSQL user for this platform (enables DB-level isolation) |
| `db_password` | No | Password for dedicated PostgreSQL user |

Both `db_user` and `db_password` must be provided together; partial credential pairs are rejected.

**Response** (`201 Created`):
```json
{
  "status": "registered",
  "platform": "myapp",
  "message": "Platform registered with dedicated PostgreSQL credentials. Database isolation enabled.",
  "has_dedicated_credentials": true
}
```

**Error**: `409 platform_already_registered` if platform name is taken.

---

### 6.2 Upload Schema

```
POST /platform/{platform}/schema
Content-Type: multipart/form-data
```

**Form fields**:

| Field | Description |
|---|---|
| `schema_name` (or `name`) | Name for this schema (e.g., `main_db`, `tenant_db`) |
| `schema` (or `file`) | A `.tar.gz` archive containing the schema directory structure |

**Schema archive structure**:
```
postgresql/                  # Optional wrapper directory (auto-stripped)
├── extensions/              # *.sql files — PostgreSQL extensions to install
├── types/                   # *.pgsql files — ENUM, composite, domain definitions
├── tables/                  # *.pgsql files — CREATE TABLE statements
├── functions/               # *.pgsql files — CREATE OR REPLACE FUNCTION statements
├── migrations/              # *.pgsql files — ALTER TABLE and other schema changes
└── seeders/                 # *.pgsql files — INSERT statements for seed data
```

Files within each directory are processed in lexicographic order. Use numeric prefixes (`001_`, `002_`) to control ordering.

**Response** (`201 Created`):
```json
{
  "status": "registered",
  "platform": "myapp",
  "schema_name": "main_db",
  "has_tables": true,
  "has_functions": true,
  "has_migrations": false,
  "checksum": "sha256:abcdef..."
}
```

---

### 6.3 List Schemas

```
GET /platform/{platform}/schemas
```

**Response** (`200 OK`):
```json
{
  "platform": "myapp",
  "schemas": [
    {
      "name": "main_db",
      "has_tables": true,
      "has_functions": true,
      "has_migrations": false,
      "has_seeders": true
    }
  ],
  "count": 1
}
```

---

### 6.4 List Databases

```
GET /platform/{platform}/databases?schema=optional_filter
```

**Query parameters**:

| Parameter | Required | Description |
|---|---|---|
| `schema` | No | Filter databases by schema name |

**Response** (`200 OK`):
```json
{
  "platform": "myapp",
  "databases": [
    {
      "id": "main",
      "database_name": "myapp_main",
      "schema_name": "main_db",
      "created_at": "2024-01-15T10:30:00Z"
    }
  ],
  "count": 1
}
```

---

### 6.5 List Platforms (Admin-Protected)

```
GET /admin/platforms
Authorization: Bearer <ADMIN_TOKEN>
```

**Response** (`200 OK`):
```json
{
  "platforms": [
    { "name": "myapp", "schemas": 2, "databases": 5 }
  ],
  "count": 1
}
```

---

## 7. Database Creation

```
POST /admin/database/create
Authorization: Bearer <ADMIN_TOKEN>
```

Creates a new PostgreSQL database and deploys the full schema from the stored schema archive. On any deployment failure, the partially-created database is dropped (atomic behavior).

### Request

```json
{
  "platform": "myapp",
  "schema_name": "main_db",
  "database_id": "main"
}
```

| Field | Required | Description |
|---|---|---|
| `platform` | Yes | Registered platform name |
| `schema_name` | Yes | Registered schema to deploy |
| `database_id` | Yes | `"main"` creates the main database; any other string creates a tenant database |

The resulting database name is:
- `{platform}_main` when `database_id == "main"`
- `{platform}_{database_id}` otherwise

Identifiers are sanitized: uppercase→lowercase, hyphens/spaces→underscores, leading/trailing underscores stripped.

**Examples**:
```
platform=myapp, database_id=main      → myapp_main
platform=myapp, database_id=clinic-001 → myapp_clinic_001
platform=hospital, database_id=WardA  → hospital_warda
```

### Deployment Sequence

When creating a database, the gateway runs these steps in order:

1. **Create PostgreSQL database** (`CREATE DATABASE`)
2. **Ensure changelog table** (`_stonescriptdb_gateway_changelog`)
3. **Install gateway functions** (internal helpers, idempotent)
4. **Install extensions** (`extensions/` directory)
5. **Deploy custom types** (`types/` directory — must precede tables)
6. **Create tables** (`tables/` directory, `CREATE TABLE IF NOT EXISTS`)
7. **Deploy functions** (`functions/` directory, `CREATE OR REPLACE FUNCTION`)
8. **Run seeders** (`seeders/` directory — only if the target table is empty)

### Response (`201 Created`)

```json
{
  "status": "created",
  "platform": "myapp",
  "schema_name": "main_db",
  "database_name": "myapp_main",
  "extensions_installed": 2,
  "types_deployed": 3,
  "tables_created": 10,
  "functions_deployed": 25,
  "seeders": [
    { "table": "roles", "inserted": 5, "skipped": 0 }
  ],
  "execution_time_ms": 1234
}
```

**Error**: `409 database_already_exists` if `database_name` already exists.

---

## 8. Migration

Migrations update an existing database's schema without recreating it. The gateway compares the stored schema against the live database state before applying changes.

### 8.1 Migrate Single Database

```
POST /v2/migrate
```

Protected by IP filter.

**Request**:
```json
{
  "platform": "myapp",
  "schema_name": "main_db",
  "database_id": "main",
  "force": false
}
```

| Field | Required | Description |
|---|---|---|
| `platform` | Yes | Platform name |
| `schema_name` | Yes | Schema name |
| `database_id` | Yes | `"main"` or a tenant identifier |
| `force` | No (default: `false`) | If `true`, bypasses data-loss validation and schema verification failures |

**Database ID to database name mapping** is identical to [Database Creation](#7-database-creation).

### Migration Sequence

Applied in this order:

1. **Ensure changelog table** (idempotent)
2. **Schema diff validation** — checks desired vs. current schema (see below)
3. **Install extensions** (idempotent)
4. **Deploy custom types** (updated if checksum changed)
5. **Deploy tables** (`CREATE TABLE IF NOT EXISTS` for new tables)
6. **Deploy functions** (`CREATE OR REPLACE` or `DROP + CREATE` if signature changed)
7. **Run migrations** (`ALTER TABLE`, etc. — skipped if checksum matches prior run)
8. **Schema verification** — verifies deployed state matches declarations

### Schema Diff Validation

Before migration, the gateway computes a diff between the desired schema (from stored files) and the current database state:

| Change | Classification | Default behavior |
|---|---|---|
| Add new table | `safe` | Allowed |
| Add nullable column | `safe` | Allowed |
| Widen column type (e.g., `INT` → `BIGINT`) | `safe` | Allowed |
| Add NOT NULL column without DEFAULT | `dataloss` | **Blocked** unless `force=true` |
| Drop column | `dataloss` | **Blocked** unless `force=true` |
| Drop table | `dataloss` | **Blocked** unless `force=true` |
| Narrow column type (e.g., `BIGINT` → `INT`) | `dataloss` | **Blocked** unless `force=true` |

### Function Deployment Logic

| Condition | Action |
|---|---|
| Signature + body checksum unchanged | Skipped |
| Body changed, signature unchanged | `CREATE OR REPLACE FUNCTION` |
| Signature changed (params added/removed/retyped) | `DROP FUNCTION` + `CREATE FUNCTION` |

### Tracking (Idempotency)

Each database maintains internal tracking tables. A migration file is skipped if its checksum matches the previously applied checksum. Checksums normalize whitespace, comments, and case.

### Response (`200 OK`)

```json
{
  "status": "completed",
  "platform": "myapp",
  "schema_name": "main_db",
  "databases_updated": ["myapp_main"],
  "migrations_applied": 3,
  "tables_created": 1,
  "functions_updated": 5,
  "seeder_validations": [
    { "table": "roles", "expected": 5, "found": 0 }
  ],
  "schema_validation": {
    "safe_changes": [
      {
        "table": "appointments",
        "change_type": "AddColumn",
        "column": "cancelled_at",
        "from_type": null,
        "to_type": "timestamptz",
        "compatibility": "safe",
        "reason": null
      }
    ],
    "dataloss_changes": [],
    "incompatible_changes": []
  },
  "verification": {
    "passed": true,
    "extensions_verified": true,
    "types_verified": true,
    "tables_verified": true,
    "seeders_verified": true,
    "error_log": null
  },
  "execution_time_ms": 820
}
```

`status` is `"completed"` when verification passes, `"completed_with_warnings"` when verification fails but `force=true` was used.

**Error**: `500 migration_failed` if a migration step fails (or verification fails without `force=true`).

---

### 8.2 Migrate All Databases

```
POST /v2/migrate-all
```

Same request shape as `/v2/migrate` but applies to all databases registered under the given `platform` and `schema_name`. The response accumulates totals across all databases.

---

## 9. Tenancy Architecture

### Database Naming Convention

| Database type | Naming pattern | Example |
|---|---|---|
| Main database | `{platform}_main` | `hospital_main` |
| Tenant database | `{platform}_{tenant_id}` | `hospital_ward_a` |

**Sanitization rules** applied to both `platform` and `tenant_id`:
- Uppercase letters → lowercase
- Hyphens (`-`) and spaces (` `) → underscores (`_`)
- Any other non-alphanumeric character → underscore (`_`)
- Leading/trailing underscores stripped

**Examples**:
```
platform="MyShop", tenant_id=None        → myshop_main
platform="my-app",   tenant_id="Org-001" → my_app_org_001
platform="__test__", tenant_id=None      → test_main
```

### Routing Function Calls to Tenants

In the `/call` request:
- Omit `tenant_id` or set it to `null` → routes to `{platform}_main`
- Provide a `tenant_id` string → routes to `{platform}_{tenant_id}`

Each tenant gets a fully isolated PostgreSQL database. There is no row-level tenancy; each tenant is a separate database.

### Platform Isolation

The gateway enforces that a `/call` request cannot target a database belonging to a different platform. Violation returns `403 platform_isolation_violation`.

### Per-Platform Database Credentials

If a platform was registered with `db_user` + `db_password`, all connections to that platform's databases use those PostgreSQL credentials instead of the shared gateway user. This provides PostgreSQL-level isolation between platforms.

---

## 10. Auth Endpoints

These endpoints are public (no IP filter). They operate on the `postgres` database where identities and tenants are stored.

### Register

```
POST /auth/register
```

**Request**:
```json
{
  "email": "user@example.com",
  "password": "SecurePass123!",
  "platform_code": "optional_platform",
  "tenant_slug": "optional_tenant"
}
```

**Response** (`201 Created`):
```json
{
  "identity_id": "uuid",
  "email": "user@example.com",
  "email_verified": false,
  "verification_sent": true
}
```

---

### Login

```
POST /auth/login
```

Rate limited. Returns JWT access + refresh tokens.

---

### Refresh

```
POST /auth/refresh
```

Exchanges a refresh token for new access + refresh tokens.

---

### Logout

```
POST /auth/logout
```

Invalidates the refresh token.

---

### Select Tenant / Switch Tenant

```
POST /auth/select-tenant
POST /auth/switch-tenant
```

Scopes a JWT to a specific tenant, returning a tenant-scoped access token.

---

### JWKS

```
GET /auth/jwks
```

Returns the public key set for JWT verification. Used by downstream services to verify tokens without contacting the gateway.

---

### OAuth

```
POST /auth/oauth/initiate
POST /auth/oauth/callback
```

Initiates and completes OAuth2 login (currently: Google). `initiate` returns a redirect URL; `callback` completes the flow and returns tokens.

---

### Account Management

```
POST /account/password-reset/request    # Send reset email
POST /account/password-reset/confirm    # Confirm reset with token
PUT  /account/password                  # Change password (authenticated)
GET  /account/oauth-connections         # List linked OAuth providers
DELETE /account/oauth-connections/:provider  # Unlink OAuth provider
```

---

### Memberships

```
GET    /memberships                  # List memberships for authenticated user
POST   /memberships/invite           # Invite a user to a tenant
POST   /memberships/accept-invite    # Accept an invite
PUT    /memberships/:id              # Update membership role
```

---

## 11. Admin Endpoints

All admin endpoints require `Authorization: Bearer <ADMIN_TOKEN>`. They are additionally restricted to `ALLOWED_ADMIN_IPS`.

### List Databases for Platform

```
GET /admin/databases?platform=myapp
Authorization: Bearer <ADMIN_TOKEN>
```

**Response** (`200 OK`):
```json
{
  "platform": "myapp",
  "databases": [
    { "name": "myapp_main", "type": "main", "size_mb": 42 },
    { "name": "myapp_tenant_001", "type": "tenant", "size_mb": 8 }
  ],
  "count": 2
}
```

---

### Create Empty Tenant Database

```
POST /admin/create-tenant
Authorization: Bearer <ADMIN_TOKEN>
```

Creates a bare PostgreSQL database without deploying any schema. Use this for manually-managed databases; for schema-managed databases, prefer `POST /admin/database/create`.

**Request**:
```json
{
  "platform": "myapp",
  "tenant_id": "clinic_001"
}
```

**Response** (`201 Created`):
```json
{
  "status": "created",
  "database": "myapp_clinic_001",
  "message": "Database created. Run /register or /migrate to deploy schema."
}
```

---

### Create Database with Schema

```
POST /admin/database/create
Authorization: Bearer <ADMIN_TOKEN>
```

See [Section 7](#7-database-creation).

---

## 12. Health Check

```
GET /health
```

No authentication. No IP filter. Suitable for load balancer probes.

**Response** (`200 OK`):
```json
{
  "status": "healthy",
  "version": "1.2.3",
  "postgres_connected": true,
  "active_pools": 7,
  "total_connections": 42,
  "uptime_seconds": 3600
}
```

`status` is `"healthy"` when PostgreSQL is reachable, `"degraded"` otherwise. The HTTP status code is always `200` regardless of `status` value.

---

## 13. Internal Tracking Tables

These tables are created automatically in every managed database. Do not modify them manually.

### `_stonescriptdb_gateway_changelog`

Audit trail of all gateway actions (extensions installed, migrations applied, functions deployed, seeders run).

### `_stonescriptdb_gateway_migrations`

Tracks which migration files have been applied and their checksums. A migration file is skipped on re-run if its checksum matches.

### `_stonescriptdb_gateway_types`

Tracks deployed custom types (ENUMs, composites, domains) and their checksums.

### `_stonescriptdb_gateway_tables`

Tracks deployed table definitions and their checksums.

### `_stonescriptdb_gateway_functions`

Tracks deployed functions including their parameter types. This table is queried by the `/call` endpoint to perform typed parameter casting.

Schema of `_stonescriptdb_gateway_functions` (relevant to consumers):
```
function_name  TEXT        -- matches what you pass in /call
param_types    TEXT[]      -- PostgreSQL type names in parameter order
return_type    TEXT
```

If a function's parameter types are not in this table, the gateway falls back to naive type inference. Run `/v2/migrate` after adding or changing functions to keep this registry current.

---

## 14. Configuration Reference

| Environment Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | — | Full PostgreSQL URL (overrides individual DB_* vars) |
| `DB_HOST` | `localhost` | PostgreSQL host |
| `DB_PORT` | `5432` | PostgreSQL port |
| `DB_NAME` | `postgres` | Admin database name |
| `DB_USER` | `gateway_user` | PostgreSQL user |
| `DB_PASSWORD` | `password` | PostgreSQL password |
| `GATEWAY_HOST` | `127.0.0.1` | IP to bind on |
| `GATEWAY_PORT` | `9000` | Port to listen on |
| `MAX_CONNECTIONS_PER_POOL` | `10` | Max connections per database pool |
| `MAX_TOTAL_CONNECTIONS` | `200` | Max total connections across all pools |
| `POOL_IDLE_TIMEOUT_SECS` | `1800` | Seconds before an idle pool is evicted |
| `POOL_MAX_LIFETIME_SECS` | `3600` | Max lifetime for pool connections |
| `ALLOWED_NETWORKS` | `127.0.0.0/8,::1/128,192.168.0.0/16` | Comma-separated CIDR list for IP filter |
| `DATA_DIR` | `./data` | Directory where schemas are stored |
| `ADMIN_TOKEN` | — | Bearer token for admin endpoints (admin disabled if unset) |
| `ALLOWED_ADMIN_IPS` | `192.168.0.0/16` | Comma-separated CIDR list for admin endpoints |
| `LOG_DIR` | `/var/log/stonescriptdb-gateway` | Log directory (daily rotation) |
| `RUST_LOG` | `debug` | Log level filter |
| `SMTP_HOST` | — | SMTP server hostname |
| `SMTP_PORT` | — | SMTP server port |
| `SMTP_USERNAME` | — | SMTP authentication username |
| `SMTP_PASSWORD` | — | SMTP authentication password |
| `SMTP_FROM_EMAIL` | — | Sender email address |
| `SMTP_FROM_NAME` | — | Sender display name |
| `EMAIL_DEV_MODE` | `false` | If `true`, logs emails instead of sending |
| `GOOGLE_CLIENT_ID` | — | Google OAuth2 client ID |
| `GOOGLE_CLIENT_SECRET` | — | Google OAuth2 client secret |
| `FRONTEND_URL` | — | Base URL for email links (e.g., password reset) |

---

## 15. Connection Pooling

### Pool Lifecycle

- Pools are created lazily on the first `/call` to a database
- One pool per database name
- Pools are evicted when idle for longer than `POOL_IDLE_TIMEOUT_SECS`
- A background cleanup task runs every 5 minutes to remove idle pools

### Limits

- Per-pool connection limit: `MAX_CONNECTIONS_PER_POOL` (default 10)
- Total connections across all pools: `MAX_TOTAL_CONNECTIONS` (default 200)

When a pool is at capacity, new requests wait for a connection to become available. If none becomes available, the request fails with `503 pool_exhausted`.

### LRU Eviction

When the total connection budget approaches the limit, the least-recently-used pools are evicted to make room for new ones.

---

## Appendix: Typical Setup Workflow

```
# 1. Register the platform
POST /platform/register
  { "platform": "myapp" }

# 2. Upload schema archive
POST /platform/myapp/schema
  multipart: schema_name=main_db, schema=<tar.gz>

# 3. Create the main database
POST /admin/database/create
  { "platform": "myapp", "schema_name": "main_db", "database_id": "main" }

# 4. Create a tenant database
POST /admin/database/create
  { "platform": "myapp", "schema_name": "main_db", "database_id": "org-001" }

# 5. Call a function on the main database
POST /call
  { "platform": "myapp", "function": "list_users", "params": [] }

# 6. Call a function on a tenant database
POST /call
  { "platform": "myapp", "tenant_id": "org-001", "function": "get_settings", "params": [] }

# 7. After schema changes, re-upload and migrate
POST /platform/myapp/schema
  multipart: schema_name=main_db, schema=<updated-tar.gz>
POST /v2/migrate
  { "platform": "myapp", "schema_name": "main_db", "database_id": "main" }
```
