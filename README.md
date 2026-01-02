# StoneScript DB Gateway

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub release](https://img.shields.io/github/v/release/progalaxyelabs/stonescriptdb-gateway)](https://github.com/progalaxyelabs/stonescriptdb-gateway/releases)

Rust-based multi-tenant database gateway and schema orchestrator for PostgreSQL function-based platforms.

## Overview

This service acts as a centralized database proxy and schema orchestrator for multi-tenant platforms using PostgreSQL stored functions (like StoneScriptPHP). Platform containers send their schema files on startup, and the gateway handles migrations and function deployments.

## Features

- **Schema Management**: Receive postgresql.tar.gz, run migrations, deploy functions
- **Query Routing**: Route function calls to correct tenant database
- **Connection Pooling**: Shared pool across all platforms (max 200 connections)
- **Tenant Lifecycle**: Create and manage tenant databases
- **IP-based Security**: No API keys needed for internal services
- **PostgreSQL Extensions**: Automatic installation of uuid-ossp, pgvector, postgis, etc.
- **Custom Types**: ENUM, composite, and domain type management with checksum tracking
- **Table Dependency Ordering**: Automatic topological sort for foreign key constraints
- **Schema Diff Validation**: Pre-migration type compatibility checking with data loss prevention
- **Function Signature Tracking**: Intelligent function deployment with orphan cleanup
- **Seeder Validation**: Seed data integrity checking on migrations

## Quick Start

### Installation from Release

```bash
# Clone specific version
git clone https://github.com/progalaxyelabs/stonescriptdb-gateway.git
cd stonescriptdb-gateway
git checkout v1.0.0

# Install as systemd service
sudo ./deploy/install.sh

# Configure
sudo nano /opt/stonescriptdb-gateway/.env

# Start
sudo systemctl start stonescriptdb-gateway
```

### Updating to Latest Release

```bash
cd /path/to/stonescriptdb-gateway
git fetch --tags
git checkout v1.1.0  # or latest version
cargo build --release
sudo cp target/release/stonescriptdb-gateway /opt/stonescriptdb-gateway/
sudo systemctl restart stonescriptdb-gateway
```

## How It Works

```
Container Startup:
1. php stone schema:export → postgresql.tar.gz
2. POST /register with tar.gz
3. Gateway: extract → create DB → run migrations → deploy functions
4. Return "ready"
5. Container starts serving

Runtime:
Platform API → POST /call → Gateway → PostgreSQL function → Response
```

## Architecture

```
Platform Containers              StoneScriptDB Gateway
┌──────────────────┐           ┌──────────────────┐
│ platform-a-api   │──────────▶│                  │
│ platform-b-api   │  /register│  Rust Service    │
│ platform-c-api   │  /call    │  (port 9000)     │
└──────────────────┘           └────────┬─────────┘
                                        │
                                        ▼
                               ┌──────────────────┐
                               │  PostgreSQL 16   │
                               │  (port 5432)     │
                               │                  │
                               │  *_main DBs      │
                               │  *_tenant DBs    │
                               └──────────────────┘
```

See [HLD.md](./HLD.md) for detailed architecture documentation.

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/register` | POST | Container startup: deploy schema to database |
| `/migrate` | POST | Hot update: deploy schema without restart |
| `/call` | POST | Execute database function |
| `/health` | GET | Health check |
| `/admin/databases` | GET | List databases for a platform |
| `/admin/create-tenant` | POST | Create new tenant database |

## Development

### Building with Docker (Cross-platform)

Use the provided Dockerfile to build a binary compatible with Ubuntu 22.04:

```bash
# Copy the example Dockerfile
cp Dockerfile.build.example Dockerfile.build

# Build the builder image
docker build -f Dockerfile.build -t stonescriptdb-gateway-builder .

# Build the binary
mkdir -p output
docker run --rm -v "$PWD/output:/output" stonescriptdb-gateway-builder

# Binary output: ./output/stonescriptdb-gateway
```

### Deploy to Server

```bash
# Upload binary
scp output/stonescriptdb-gateway user@server:/tmp/

# Install on server
ssh user@server
sudo cp /tmp/stonescriptdb-gateway /opt/stonescriptdb-gateway/
sudo systemctl restart stonescriptdb-gateway
```

### Local Development (requires Rust installed)

```bash
# Clone
git clone https://github.com/progalaxyelabs/stonescriptdb-gateway.git
cd stonescriptdb-gateway

# Setup
cargo build

# Run locally
cp .env.example .env
# Edit .env with your PostgreSQL credentials
cargo run

# Run tests
cargo test

# Build release
cargo build --release
```

### Production Deployment (systemd)

```bash
# Install as systemd service
sudo ./deploy/install.sh

# Configure
sudo nano /opt/stonescriptdb-gateway/.env

# Start service
sudo systemctl start stonescriptdb-gateway

# Check status
sudo systemctl status stonescriptdb-gateway

# View logs
sudo journalctl -u stonescriptdb-gateway -f
```

See `deploy/` directory for installation scripts and systemd service file.

## Environment Variables

```bash
# PostgreSQL connection (individual fields recommended)
DB_HOST=localhost
DB_PORT=5432
DB_NAME=postgres
DB_USER=gateway_user
DB_PASSWORD=your_password

# Or use DATABASE_URL (less recommended due to special char issues)
# DATABASE_URL=postgres://gateway_user:password@localhost:5432/postgres

GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=9000
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200
POOL_IDLE_TIMEOUT_SECS=1800
ALLOWED_NETWORKS=127.0.0.0/8,10.0.1.0/24
RUST_LOG=info
```

## Schema Tar.gz Structure

Platforms export their postgresql folder as tar.gz:

```
postgresql/
├── extensions/         # *.sql - PostgreSQL extensions (uuid-ossp, pgvector, etc.)
├── types/              # *.pssql - Custom types (ENUM, composite, domain)
├── functions/          # *.pssql - CREATE OR REPLACE FUNCTION
├── migrations/         # *.pssql - Ordered by dependency, not filename
├── tables/             # Table definitions (declarative schema)
└── seeders/            # Initial data (validated on migrate)
```

## Advanced Schema Features

### PostgreSQL Extensions

Define required extensions in the `extensions/` folder. Each file represents one extension (filename = extension name).

**Simple extension** (`extensions/uuid-ossp.sql`):
```sql
-- UUID generation functions
```

**Extension with options** (`extensions/pgvector.sql`):
```sql
-- Vector similarity search
-- version: 0.5.0
-- schema: extensions
```

Extensions are installed **before** migrations run, so your migrations can use extension types like `UUID` or `VECTOR`.

| Feature | Description |
|---------|-------------|
| Automatic skip | Already-installed extensions are skipped |
| Version pinning | Optional `-- version: X.Y.Z` comment |
| Custom schema | Optional `-- schema: name` comment |
| Error handling | Clear error if extension not available on server |

**Common extensions:**
- `uuid-ossp` - UUID generation (`uuid_generate_v4()`)
- `pgcrypto` - Cryptographic functions
- `pgvector` - Vector embeddings for AI/ML
- `postgis` - Geographic data types
- `pg_trgm` - Trigram text search

### Custom Types

Define custom PostgreSQL types in the `types/` folder. Types are deployed **after** extensions but **before** migrations.

**ENUM type** (`types/order_status.pssql`):
```sql
CREATE TYPE order_status AS ENUM (
    'pending',
    'processing',
    'shipped',
    'delivered'
);
```

**Composite type** (`types/address.pssql`):
```sql
CREATE TYPE address AS (
    street TEXT,
    city TEXT,
    zip_code TEXT
);
```

**Domain type** (`types/email.pssql`):
```sql
CREATE DOMAIN email AS TEXT
CHECK (VALUE ~ '^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$');
```

| Feature | Description |
|---------|-------------|
| Checksum tracking | Unchanged types are skipped |
| Type detection | Automatically detects ENUM, composite, domain |
| Tracking table | `_stonescriptdb_gateway_types` |

**Note:** PostgreSQL ENUMs cannot be modified after creation. To change an ENUM, create a migration that creates a new type and migrates columns.

### Table Dependency Ordering

Tables are automatically ordered by foreign key dependencies using topological sort. You don't need to manually prefix files with `001_`, `002_`, etc.

```sql
-- users.pssql (no dependencies - created first)
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);

