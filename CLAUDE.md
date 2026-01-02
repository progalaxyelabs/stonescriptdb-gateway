# StoneScriptDB Gateway - AI Agent Guide

Quick reference for Claude AI agents working on stonescriptdb-gateway.

## Project Overview

**Type:** Rust-based multi-tenant database gateway
**Language:** Rust (Cargo project)
**Port:** 9000
**Purpose:** PostgreSQL schema management and query routing for multi-tenant platforms

## Key Components

### Core Files
- `src/main.rs` - Entry point
- `Cargo.toml` - Dependencies and project metadata
- `README.md` - User documentation
- `HLD.md` - Architecture details

### Documentation
- `docs/QUICKSTART.md` - 5-minute getting started guide
- `docs/INTEGRATION.md` - Platform integration patterns
- `CLAUDE.md` - This file

### Infrastructure
- `Dockerfile.build` - Multi-stage build for Ubuntu 22.04 compatibility
- `deploy/` - systemd service files and installation scripts
- `.env.example` - Configuration template

## Core Architecture

```
Platform API → Gateway (port 9000) → PostgreSQL
```

### Three Main Endpoints
1. **POST /register** - Deploy schema on platform startup
2. **POST /migrate** - Hot update schema without restart
3. **POST /call** - Execute database functions

### Database Naming
- `{platform}_main` - Main/admin database
- `{platform}_{tenant_id}` - Tenant-specific databases

## Schema Structure

Platforms provide `postgresql/` folder (as tar.gz):
```
postgresql/
├── migrations/      # NNN_name.pssql files (ordered, run once)
├── functions/       # function_name.pssql (redeployed every time)
├── tables/          # Reference documentation
└── seeders/         # Optional seed data
```

## Development

### Building
```bash
cargo build              # Debug
cargo build --release   # Production
```

### Testing
```bash
cargo test
./tests/run-tests.sh    # Integration tests (requires running gateway)
```

### Cross-Platform Build (Docker)
```bash
cp Dockerfile.build.example Dockerfile.build
docker build -f Dockerfile.build -t stonescriptdb-gateway-builder .
docker run --rm -v "$PWD/output:/output" stonescriptdb-gateway-builder
```

### Running Locally
```bash
cp .env.example .env
# Edit .env with your PostgreSQL credentials
cargo run
```

## Environment Variables

**Required:**
- `DB_HOST` - PostgreSQL host
- `DB_PORT` - PostgreSQL port (typically 5432)
- `DB_USER` - Database user
- `DB_PASSWORD` - Database password

**Optional:**
- `GATEWAY_HOST` - Bind address (default: 127.0.0.1)
- `GATEWAY_PORT` - Bind port (default: 9000)
- `MAX_CONNECTIONS_PER_POOL` - Per-platform connections (default: 10)
- `MAX_TOTAL_CONNECTIONS` - Total pool size (default: 200)
- `ALLOWED_NETWORKS` - CIDR allowed to call gateway
- `RUST_LOG` - Log level (info, debug, trace)

## Integration Points

### StoneScriptPHP Integration
```php
php stone gateway:register    // On startup
php stone gateway:migrate     // Hot migrate
php stone schema:export       // Create tar.gz
```

### Health Check
```bash
curl http://localhost:9000/health
```

### Admin Endpoints
```bash
# List databases for platform
curl "http://localhost:9000/admin/databases?platform=myapp"

# Create new tenant database
curl -X POST http://localhost:9000/admin/create-tenant \
  -H "Content-Type: application/json" \
  -d '{"platform": "myapp", "tenant_id": "tenant_123"}'
```

## Common Tasks

**Add a new feature:**
1. Modify `src/` files
2. Run `cargo build` to check
3. Add tests in `tests/`
4. Run `cargo test`

**Fix a bug:**
1. Create minimal test case
2. Fix the issue
3. Verify tests pass

**Update dependencies:**
1. Edit `Cargo.toml`
2. Run `cargo update`
3. Verify tests pass

**Deploy new version:**
1. Tag release: `git tag v1.x.x`
2. Build with Docker: `./scripts/build.sh`
3. Use `deploy/install.sh` on server

## Key Concepts

- **Schema Migrations:** Only run once, tracked by filename + checksum
- **Functions:** Redeployed on every register/migrate (can change logic)
- **Multi-tenant:** Each tenant gets isolated database with identical schema
- **IP-based Security:** No API keys for internal Docker network
- **Connection Pooling:** Efficient resource usage across platforms

## Testing Platform Integration

```bash
# 1. Start gateway
cargo run

# 2. Create test schema
mkdir -p test-schema/postgresql/migrations
echo "CREATE TABLE IF NOT EXISTS test (id SERIAL PRIMARY KEY)" > \
  test-schema/postgresql/migrations/001_create_test.pssql

# 3. Register platform
tar -czf test-schema.tar.gz -C test-schema postgresql
curl -X POST http://localhost:9000/register \
  -F "platform=testapp" \
  -F "schema=@test-schema.tar.gz"

# 4. Call function
curl -X POST http://localhost:9000/call \
  -H "Content-Type: application/json" \
  -d '{"platform": "testapp", "function": "test_function", "params": []}'
```

## Useful Links

- **GitHub:** https://github.com/progalaxyelabs/stonescriptdb-gateway
- **Issues:** https://github.com/progalaxyelabs/stonescriptdb-gateway/issues
- **Releases:** https://github.com/progalaxyelabs/stonescriptdb-gateway/releases
- **Platform Guide:** `/ssd2/projects/progalaxy-elabs/.about/PLATFORM-DEVELOPMENT-GUIDE.md`

## File Location

This project: `/ssd2/projects/progalaxy-elabs/opensource/stonescriptdb-gateway/`
