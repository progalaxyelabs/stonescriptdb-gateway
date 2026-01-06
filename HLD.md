# StoneScript DB Gateway - High Level Design

---
**📖 Navigation:** [Home](README.md) | [Quick Start](docs/QUICKSTART.md) | [Integration](docs/INTEGRATION.md) | **HLD** | [Dev Setup](docs/DEV-ENVIRONMENT.md) | [API v2](docs/API-V2.md)

---

## 1. Overview

### 1.1 Purpose
The Database Gateway is a Rust-based service that acts as a centralized database proxy and schema orchestrator for multi-tenant platforms using PostgreSQL stored functions (like StoneScriptPHP).

**Core responsibilities:**
- **Schema Management**: Receive postgresql folder from platforms, run migrations, deploy functions
- **Query Routing**: Route function calls to correct tenant database
- **Connection Pooling**: Shared pool across all platforms
- **Tenant Lifecycle**: Create/manage tenant databases

### 1.2 Problem Statement
Multi-tenant SaaS platforms where each customer gets their own database face challenges:
- Direct connections from each API replica exhaust PostgreSQL connections
- No central visibility into database schemas across tenants
- Schema migrations must be coordinated across hundreds of databases
- Each platform duplicates connection pooling logic

### 1.3 Solution
A single Rust service that:
- Receives schema files (tar.gz) from platforms on container startup
- Manages migrations and function deployments centrally
- Provides connection pooling (max 200 connections shared)
- Routes queries to correct tenant database

## 2. Architecture

### 2.1 System Context

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      Platform Container Startup                          │
│                                                                          │
│  1. Container starts                                                     │
│  2. php stone schema:export → /tmp/postgresql.tar.gz                    │
│  3. POST /register with tar.gz                                          │
│  4. Wait for "ready" response                                           │
│  5. php stone serve                                                      │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     DB Gateway (vm-postgres-primary)                     │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                      Rust Service (port 9000)                      │  │
│  │                                                                    │  │
│  │  POST /register                                                    │  │
│  │    1. Extract postgresql.tar.gz                                   │  │
│  │    2. Ensure database exists (CREATE DATABASE IF NOT EXISTS)      │  │
│  │    3. Run pending migrations                                       │  │
│  │    4. Deploy all functions (CREATE OR REPLACE FUNCTION)           │  │
│  │    5. Return "ready"                                               │  │
│  │                                                                    │  │
│  │  POST /migrate                                                     │  │
│  │    1. Extract postgresql.tar.gz                                   │  │
│  │    2. Run migrations on specified tenant(s)                       │  │
│  │    3. Update functions                                             │  │
│  │                                                                    │  │
│  │  POST /call                                                        │  │
│  │    1. Route to correct tenant database                            │  │
│  │    2. Execute: SELECT * FROM function_name($1, $2, ...)          │  │
│  │    3. Return result                                                │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                    │                                     │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                    PostgreSQL 16 (port 5432)                       │  │
│  │                                                                    │  │
│  │  platform_main databases:                                          │  │
│  │    myapp_main, platformb_main, platformc_main, ...       │  │
│  │                                                                    │  │
│  │  tenant databases:                                                 │  │
│  │    myapp_tenant_001, myapp_tenant_002, ...            │  │
│  │    platformb_school_001, platformb_school_002, ...          │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Deployment Architecture

The gateway runs in a dedicated VM alongside PostgreSQL, separate from the Docker host:

```
DEV Environment:
  Docker containers on host (rootless) → VM IP (e.g., 192.168.1.100:9000)
  Gateway on VM → localhost:5432 (PostgreSQL on same VM)

PROD Environment:
  Platform containers (Docker Swarm) → VM IP (e.g., 192.168.1.10:9000)
  Gateway on VM → localhost:5432 (PostgreSQL on same VM)
```

**Benefits of VM-based deployment:**
- Avoids rootless Docker network complexity
- Better isolation between compute (containers) and data (database)
- Simplified backup and recovery (entire VM snapshot)
- Easier database performance tuning
- Standard PostgreSQL deployment patterns

Same codebase across environments, only `DB_GATEWAY_URL` changes.

## 3. API Design

### 3.1 POST /register

Container startup - creates DB if needed, runs migrations, deploys functions.

**Request:**
```
POST /register
Content-Type: multipart/form-data

platform: myapp
tenant_id: tenant_001    (optional, null = main DB)
schema: <postgresql.tar.gz>
```

**Response (200):**
```json
{
  "status": "ready",
  "database": "myapp_tenant_001",
  "migrations_applied": 3,
  "functions_deployed": 76,
  "execution_time_ms": 1250
}
```

**Tar.gz structure:**
```
postgresql/
├── functions/
│   ├── get_patient_by_id.pssql
│   ├── list_appointments.pssql
│   └── ... (all .pssql function files)
├── migrations/
│   ├── 001_initial.pssql
│   ├── 002_add_prescriptions.pssql
│   └── ... (ordered migration files)
├── tables/
│   └── ... (table definitions, for reference)
└── seeders/
    └── ... (initial data, optional)
```

