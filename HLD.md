# Database Gateway - High Level Design

## 1. Overview

### 1.1 Purpose
The Database Gateway is a Rust-based service that acts as a centralized database proxy for all multi-tenant platforms at Progalaxy E-Labs. Instead of each platform API connecting directly to PostgreSQL, they connect to this gateway which handles:

- **Tenant Routing**: Maps platform + tenant_id to the correct database
- **Connection Pooling**: Shared pool across all platforms (efficient resource usage)
- **Database Registry**: Tracks which databases exist per platform
- **Query Execution**: Parameterized query execution with logging
- **Metrics**: Prometheus-compatible metrics for monitoring

### 1.2 Problem Statement
Our platforms (medstoreapp, instituteapp, etc.) are multi-tenant SaaS applications where each customer (clinic, school, business) gets their own database. With 8+ platforms and potentially hundreds of tenant databases:

- Direct connections from each API replica would exhaust PostgreSQL connections
- No central visibility into which databases exist
- Each platform would need duplicate connection pooling logic
- Difficult to implement cross-cutting concerns (logging, rate limiting)

### 1.3 Solution
A single Rust service running on the PostgreSQL VM that:
- Maintains a shared connection pool (max 200 connections)
- Routes requests to correct tenant database
- Provides admin APIs for database lifecycle management
- Exposes metrics for monitoring

## 2. Architecture

### 2.1 System Context

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Docker Swarm Cluster                              │
│                                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                   │
│  │ medstoreapp  │  │ instituteapp │  │ btechrecruiter│  ... more        │
│  │     API      │  │     API      │  │     API      │                   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘                   │
│         │                 │                 │                            │
└─────────┼─────────────────┼─────────────────┼────────────────────────────┘
          │                 │                 │
          │    Internal Network (10.0.1.x)    │
          │                 │                 │
          ▼                 ▼                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                     vm-postgres-primary (10.0.1.6)                       │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                    DB Gateway (port 9000)                          │  │
│  │                                                                    │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌──────────┐  │  │
│  │  │   Axum      │  │   Tenant    │  │  Connection │  │ Metrics  │  │  │
│  │  │   Router    │  │   Router    │  │    Pool     │  │ Exporter │  │  │
│  │  │             │  │             │  │  (deadpool) │  │          │  │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘  └──────────┘  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                    │                                     │
│                                    ▼                                     │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                    PostgreSQL 16 (port 5432)                       │  │
│  │                         16GB RAM                                   │  │
│  │                                                                    │  │
│  │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐    │  │
│  │  │ medstoreapp_main│  │medstoreapp_     │  │medstoreapp_     │    │  │
│  │  │                 │  │  clinic_001     │  │  clinic_002     │    │  │
│  │  └─────────────────┘  └─────────────────┘  └─────────────────┘    │  │
│  │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐    │  │
│  │  │instituteapp_main│  │instituteapp_    │  │instituteapp_    │    │  │
│  │  │                 │  │  school_001     │  │  school_002     │    │  │
│  │  └─────────────────┘  └─────────────────┘  └─────────────────┘    │  │
│  │                    ... hundreds of databases ...                   │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Component Design

```
db-gateway/
├── src/
│   ├── main.rs              # Entry point, server setup
│   ├── config.rs            # Environment configuration
│   ├── error.rs             # Error types
│   ├── pool/
│   │   ├── mod.rs
│   │   ├── manager.rs       # Pool lifecycle management
│   │   └── registry.rs      # Database registry (which DBs exist)
│   ├── router/
│   │   ├── mod.rs
│   │   └── tenant.rs        # Platform + tenant_id → database name
│   ├── api/
│   │   ├── mod.rs
│   │   ├── health.rs        # GET /health
│   │   ├── query.rs         # POST /query
│   │   ├── admin.rs         # Admin endpoints
│   │   └── middleware.rs    # Auth, logging, rate limiting
│   └── metrics.rs           # Prometheus metrics
├── Cargo.toml
├── Dockerfile
├── docker-compose.yaml
└── .env.example
```

## 3. Data Model

### 3.1 Database Naming Convention

