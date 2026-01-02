# StoneScriptDB Gateway Integration Guide

## Overview

This document describes how platforms integrate with StoneScriptDB Gateway for database management. The gateway handles:
- Database creation per platform/tenant
- PostgreSQL extension installation
- Schema migrations with validation
- Intelligent function deployment
- Seed data management
- Query routing

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Platform API                              │
│  (stonescriptphp-server or any HTTP client)                     │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ HTTP (POST /register, /migrate, /call)
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                   StoneScriptDB Gateway                          │
│                   (port 9000)                                    │
│  • Receives postgresql/ folder as tar.gz                        │
│  • Runs migrations                                               │
│  • Deploys functions                                             │
│  • Routes /call requests to correct tenant DB                   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      PostgreSQL                                  │
│  • {platform}_main (main database)                              │
│  • {platform}_{tenant_id} (tenant databases)                    │
└─────────────────────────────────────────────────────────────────┘
```

## Integration Options

### Option 1: StoneScriptPHP-Server Integration (Recommended)

For platforms using `stonescriptphp-server`, the server handles gateway communication automatically.

**Configuration in platform's `.env`:**
```env
DB_GATEWAY_URL=http://localhost:9000
PLATFORM_ID=myplatform
```

**How it works:**
1. On startup, stonescriptphp-server reads `src/postgresql/` folder
2. Creates tar.gz and sends to gateway's `/register` endpoint
3. Gateway creates database, runs migrations, deploys functions
4. All database calls go through gateway's `/call` endpoint

**Platform folder structure:**
```
my-platform/
├── src/
│   └── postgresql/
│       ├── extensions/          # PostgreSQL extensions (uuid-ossp, pgvector, etc.)
│       │   ├── uuid-ossp.sql
│       │   └── pgvector.sql
│       ├── migrations/          # Ordered SQL migrations
│       │   ├── 001_create_users.pssql
│       │   ├── 002_create_orders.pssql
│       │   └── 003_add_indexes.pssql
│       ├── tables/              # Declarative table definitions (for validation)
│       │   ├── users.pssql
│       │   └── orders.pssql
│       ├── functions/           # PostgreSQL functions
│       │   ├── get_user_by_id.pssql
│       │   ├── insert_order.pssql
│       │   └── ...
│       └── seeders/             # Seed data (auto-run on register, validated on migrate)
│           └── roles.pssql
└── ...
```

### Option 2: Direct HTTP Integration

For non-StoneScriptPHP platforms or custom integrations.

**Register/Update Schema:**
```bash
# Create tar.gz of postgresql folder
tar -czf schema.tar.gz -C src postgresql

# Register with gateway
curl -X POST http://localhost:9000/register \
  -F "platform=myplatform" \
  -F "schema=@schema.tar.gz"

# For tenant-specific database
curl -X POST http://localhost:9000/register \
  -F "platform=myplatform" \
  -F "tenant_id=tenant_001" \
  -F "schema=@schema.tar.gz"
```

**Call Functions:**
```bash
# No parameters
curl -X POST http://localhost:9000/call \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myplatform",
    "function": "get_all_users",
    "params": []
  }'

# With parameters (array in order)
curl -X POST http://localhost:9000/call \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myplatform",
    "function": "get_user_by_id",
    "params": [123]
  }'

# With tenant
curl -X POST http://localhost:9000/call \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myplatform",
    "tenant_id": "tenant_001",
    "function": "get_orders",
    "params": [1, 10]
  }'
```

### Option 3: Use Provided Scripts

Copy the scripts from gateway repo to your platform:

```bash
# Copy scripts
cp /path/to/stonescriptdb-gateway/scripts/register-with-gateway.sh ./scripts/
cp /path/to/stonescriptdb-gateway/scripts/migrate-schema.sh ./scripts/

# Register platform
DB_GATEWAY_URL=http://localhost:9000 \
PLATFORM_ID=myplatform \
./scripts/register-with-gateway.sh