### 3.2 POST /migrate

Hot update - deploy new schema without container restart.

**Request:**
```
POST /migrate
Content-Type: multipart/form-data

platform: myapp
tenant_id: tenant_001    (optional, null = ALL tenant DBs for this platform)
schema: <postgresql.tar.gz>
```

**Response (200):**
```json
{
  "status": "completed",
  "databases_updated": [
    "myapp_main",
    "myapp_tenant_001",
    "myapp_tenant_002"
  ],
  "migrations_applied": 1,
  "functions_updated": 76,
  "execution_time_ms": 3500
}
```

### 3.3 POST /call

Execute a database function.

**Request:**
```json
{
  "platform": "myapp",
  "tenant_id": "tenant_001",
  "function": "get_patient_by_id",
  "params": [123]
}
```

**Response (200):**
```json
{
  "rows": [
    {
      "o_patient_id": 123,
      "o_name": "John Doe",
      "o_phone": "+91-9876543210"
    }
  ],
  "row_count": 1,
  "execution_time_ms": 5
}
```

### 3.4 GET /health

**Response (200):**
```json
{
  "status": "healthy",
  "postgres_connected": true,
  "active_pools": 15,
  "total_connections": 45,
  "uptime_seconds": 86400
}
```

### 3.5 GET /admin/databases

List all databases for a platform.

**Request:**
```
GET /admin/databases?platform=myapp
```

**Response (200):**
```json
{
  "platform": "myapp",
  "databases": [
    {"name": "myapp_main", "type": "main", "size_mb": 125},
    {"name": "myapp_tenant_001", "type": "tenant", "size_mb": 45},
    {"name": "myapp_tenant_002", "type": "tenant", "size_mb": 32}
  ],
  "count": 3
}
```

### 3.6 POST /admin/create-tenant

Create a new tenant database.

**Request:**
```json
{
  "platform": "myapp",
  "tenant_id": "tenant_042"
}
```

**Response (201):**
```json
{
  "status": "created",
  "database": "myapp_tenant_042",
  "message": "Database created. Run /register or /migrate to deploy schema."
}
```

## 4. Security

### 4.1 IP Allowlist (No API Keys)

Since stonescriptdb-gateway is internal-only, security is via IP filtering:

```rust
fn is_allowed(ip: IpAddr) -> bool {
    match ip {
        // Localhost (dev)
        IpAddr::V4(v4) if v4.is_loopback() => true,
        IpAddr::V6(v6) if v6.is_loopback() => true,

        // private network (prod): 192.168.1.10/24
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 10 && octets[1] == 0 && octets[2] == 1
        },

        _ => false
    }
}
```

### 4.2 Platform Isolation

- Platforms can only access their own databases
- Platform name extracted from request, validated against database prefix
- Cannot query `platformb_*` databases with `platform: myapp`

## 5. Schema Management

### 5.1 Migration Tracking

Each database has a migrations table:

```sql
CREATE TABLE IF NOT EXISTS _stonescriptdb_gateway_migrations (
    id SERIAL PRIMARY KEY,
    migration_file TEXT NOT NULL UNIQUE,
    checksum TEXT NOT NULL,
    applied_at TIMESTAMPTZ DEFAULT NOW()
);
```

### 5.2 Migration Execution

```rust
async fn run_migrations(&self, db: &str, migrations_dir: &Path) -> Result<usize> {
    let mut applied = 0;

    // Get already applied migrations
    let existing = self.get_applied_migrations(db).await?;

    // Find .pssql files, sorted by name (001_, 002_, etc.)
    let files = self.find_migration_files(migrations_dir)?;

    for file in files {
        if !existing.contains(&file.name) {
            // Run migration
            let sql = fs::read_to_string(&file.path)?;
            self.execute_sql(db, &sql).await?;

            // Record it
            self.record_migration(db, &file.name, &file.checksum).await?;
            applied += 1;
        }
    }

    Ok(applied)
}
```

### 5.3 Function Deployment

All functions use `CREATE OR REPLACE`, so they're always overwritten:

```rust
async fn deploy_functions(&self, db: &str, functions_dir: &Path) -> Result<usize> {
    let files = self.find_pssql_files(functions_dir)?;

    for file in &files {
        let sql = fs::read_to_string(&file)?;
        self.execute_sql(db, &sql).await?;
    }

    Ok(files.len())
}
```

## 6. Connection Pool Strategy

### 6.1 Pool Configuration

```rust
PoolConfig {
    max_size: 10,              // Per database
    min_idle: 1,               // Keep 1 warm connection
    connection_timeout: 5s,
    idle_timeout: 30min,
    max_lifetime: 1hour,
}
```

### 6.2 Global Limits

```
MAX_TOTAL_CONNECTIONS = 200
MAX_POOLS = 100  // Evict LRU if exceeded
```

### 6.3 Pool Lifecycle