| Type | Pattern | Example |
|------|---------|---------|
| Main DB | `{platform}_main` | `medstoreapp_main` |
| Tenant DB | `{platform}_{tenant_id}` | `medstoreapp_clinic_001` |

### 3.2 In-Memory Registry

```rust
struct DatabaseRegistry {
    // platform -> set of tenant_ids
    databases: DashMap<String, HashSet<String>>,
}

// Populated on startup by querying pg_database
// Updated when create/drop tenant DB
```

### 3.3 Connection Pool Structure

```rust
struct PoolManager {
    // database_name -> connection pool
    pools: DashMap<String, Pool<PostgresConnectionManager>>,

    // LRU tracking for idle pool eviction
    last_used: DashMap<String, Instant>,

    // Global connection counter
    total_connections: AtomicUsize,
}
```

## 4. API Design

### 4.1 Query Execution

```http
POST /query
Content-Type: application/json
X-API-Key: {platform_api_key}

{
  "platform": "medstoreapp",
  "tenant_id": "clinic_001",    // null for main DB
  "query": "SELECT * FROM patients WHERE id = $1",
  "params": [123]
}

Response 200:
{
  "rows": [...],
  "row_count": 1,
  "execution_time_ms": 5
}
```

### 4.2 Admin: Create Tenant Database

```http
POST /admin/create-tenant-db
Content-Type: application/json
X-API-Key: {admin_api_key}

{
  "platform": "medstoreapp",
  "tenant_id": "clinic_042",
  "template": "medstoreapp_template"  // optional
}

Response 201:
{
  "database": "medstoreapp_clinic_042",
  "created": true
}
```

### 4.3 Admin: List Databases

```http
GET /admin/databases?platform=medstoreapp
X-API-Key: {admin_api_key}

Response 200:
{
  "platform": "medstoreapp",
  "databases": [
    {"name": "medstoreapp_main", "type": "main"},
    {"name": "medstoreapp_clinic_001", "type": "tenant"},
    {"name": "medstoreapp_clinic_002", "type": "tenant"}
  ],
  "count": 3
}
```

### 4.4 Health Check

```http
GET /health

Response 200:
{
  "status": "healthy",
  "postgres_connected": true,
  "active_pools": 15,
  "total_connections": 45
}
```

### 4.5 Metrics

```http
GET /metrics

# Prometheus format
db_gateway_queries_total{platform="medstoreapp"} 1234
db_gateway_query_duration_seconds{platform="medstoreapp",quantile="0.99"} 0.05
db_gateway_active_pools 15
db_gateway_total_connections 45
db_gateway_pool_hits{database="medstoreapp_clinic_001"} 500
db_gateway_pool_misses{database="medstoreapp_clinic_001"} 10
```

## 5. Security

### 5.1 Authentication

Each platform gets its own API key:
```
API_KEY_MEDSTOREAPP=ms_xxxxxxxxxxxxx
API_KEY_INSTITUTEAPP=ia_xxxxxxxxxxxxx
API_KEY_ADMIN=admin_xxxxxxxxxxxxx
```

Middleware validates:
1. X-API-Key header present
2. Key matches platform in request body
3. Admin endpoints require admin key

### 5.2 Authorization

- Platform APIs can only query their own databases
- Cannot access other platform's databases
- Admin key required for create/drop operations

### 5.3 SQL Injection Prevention

- All queries use parameterized execution
- No string interpolation of user input
- Query logging for audit

### 5.4 Network Security

- Service binds to 127.0.0.1:9000 (localhost only)
- Accessed via internal network (10.0.1.x)
- No public internet exposure

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
MAX_POOLS = 50  // Evict LRU if exceeded
```

### 6.3 Pool Lifecycle

1. **Lazy Creation**: Pool created on first query to database
2. **LRU Eviction**: Pools unused for 30min are closed
3. **Connection Recycling**: Connections recycled after 1 hour
4. **Health Checks**: Periodic validation of idle connections

## 7. Error Handling

### 7.1 Error Types

```rust
enum GatewayError {
    DatabaseNotFound { platform: String, tenant_id: String },
    ConnectionFailed { database: String, cause: String },
    QueryFailed { query: String, cause: String },
    Unauthorized { reason: String },
    RateLimited { platform: String },
    PoolExhausted { database: String },
}
```

### 7.2 Error Responses

```json
{
  "error": "database_not_found",
  "message": "Database medstoreapp_clinic_999 does not exist",
  "platform": "medstoreapp",
  "tenant_id": "clinic_999"
}
```

## 8. Deployment

### 8.1 Docker Configuration

```dockerfile
# Build stage
FROM rust:1.75-slim-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libpq5 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/db-gateway /usr/local/bin/
EXPOSE 9000
CMD ["db-gateway"]
```

### 8.2 Systemd Service (Alternative)

```ini
[Unit]
Description=Database Gateway Service
After=postgresql.service