-- orders.pssql (depends on users - created after)
CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id)
);
```

The gateway analyzes `REFERENCES` constraints and ensures tables are created in the correct order.

### Schema Diff Validation

Before running migrations, the gateway compares your desired schema (from `tables/`) against the current database and classifies changes:

| Change Type | Classification | Behavior |
|-------------|---------------|----------|
| Add table | Safe | Allowed |
| Drop table | DataLoss | Blocked |
| Add nullable column | Safe | Allowed |
| Add NOT NULL column without DEFAULT | DataLoss | Blocked |
| Drop column | DataLoss | Blocked |
| Widen type (INT → BIGINT) | Safe | Allowed |
| Narrow type (BIGINT → INT) | DataLoss | Blocked |
| Incompatible type (INT → TEXT) | Incompatible | Blocked |

Use `force=true` to bypass data loss checks:

```bash
curl -X POST http://localhost:9000/migrate \
  -F "platform=myapp" \
  -F "schema=@schema.tar.gz" \
  -F "force=true"
```

The response includes detailed schema validation info:

```json
{
  "schema_validation": {
    "safe_changes": [...],
    "dataloss_changes": [...],
    "incompatible_changes": [...]
  }
}
```

### Function Deployment

Functions are tracked in `_stonescriptdb_gateway_functions` with intelligent change detection:

| Scenario | Action |
|----------|--------|
| Unchanged function | Skipped (checksum match) |
| Body changed, same signature | `CREATE OR REPLACE` |
| Signature changed (params added/removed) | `DROP` old + `CREATE` new |
| Function renamed | `DROP` old + `CREATE` new |

**Checksum normalization** prevents false positives:
- Whitespace changes (formatting) → Same checksum
- Comment changes → Same checksum
- Case changes (`BEGIN` vs `begin`) → Same checksum

This means 75 unchanged functions = 75 skipped (no SQL executed).

### Seeder Handling

Seeders behave differently on `/register` vs `/migrate`:

| Endpoint | Behavior |
|----------|----------|
| `/register` | Run seeders only if table is empty |
| `/migrate` | Validate seeders exist in database, rollback if missing |

This ensures seed data integrity - if you define a seeder for `roles` table with 3 roles, the gateway verifies all 3 exist after migration.

Example seeder (`seeders/roles.pssql`):
```sql
INSERT INTO roles (id, name) VALUES
    (1, 'admin'),
    (2, 'user'),
    (3, 'guest');
