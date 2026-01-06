# StoneScript DB Gateway

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub release](https://img.shields.io/github/v/release/YOUR-ORG/stonescriptdb-gateway)](https://github.com/YOUR-ORG/stonescriptdb-gateway/releases)

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

## 📚 Documentation

- ⚡ **[Quick Start Guide](docs/QUICKSTART.md)** - Get running in 5 minutes
- 🔌 **[Integration Guide](docs/INTEGRATION.md)** - Connect your platform (Docker, CI/CD, multi-tenant)
- 🏗️ **[Architecture (HLD)](HLD.md)** - Technical design & decisions
- 🛠️ **[Development Environment](docs/DEV-ENVIRONMENT.md)** - Local VM setup with libvirt
- 📡 **[API v2](docs/API-V2.md)** - Multi-tenant platform management with stored schemas

## Quick Start

### Recommended Deployment: Separate VM

The gateway is designed to run in a dedicated VM alongside PostgreSQL, separate from your Docker host. This provides:
- Clean separation of concerns
- Better network isolation
- Easier database backup and maintenance
- Avoids rootless Docker networking issues

**Setup:**

1. **Create a VM** with PostgreSQL 16 installed (e.g., using libvirt, VirtualBox, or cloud provider)
2. **Configure static IP** for the VM on a bridge network accessible from your Docker host
3. **Deploy the gateway** on the VM

```bash
# On the VM:
git clone https://github.com/YOUR-ORG/stonescriptdb-gateway.git
cd stonescriptdb-gateway
git checkout v1.0.0

# Build the binary (or use Docker build)
cargo build --release

# Install as systemd service
sudo mkdir -p /opt/stonescriptdb-gateway
sudo cp target/release/stonescriptdb-gateway /opt/stonescriptdb-gateway/
sudo cp deploy/stonescriptdb-gateway.service /etc/systemd/system/

# Configure
sudo nano /opt/stonescriptdb-gateway/.env
# Set:
#   DB_HOST=localhost (PostgreSQL on same VM)
#   GATEWAY_HOST=0.0.0.0 (listen on all interfaces)
#   ALLOWED_NETWORKS=<your-docker-host-subnet>

# Start
sudo systemctl daemon-reload
sudo systemctl enable stonescriptdb-gateway
sudo systemctl start stonescriptdb-gateway
```

4. **Configure your Docker containers** to access the gateway at `http://<VM_IP>:9000`

See [docs/DEV-ENVIRONMENT.md](./docs/DEV-ENVIRONMENT.md) for detailed VM setup instructions.

### Alternative: Local Development

For local development and testing, you can run the gateway directly:

```bash
# Clone
git clone https://github.com/YOUR-ORG/stonescriptdb-gateway.git
cd stonescriptdb-gateway

# Setup
cargo build
cp .env.example .env
# Edit .env with your PostgreSQL credentials

# Run
cargo run
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
Docker Host                     Database VM (e.g., <VM_IP>)
┌──────────────────┐           ┌──────────────────────────────────┐
│ Platform Containers          │  StoneScriptDB Gateway           │
│                  │           │  ┌────────────────────────────┐  │
│ platform-a-api   │──────────▶│  │ Rust Service (port 9000)   │  │
│ platform-b-api   │  HTTP     │  │ /register, /migrate, /call │  │
│ platform-c-api   │           │  └────────────┬───────────────┘  │
└──────────────────┘           │               │                  │
                               │               ▼                  │
                               │  ┌────────────────────────────┐  │
                               │  │ PostgreSQL 16 (port 5432)  │  │
                               │  │                            │  │
                               │  │ *_main DBs                 │  │
                               │  │ *_tenant DBs               │  │
                               │  └────────────────────────────┘  │
                               └──────────────────────────────────┘
```

See [HLD.md](./HLD.md) for detailed architecture documentation.

## API Endpoints

### Legacy Endpoints (v1 - Upload schema each time)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/register` | POST | Deploy schema + create database (multipart: platform, schema.tar.gz) |
| `/migrate` | POST | Deploy schema to existing databases (multipart: platform, schema.tar.gz) |
| `/call` | POST | Execute database function |
| `/health` | GET | Health check |
| `/admin/databases` | GET | List databases for a platform |
| `/admin/create-tenant` | POST | Create new tenant database |

### Platform Management Endpoints (v2 - Stored schemas)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/platform/register` | POST | Register platform (JSON: `{platform}`) |
| `/platform/{platform}/schema` | POST | Upload schema (multipart: schema_name, schema.tar.gz) |
| `/platform/{platform}/schemas` | GET | List registered schemas |
| `/platform/{platform}/databases` | GET | List created databases |
| `/platforms` | GET | List all platforms with schema/database counts |
| `/database/create` | POST | Create database from stored schema (JSON) |
| `/v2/migrate` | POST | Migrate using stored schemas (JSON) |

**Note:** The `/platforms` endpoint reads from the file-based platform registry (persisted to disk), not in-memory connection pools. Per-database deployment tracking (migrations, functions, types) is stored in PostgreSQL tables with checksums to skip unchanged deployments.

See [docs/API-V2.md](./docs/API-V2.md) for detailed v2 API documentation.

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

### Deploy to VM

```bash
# Upload binary to VM
scp output/stonescriptdb-gateway user@vm-ip:/tmp/

# Install on VM
ssh user@vm-ip
sudo mkdir -p /opt/stonescriptdb-gateway
sudo cp /tmp/stonescriptdb-gateway /opt/stonescriptdb-gateway/
sudo cp /path/to/deploy/stonescriptdb-gateway.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl restart stonescriptdb-gateway
```

### Local Development (requires Rust installed)

```bash
# Clone
git clone https://github.com/YOUR-ORG/stonescriptdb-gateway.git
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

### Production Deployment on VM

```bash
# On the VM with PostgreSQL installed:

# Create installation directory
sudo mkdir -p /opt/stonescriptdb-gateway
sudo mkdir -p /var/log/stonescriptdb-gateway

# Copy binary
sudo cp target/release/stonescriptdb-gateway /opt/stonescriptdb-gateway/

# Install systemd service
sudo cp deploy/stonescriptdb-gateway.service /etc/systemd/system/

# Configure
sudo nano /opt/stonescriptdb-gateway/.env

# Start service
sudo systemctl daemon-reload
sudo systemctl enable stonescriptdb-gateway
sudo systemctl start stonescriptdb-gateway

# Check status
sudo systemctl status stonescriptdb-gateway

# View logs
sudo journalctl -u stonescriptdb-gateway -f
```

See `deploy/` directory for the systemd service file.

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

GATEWAY_HOST=0.0.0.0
GATEWAY_PORT=9000
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200
POOL_IDLE_TIMEOUT_SECS=1800
ALLOWED_NETWORKS=127.0.0.0/8,192.168.1.100/24
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

The gateway creates internal tables with `_stonescriptdb_gateway_` prefix **in each database**:

| Table | Purpose |
|-------|---------|
| `_stonescriptdb_gateway_migrations` | Track applied migrations (filename + checksum) |
| `_stonescriptdb_gateway_types` | Track deployed custom types (name + checksum) |
| `_stonescriptdb_gateway_tables` | Track deployed tables (name + checksum) |
| `_stonescriptdb_gateway_functions` | Track deployed functions (signature + checksum) |
| `_stonescriptdb_gateway_changelog` | Audit trail of all schema changes (migrations, functions, types, tables) |

**How it works:**
- Each database gets its own tracking tables (not shared across platforms)
- Checksums are used to detect changes and skip re-deploying unchanged items
- On `/register` or `/migrate`, the gateway compares incoming checksums with stored checksums
- Only changed items are re-deployed (e.g., 75 unchanged functions = 75 skipped)
- The `_changelog` table records all changes with timestamps for auditing
- These tables are excluded from schema diff comparisons

**Platform registry (v2 API):**
- The `/platforms` endpoint reads from a **file-based registry** (`data_dir/<platform>/platform.json`)
- This is separate from per-database tracking tables
- Used for managing registered platforms, schemas, and databases

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
- **[Integration Guide](./docs/INTEGRATION.md)** - Detailed integration documentation (v1 API)
- **[API v2 Guide](./docs/API-V2.md)** - Multi-tenant platform management with stored schemas
- **[High-Level Design](./HLD.md)** - Architecture and design decisions

## Running Tests

```bash
# Run integration tests (requires gateway running)
./tests/run-tests.sh

# Set custom gateway URL
GATEWAY_URL=http://localhost:9000 ./tests/run-tests.sh
```

## Links

- **GitHub:** https://github.com/YOUR-ORG/stonescriptdb-gateway
- **Issues:** https://github.com/YOUR-ORG/stonescriptdb-gateway/issues
- **Releases:** https://github.com/YOUR-ORG/stonescriptdb-gateway/releases

## License

MIT License - See [LICENSE](./LICENSE)