[Service]
Type=simple
User=dbgateway
ExecStart=/usr/local/bin/db-gateway
Restart=always
Environment=DATABASE_URL=postgres://...

[Install]
WantedBy=multi-user.target
```

### 8.3 Environment Variables

```bash
# Database connection
DATABASE_URL=postgres://gateway_user:password@localhost:5432/postgres

# Server config
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=9000

# Pool config
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200
POOL_IDLE_TIMEOUT_SECS=1800

# API keys
API_KEY_MEDSTOREAPP=ms_xxxxxxxxxxxxx
API_KEY_INSTITUTEAPP=ia_xxxxxxxxxxxxx
API_KEY_BTECHRECRUITER=br_xxxxxxxxxxxxx
API_KEY_PROGALAXY=pg_xxxxxxxxxxxxx
API_KEY_WEBMETEOR=wm_xxxxxxxxxxxxx
API_KEY_AASAANWORK=aw_xxxxxxxxxxxxx
API_KEY_RESTRANTAPP=ra_xxxxxxxxxxxxx
API_KEY_LOGISTICSAPP=la_xxxxxxxxxxxxx
API_KEY_ADMIN=admin_xxxxxxxxxxxxx

# Logging
RUST_LOG=info
```

## 9. Monitoring

### 9.1 Key Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `queries_total` | Counter | Total queries by platform |
| `query_duration_seconds` | Histogram | Query latency |
| `active_pools` | Gauge | Number of active connection pools |
| `total_connections` | Gauge | Total PostgreSQL connections |
| `pool_hits` | Counter | Requests served by existing pool |
| `pool_misses` | Counter | Requests requiring new pool |
| `errors_total` | Counter | Errors by type |

### 9.2 Alerting Rules

```yaml
- alert: DbGatewayHighLatency
  expr: histogram_quantile(0.99, db_gateway_query_duration_seconds) > 1
  for: 5m

- alert: DbGatewayConnectionsHigh
  expr: db_gateway_total_connections > 180
  for: 5m

- alert: DbGatewayErrorRate
  expr: rate(db_gateway_errors_total[5m]) > 0.1
  for: 5m
```

## 10. Future Enhancements

### Phase 2
- Read replica routing (write to primary, read from replica)
- Query caching (Redis integration)
- Connection multiplexing

### Phase 3
- GraphQL support
- Real-time subscriptions (LISTEN/NOTIFY)
- Multi-region support

## 11. Appendix

### A. Supported Platforms

| Platform | Main DB | Tenant Pattern |
|----------|---------|----------------|
| medstoreapp | medstoreapp_main | medstoreapp_{clinic_id} |
| instituteapp | instituteapp_main | instituteapp_{school_id} |
| btechrecruiter | btechrecruiter_main | btechrecruiter_{company_id} |
| progalaxy | progalaxy_main | progalaxy_{institute_id} |
| webmeteor | webmeteor_main | webmeteor_{customer_id} |
| aasaanwork | aasaanwork_main | aasaanwork_{business_id} |
| restrantapp | restrantapp_main | restrantapp_{restaurant_id} |
| logisticsapp | logisticsapp_main | logisticsapp_{company_id} |

### B. PostgreSQL User Setup

```sql
-- Gateway service user (can create databases)
CREATE USER gateway_user WITH PASSWORD 'xxx' CREATEDB;

-- Grant connect to all databases
GRANT CONNECT ON DATABASE postgres TO gateway_user;
ALTER DEFAULT PRIVILEGES GRANT ALL ON TABLES TO gateway_user;
```