# Hot migrate (update functions without restart)
DB_GATEWAY_URL=http://localhost:9000 \
PLATFORM_ID=myplatform \
./scripts/migrate-schema.sh
```

---

## Schema Processing Order

When you call `/register` or `/migrate`, the gateway processes your schema in this order:

```
1. Extensions    → Install PostgreSQL extensions (uuid-ossp, pgvector, etc.)
2. Types         → Deploy custom types (ENUMs, composites, domains)
3. Tables        → Validate schema changes (check for data loss)
4. Migrations    → Run new migration files
5. Functions     → Deploy changed functions
6. Seeders       → Run (register) or validate (migrate)
```

This order ensures:
- Extensions are available before custom types use their features
- Custom types are available before migrations use them
- Schema validation happens before any changes
- Migrations run before functions that may depend on new tables
- Seeders run last after all schema is in place

---

## File Conventions

### Extensions (`postgresql/extensions/`)

Define PostgreSQL extensions your schema requires. **Extensions are installed before migrations**, so your migrations can use extension types.

**Naming:** `extension_name.sql` (filename = extension name)

**Simple extension:**
```sql
-- extensions/uuid-ossp.sql
-- UUID generation functions
```

**Extension with options:**
```sql
-- extensions/pgvector.sql
-- Vector similarity search
-- version: 0.5.0
-- schema: extensions
```

**Behavior:**
- Already-installed extensions are skipped
- Extensions installed before migrations run
- Clear error if extension not available on server

**Common extensions:**
| Extension | Purpose |
|-----------|---------|
| `uuid-ossp` | UUID generation (`uuid_generate_v4()`) |
| `pgcrypto` | Cryptographic functions |
| `pgvector` | Vector embeddings for AI/ML |
| `postgis` | Geographic data types |
| `pg_trgm` | Trigram text search |

### Types (`postgresql/types/`)

Define custom PostgreSQL types (ENUMs, composite types, domains). **Types are deployed after extensions but before migrations**, so your migrations can use custom types.

**Naming:** `type_name.pssql` (filename = type name)

**ENUM type:**
```sql
-- types/order_status.pssql
CREATE TYPE order_status AS ENUM (
    'pending',
    'processing',
    'shipped',
    'delivered',
    'cancelled'
);
```

**Composite type:**
```sql
-- types/address.pssql
CREATE TYPE address AS (
    street TEXT,
    city TEXT,
    state TEXT,
    zip_code TEXT,
    country TEXT
);
```

**Domain type:**
```sql
-- types/email.pssql
CREATE DOMAIN email AS TEXT
CHECK (VALUE ~ '^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$');
```

**Behavior:**
- Types are tracked with checksums (like functions)
- Already-deployed types with matching checksum are skipped
- Changed types require manual migration (PostgreSQL limitation)
- Tracked in `_stonescriptdb_gateway_types` table

**Important:** PostgreSQL ENUMs cannot be modified after creation (you can only add values, not remove/rename). If you need to change an ENUM, create a migration that:
1. Creates a new type
2. Alters columns to use the new type
3. Drops the old type

### Tables (`postgresql/tables/`)

Declarative table definitions used for **schema validation** before migrations. The gateway compares your table definitions against the current database and blocks potentially destructive changes.

**Example:**
```sql
-- tables/users.pssql
CREATE TABLE users (
    user_id SERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(100) NOT NULL,
    created_on TIMESTAMPTZ DEFAULT NOW()
);
```

**Automatic dependency ordering:** Tables with foreign keys are created in the correct order. No need to prefix with `001_`, `002_`, etc.

```sql
-- tables/orders.pssql (created AFTER users because of FK)
CREATE TABLE orders (
    order_id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(user_id),
    total DECIMAL(10,2) NOT NULL
);
```

**Schema diff validation:** Before migrations run, the gateway classifies changes:

| Change | Classification | Behavior |
|--------|---------------|----------|
| Add table | Safe | Allowed |
| Drop table | DataLoss | Blocked |
| Add nullable column | Safe | Allowed |
| Add NOT NULL column without DEFAULT | DataLoss | Blocked |
| Drop column | DataLoss | Blocked |
| Widen type (INT → BIGINT) | Safe | Allowed |
| Narrow type (BIGINT → INT) | DataLoss | Blocked |
| Incompatible type (INT → TEXT) | Incompatible | Blocked |

**Force mode:** To bypass data loss checks (e.g., intentional column removal):
```bash
curl -X POST http://localhost:9000/migrate \
  -F "platform=myapp" \
  -F "schema=@schema.tar.gz" \
  -F "force=true"