```

### Gateway Tracking Tables

The gateway creates internal tables with `_stonescriptdb_gateway_` prefix:

| Table | Purpose |
|-------|---------|
| `_stonescriptdb_gateway_migrations` | Track applied migrations (filename + checksum) |
| `_stonescriptdb_gateway_functions` | Track deployed functions (signature + checksum) |
| `_stonescriptdb_gateway_types` | Track deployed custom types (name + checksum) |

These are excluded from schema diff comparisons.

## StoneScriptPHP Integration

If using StoneScriptPHP, these CLI commands are available:

```bash
# Export schema as tar.gz
php stone schema:export

# Register with gateway on container startup
php stone gateway:register

# Hot migrate schema to running gateway
php stone gateway:migrate
```

## Documentation

- **[Quick Start Guide](./docs/QUICKSTART.md)** - Get started in 5 minutes
- **[Integration Guide](./docs/INTEGRATION.md)** - Detailed integration documentation
- **[High-Level Design](./HLD.md)** - Architecture and design decisions

## Running Tests

```bash
# Run integration tests (requires gateway running)
./tests/run-tests.sh

# Set custom gateway URL
GATEWAY_URL=http://localhost:9000 ./tests/run-tests.sh
```

## Links

- **GitHub:** https://github.com/progalaxyelabs/stonescriptdb-gateway
- **Issues:** https://github.com/progalaxyelabs/stonescriptdb-gateway/issues
- **Releases:** https://github.com/progalaxyelabs/stonescriptdb-gateway/releases

## License

MIT License - See [LICENSE](./LICENSE)