1. **Lazy Creation**: Pool created on first query to database
2. **LRU Eviction**: Pools unused for 30min are closed
3. **Connection Recycling**: Connections recycled after 1 hour

## 7. Project Structure

```
stonescriptdb-gateway/
├── src/
│   ├── main.rs              # Entry point
│   ├── config.rs            # Environment config
│   ├── error.rs             # Error types
│   ├── api/
│   │   ├── mod.rs
│   │   ├── register.rs      # POST /register
│   │   ├── migrate.rs       # POST /migrate
│   │   ├── call.rs          # POST /call
│   │   ├── admin.rs         # Admin endpoints
│   │   └── health.rs        # GET /health
│   ├── schema/
│   │   ├── mod.rs
│   │   ├── extractor.rs     # Unpack tar.gz
│   │   ├── migration.rs     # Run migrations
│   │   └── functions.rs     # Deploy functions
│   ├── pool/
│   │   ├── mod.rs
│   │   ├── manager.rs       # Connection pool lifecycle
│   │   └── router.rs        # Platform/tenant → database
│   └── security/
│       └── ip_filter.rs     # IP allowlist middleware
├── Cargo.toml
├── Dockerfile
├── docker-compose.yaml
├── .env.example
└── README.md
```

## 8. Deployment

### 8.1 Docker

```dockerfile
# Build
FROM rust:1.75-slim-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

# Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libpq5 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/stonescriptdb-gateway /usr/local/bin/
EXPOSE 9000
HEALTHCHECK --interval=30s --timeout=5s CMD curl -f http://localhost:9000/health || exit 1
CMD ["stonescriptdb-gateway"]
```

### 8.2 Systemd (Alternative)

```ini
[Unit]
Description=StoneScriptDB Gateway
After=postgresql.service

[Service]
Type=simple
User=stonescriptdb-gateway
ExecStart=/usr/local/bin/stonescriptdb-gateway
Restart=always
EnvironmentFile=/etc/stonescriptdb-gateway/env

[Install]
WantedBy=multi-user.target
```

### 8.3 Environment Variables

```bash
# PostgreSQL connection
DATABASE_URL=postgres://gateway_user:password@localhost:5432/postgres

# Server
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=9000

# Pool settings
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200
POOL_IDLE_TIMEOUT_SECS=1800

# Security
ALLOWED_NETWORKS=127.0.0.0/8,192.168.1.10/24

# Logging
RUST_LOG=info
```

## 9. StoneScriptPHP Integration

### 9.1 New CLI Command

```bash
php stone schema:export
# Creates /tmp/postgresql.tar.gz from api/src/postgresql/
```

### 9.2 Container Entrypoint

```bash
#!/bin/bash
set -e

# Export schema
php stone schema:export

# Register with gateway
response=$(curl -s -X POST $DB_GATEWAY_URL/register \
  -F "platform=$PLATFORM_ID" \
  -F "tenant_id=$TENANT_ID" \
  -F "schema=@/tmp/postgresql.tar.gz")

status=$(echo $response | jq -r '.status')
if [ "$status" != "ready" ]; then
  echo "Gateway registration failed: $response"
  exit 1
fi

echo "Schema deployed: $(echo $response | jq -r '.functions_deployed') functions"

# Start server
exec php stone serve
```

### 9.3 Database.php Changes

```php
// Instead of direct pg_connect, call gateway
class Database {
    private static string $gateway_url;
    private static string $platform;
    private static ?string $tenant_id;

    public static function fn(string $function_name, array $params): array
    {
        $response = self::http_post(self::$gateway_url . '/call', [
            'platform' => self::$platform,
            'tenant_id' => self::$tenant_id,
            'function' => $function_name,
            'params' => $params
        ]);

        return $response['rows'];
    }
}
```

## 10. Database Naming Convention

| Type | Pattern | Example |
|------|---------|---------|
| Main DB | `{platform}_main` | `myapp_main` |
| Tenant DB | `{platform}_{tenant_id}` | `myapp_tenant_001` |

## 11. Error Handling

### 11.1 Error Types

```rust
enum GatewayError {
    DatabaseNotFound { platform: String, tenant_id: Option<String> },
    MigrationFailed { database: String, migration: String, cause: String },
    FunctionDeployFailed { database: String, function: String, cause: String },
    QueryFailed { function: String, cause: String },
    SchemaExtractionFailed { cause: String },
    ConnectionFailed { database: String, cause: String },
    PoolExhausted { database: String },
    Unauthorized { ip: String },
}
```

### 11.2 Error Responses

```json
{
  "error": "migration_failed",
  "message": "Migration 003_add_audit_log.pssql failed",
  "database": "myapp_tenant_001",
  "cause": "relation 'audit_log' already exists"
}
```

## 12. Future Enhancements

- **Read replica routing**: Write to primary, read from replica
- **Query caching**: Redis integration for frequent queries
- **Metrics**: Prometheus endpoint for monitoring
- **Backup coordination**: Trigger backups across tenant databases
- **Schema versioning**: Track schema versions per platform