```

### Migrations (`postgresql/migrations/`)

- **Naming:** `NNN_description.pssql` (e.g., `001_create_users.pssql`)
- **Ordering:** Files are executed in alphabetical order
- **Idempotent:** Use `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`
- **Tracked:** Gateway tracks applied migrations by filename + checksum

```sql
-- 001_create_users.pssql
CREATE TABLE IF NOT EXISTS users (
    user_id SERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    display_name VARCHAR(100) NOT NULL,
    created_on TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
```

### Functions (`postgresql/functions/`)

- **Naming:** `function_name.pssql` (matches function name)
- **Format:** Use `CREATE OR REPLACE FUNCTION`
- **Smart deployment:** Only changed functions are redeployed

```sql
-- get_user_by_id.pssql
CREATE OR REPLACE FUNCTION get_user_by_id(
    p_user_id INT
)
RETURNS TABLE (
    user_id INT,
    email TEXT,
    display_name TEXT
) AS $$
BEGIN
    RETURN QUERY
    SELECT u.user_id, u.email::TEXT, u.display_name::TEXT
    FROM users u
    WHERE u.user_id = p_user_id;
END;
$$ LANGUAGE plpgsql;
```

**Intelligent change detection:** The gateway tracks functions with checksums and only redeploys when needed:

| Scenario | Action |
|----------|--------|
| Unchanged function | **Skipped** (checksum match) |
| Body changed, same signature | `CREATE OR REPLACE` |
| Signature changed (params added/removed) | `DROP` old + `CREATE` new |
| Function removed from source | `DROP` (orphan cleanup) |

**Checksum normalization:** These changes are considered identical (no redeploy):
- Whitespace/formatting changes
- Comment changes
- Case changes (`BEGIN` vs `begin`)

This means 75 unchanged functions = 75 skipped (no SQL executed), making deployments fast.

**Signature changes:** If you change a function's parameters, the gateway handles it automatically:

```sql
-- Before: get_user(INT)
CREATE OR REPLACE FUNCTION get_user(p_id INT) ...

-- After: get_user(INT, BOOLEAN) - different signature!
CREATE OR REPLACE FUNCTION get_user(p_id INT, p_include_deleted BOOLEAN DEFAULT FALSE) ...
```

The gateway will `DROP FUNCTION get_user(INT)` before creating `get_user(INT, BOOLEAN)`. Without this, PostgreSQL would have **both** functions (overloads).

### Function Parameter Conventions

Parameters are passed as an ordered array, so document the order:

```sql
-- insert_order.pssql
-- Params: [p_user_id, p_product_id, p_quantity, p_notes]
CREATE OR REPLACE FUNCTION insert_order(
    p_user_id INT,
    p_product_id INT,
    p_quantity INT,
    p_notes TEXT DEFAULT NULL
)
RETURNS INT AS $$
...
```

**Calling from API:**
```json
{
  "platform": "myplatform",
  "function": "insert_order",
  "params": [1, 42, 3, "Gift wrap please"]
}
```

### Seeders (`postgresql/seeders/`)

Seed data (initial records like roles, permissions, countries). **Behavior differs between `/register` and `/migrate`:**

| Endpoint | Behavior |
|----------|----------|
| `/register` | Run seeders **only if table is empty** |
| `/migrate` | **Validate** seeders exist, fail if missing |

**Example seeder:**
```sql
-- seeders/roles.pssql
INSERT INTO roles (id, name) VALUES
    (1, 'admin'),
    (2, 'user'),
    (3, 'guest');
```

**Why validation on migrate?**

Seeders define required data. If you have a seeder with 3 roles, those roles should always exist in production. The `/migrate` endpoint verifies this - if someone manually deleted a role, the migration fails rather than silently continuing with broken data.

**Response includes validation info:**
```json
{
  "seeder_validations": [
    {"table": "roles", "expected": 3, "found": 3},
    {"table": "permissions", "expected": 10, "found": 10}
  ]
}
```

---

## Development Workflow

### Initial Setup

1. **Create postgresql folder structure:**
   ```
   src/postgresql/
   ├── migrations/
   └── functions/
   ```

2. **Add first migration:**
   ```sql
   -- migrations/001_initial_schema.pssql
   CREATE TABLE IF NOT EXISTS ...
   ```

3. **Add functions:**
   ```sql
   -- functions/get_something.pssql
   CREATE OR REPLACE FUNCTION get_something(...) ...
   ```

4. **Register with gateway:**
   ```bash
   DB_GATEWAY_URL=http://localhost:9000 \
   PLATFORM_ID=myapp \
   ./scripts/register-with-gateway.sh
   ```

### Adding New Features

1. **Add migration file** (if schema changes needed):
   ```sql
   -- migrations/002_add_orders_table.pssql
   ```

2. **Add/update functions**

3. **Migrate:**
   ```bash
   ./scripts/migrate-schema.sh
   ```

### Updating Functions Only (Hot Deploy)

If only changing function logic (no schema changes):

```bash
./scripts/migrate-schema.sh
```

Functions are redeployed without running migrations again.

---

## Multi-Tenant Setup

### Per-Tenant Databases

Each tenant gets their own database with identical schema:

```
myplatform_main         # Main/default database
myplatform_tenant_001   # Tenant 001
myplatform_tenant_002   # Tenant 002
```

### Registering Tenants

```bash
# Register new tenant
curl -X POST http://localhost:9000/register \
  -F "platform=myplatform" \
  -F "tenant_id=tenant_001" \
  -F "schema=@schema.tar.gz"
```

### Calling Tenant Functions

```json
{
  "platform": "myplatform",
  "tenant_id": "tenant_001",
  "function": "get_orders",
  "params": []
}
```

### Migrating All Tenants

```bash
# Migrate all tenant databases
curl -X POST http://localhost:9000/migrate \
  -F "platform=myplatform" \
  -F "schema=@schema.tar.gz"
```

---

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Deploy Schema

on:
  push:
    branches: [main]
    paths:
      - 'src/postgresql/**'

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Create schema archive
        run: tar -czf schema.tar.gz -C src postgresql

      - name: Deploy to gateway
        run: |
          curl -X POST ${{ secrets.DB_GATEWAY_URL }}/migrate \
            -F "platform=${{ secrets.PLATFORM_ID }}" \
            -F "schema=@schema.tar.gz"
```

### Docker Compose Integration

```yaml
services:
  api:
    environment:
      - DB_GATEWAY_URL=http://gateway:9000
      - PLATFORM_ID=myplatform
    depends_on:
      - gateway

  gateway:
    image: stonescriptdb-gateway
    environment:
      - DB_HOST=postgres
      - DB_USER=gateway_user
      - DB_PASSWORD=secret
    depends_on:
      - postgres

  postgres:
    image: postgres:15
```

---

## Register vs Migrate

Understanding when to use each endpoint:

| Aspect | `/register` | `/migrate` |
|--------|-------------|------------|
| **Purpose** | Create new database | Update existing database(s) |
| **Database state** | Must NOT exist | Must exist |
| **When to use** | First-time setup, new tenant | Schema updates, function changes |
| **Migrations** | Run all migrations | Run only new migrations |
| **Functions** | Deploy all functions | Redeploy changed functions |
| **Seeders** | Insert into empty tables | Validate records exist |
| **Error if DB exists** | Yes (409 Conflict) | No |
| **Error if DB missing** | No (creates it) | Yes (404 Not Found) |
| **Tenant behavior** | Creates single tenant DB | Updates all tenant DBs (if no tenant_id) |

**Typical workflow:**
1. Platform starts → calls `/register` (creates DB if new, fails if exists)
2. Schema changes → calls `/migrate` (updates existing DBs)
3. New tenant → calls `/register` with `tenant_id` (creates tenant DB)
4. Tenant updates → calls `/migrate` with or without `tenant_id`

---

## API Reference

### POST /register

Register a **new** platform/tenant and apply schema. Creates database and applies initial schema.

**Important:** This endpoint is for **new databases only**. If the database already exists, the request will fail with HTTP 409 Conflict. Use `/migrate` to update existing databases.

**Request:**
- `platform` (required): Platform identifier
- `tenant_id` (optional): Tenant identifier (omit for main database)
- `schema` (required): tar.gz file containing `postgresql/` folder

**Response:**
```json
{
  "status": "ready",
  "database": "myplatform_main",
  "extensions_installed": 2,
  "types_deployed": 3,
  "migrations_applied": 3,
  "functions_deployed": 10,
  "seeders": [
    {"table": "roles", "inserted": 3, "skipped": 0}
  ],
  "execution_time_ms": 250
}
```

**Error - Database exists:**
```json
{
  "error": "database_already_exists",
  "message": "Database 'myplatform_main' already exists",
  "database": "myplatform_main"
}
```

### POST /migrate

Update schema for existing platform. Validates schema changes and seeders.

**Request:**
- `platform` (required): Platform identifier
- `tenant_id` (optional): Specific tenant (omit to migrate ALL tenants)
- `schema` (required): tar.gz file containing `postgresql/` folder
- `force` (optional): Set to `true` to bypass data loss checks

**Response:**
```json
{
  "status": "completed",
  "databases_updated": ["myplatform_main", "myplatform_tenant_001"],
  "extensions_installed": 0,
  "migrations_applied": 1,
  "functions_updated": 2,
  "seeder_validations": [
    {"table": "roles", "expected": 3, "found": 3}
  ],
  "schema_validation": {
    "safe_changes": [
      {"table": "users", "change_type": "AddColumn", "column": "phone", "compatibility": "safe"}
    ],
    "dataloss_changes": [],
    "incompatible_changes": []
  },
  "execution_time_ms": 150
}
```

**Error responses:**

Data loss detected (without force=true):
```json
{
  "error": "schema_validation_failed",
  "message": "Schema changes would cause data loss",
  "dataloss_changes": [
    {"table": "users", "change_type": "DropColumn", "column": "legacy_field"}
  ]
}
```

Seeder validation failed:
```json
{
  "error": "seeder_validation_failed",
  "message": "Seeder validation failed for table 'roles': expected 3 records, found 2"
}
```

### POST /call

Execute a PostgreSQL function.

**Request:**
```json
{
  "platform": "myplatform",
  "tenant_id": "optional_tenant",
  "function": "function_name",
  "params": [param1, param2, ...]
}
```

**Response:**
```json
{
  "rows": [{"column1": "value1", ...}],
  "row_count": 1,
  "execution_time_ms": 5
}
```

### GET /health

Health check endpoint.

**Response:**
```json
{
  "status": "healthy",
  "postgres_connected": true,
  "active_pools": 2,
  "total_connections": 20,
  "uptime_seconds": 3600
}
```

---

## Troubleshooting

### Common Issues

**"Function not found" error:**
- Ensure function file is in `postgresql/functions/`
- File must have `.pssql` extension
- Function name must be lowercase with underscores only

**Migration not applied:**
- Check migration filename follows `NNN_name.pssql` pattern
- Verify file is in `postgresql/migrations/` folder
- Check gateway logs for SQL errors

**Connection refused:**
- Verify gateway is running: `systemctl status stonescriptdb-gateway`
- Check ALLOWED_NETWORKS includes your IP
- Verify PostgreSQL is accessible

### Viewing Logs

```bash
# Gateway logs
journalctl -u stonescriptdb-gateway -f

# File logs
tail -f /var/log/stonescriptdb-gateway/stonescriptdb-gateway.log
```

### Testing Connection

```bash
curl http://localhost:9000/health
```

---

## Best Practices

1. **Version your migrations:** Use sequential numbering (001, 002, ...)
2. **Keep functions focused:** One function per file, single responsibility
3. **Document parameters:** Add comments showing param order
4. **Use transactions:** Migrations should be atomic
5. **Test locally:** Run tests before deploying to production
6. **Backup before migrate:** Especially for production databases
