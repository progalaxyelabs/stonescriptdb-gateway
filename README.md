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
