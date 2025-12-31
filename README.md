# Database Gateway

Rust-based multi-tenant database gateway/proxy for Progalaxy E-Labs platforms.

## Overview

This service acts as a centralized database proxy for all multi-tenant platforms (medstoreapp, instituteapp, etc.). Platform APIs connect to this gateway instead of directly to PostgreSQL.

## Features

- **Tenant Routing**: Maps platform + tenant_id to correct database
- **Connection Pooling**: Shared pool across all platforms (max 200 connections)
- **Database Registry**: Tracks which databases exist per platform
- **Query Execution**: Parameterized queries with logging
- **Metrics**: Prometheus-compatible metrics

## Architecture

```
Platform APIs → DB Gateway (port 9000) → PostgreSQL (port 5432)
```

See [HLD.md](./HLD.md) for detailed architecture documentation.

## Development

```bash
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

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/metrics` | GET | Prometheus metrics |
| `/query` | POST | Execute query |
| `/admin/databases` | GET | List databases |
| `/admin/create-tenant-db` | POST | Create tenant database |

## Environment Variables

```bash
DATABASE_URL=postgres://gateway_user:password@localhost:5432/postgres
GATEWAY_PORT=9000
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200
API_KEY_MEDSTOREAPP=xxx
# ... see .env.example
```

## License

Proprietary - Progalaxy E-Labs
