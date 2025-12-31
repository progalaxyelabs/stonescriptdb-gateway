# Database Gateway

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

## Quick Start

### Installation from Release

```bash
# Clone specific version
git clone https://github.com/progalaxyelabs/stonescriptdb-gateway.git
cd db-gateway
git checkout v1.0.0

# Install as systemd service
sudo ./deploy/install.sh

# Configure
sudo nano /opt/db-gateway/.env

# Start
sudo systemctl start db-gateway
```

### Updating to Latest Release

```bash
cd /path/to/db-gateway
git fetch --tags
git checkout v1.1.0  # or latest version
cargo build --release
sudo cp target/release/db-gateway /opt/db-gateway/
sudo systemctl restart db-gateway
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
Platform Containers (Swarm)     DB Gateway (vm-postgres)
┌──────────────────┐           ┌──────────────────┐
│ medstoreapp-api  │──────────▶│                  │
│ instituteapp-api │  /register│  Rust Service    │
│ progalaxy-api    │  /call    │  (port 9000)     │
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

### Local Build

```bash
# Clone
git clone https://github.com/progalaxyelabs/stonescriptdb-gateway.git
cd db-gateway

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
sudo nano /opt/db-gateway/.env

# Start service
sudo systemctl start db-gateway

# Check status
sudo systemctl status db-gateway

# View logs
sudo journalctl -u db-gateway -f
```

See `deploy/` directory for installation scripts and systemd service file.

## Environment Variables

```bash
DATABASE_URL=postgres://gateway_user:password@localhost:5432/postgres
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
├── functions/          # *.pssql - CREATE OR REPLACE FUNCTION
├── migrations/         # *.pssql - Ordered: 001_xxx, 002_xxx
├── tables/            # Table definitions (reference)
└── seeders/           # Initial data (optional)
```

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

## Links

- **GitHub:** https://github.com/progalaxyelabs/stonescriptdb-gateway
- **Issues:** https://github.com/progalaxyelabs/stonescriptdb-gateway/issues
- **Releases:** https://github.com/progalaxyelabs/stonescriptdb-gateway/releases

## License

MIT License - See [LICENSE](./LICENSE)
