# StoneScriptDB Gateway Integration Guide

---
**📖 Navigation:** [Home](../README.md) | [Quick Start](QUICKSTART.md) | **Integration** | [HLD](../HLD.md) | [Dev Setup](DEV-ENVIRONMENT.md) | [API v2](API-V2.md)

---

## Overview

This guide focuses on **how to integrate** your platform with StoneScriptDB Gateway. For detailed schema structure and features, see the [main README](../README.md).

**Gateway capabilities:**
- Database creation per platform/tenant
- PostgreSQL extension installation
- Schema migrations with validation
- Intelligent function deployment (checksum-based)
- Seed data management
- Query routing

**Related Documentation:**
- 📖 [README: Schema Structure](../README.md#schema-tar-gz-structure) - postgresql/ folder structure, extensions, types
- 📖 [README: Gateway Tracking Tables](../README.md#gateway-tracking-tables) - How checksums work
- ⚡ [Quick Start](QUICKSTART.md) - Get running in 5 minutes
- 🔌 [API v2](API-V2.md) - Multi-tenant platform management

---

## Architecture

```
Docker Host / Dev Machine              Database VM (e.g., <VM_IP>)
┌──────────────────────────────┐     ┌────────────────────────────────────┐
│  Platform API                │     │  StoneScriptDB Gateway             │
│  (Docker container)          │────▶│  ┌──────────────────────────────┐  │
│  stonescriptphp-server       │HTTP │  │ Rust Service (port 9000)     │  │
│  or any HTTP client          │     │  │                              │  │
└──────────────────────────────┘     │  │ POST /register, /migrate     │  │
                                     │  │ POST /call                   │  │
                                     │  └──────────┬───────────────────┘  │
                                     │             │                      │
                                     │             ▼                      │
                                     │  ┌──────────────────────────────┐  │
                                     │  │ PostgreSQL 16 (port 5432)    │  │
                                     │  │                              │  │
                                     │  │ {platform}_main              │  │
                                     │  │ {platform}_{tenant_id}       │  │
                                     │  └──────────────────────────────┘  │
                                     └────────────────────────────────────┘
```

**Network Setup:**
- Gateway VM has a static IP accessible from Docker host
- Docker containers connect to gateway via VM IP (e.g., `http://<VM_IP>:9000`)
- Gateway connects to PostgreSQL on localhost (same VM)

---

## Integration Options

### Option 1: StoneScriptPHP-Server Integration (Recommended)

For platforms using `stonescriptphp-server`, the server handles gateway communication automatically.

**Configuration in platform's `.env`:**
```env
DB_GATEWAY_URL=http://<VM_IP>:9000  # Replace with your VM IP
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
│       ├── extensions/      # PostgreSQL extensions
│       ├── types/           # Custom types (ENUM, composite, domain)
│       ├── tables/          # Declarative table definitions
│       ├── migrations/      # DDL migrations
│       ├── functions/       # Stored functions
│       └── seeders/         # Seed data
└── ...
```

For details on each folder, see [README: Schema Structure](../README.md#schema-tar-gz-structure).

---

### Option 2: Direct HTTP Integration

For non-StoneScriptPHP platforms or custom integrations.

**Register/Update Schema:**
```bash
# Create tar.gz of postgresql folder
tar -czf schema.tar.gz -C src postgresql

# Register with gateway (replace with your VM IP)
curl -X POST http://<VM_IP>:9000/register \
  -F "platform=myplatform" \
  -F "schema=@schema.tar.gz"

# For tenant-specific database
curl -X POST http://<VM_IP>:9000/register \
  -F "platform=myplatform" \
  -F "tenant_id=tenant_001" \
  -F "schema=@schema.tar.gz"
```

**Call Functions:**
```bash
# No parameters (replace with your VM IP)
curl -X POST http://<VM_IP>:9000/call \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myplatform",
    "function": "get_all_users",
    "params": []
  }'

# With parameters
curl -X POST http://<VM_IP>:9000/call \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myplatform",
    "function": "get_user_by_id",
    "params": [123]
  }'

# Tenant-specific call
curl -X POST http://<VM_IP>:9000/call \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myplatform",
    "tenant_id": "tenant_001",
    "function": "get_orders",
    "params": [1, 10]
  }'
```

---

### Option 3: Use Provided Scripts

Copy the scripts from gateway repo to your platform:

```bash
# Copy scripts
cp /path/to/stonescriptdb-gateway/scripts/register-with-gateway.sh ./scripts/
cp /path/to/stonescriptdb-gateway/scripts/migrate-schema.sh ./scripts/

# Register platform
DB_GATEWAY_URL=http://<VM_IP>:9000 \
PLATFORM_ID=myplatform \
./scripts/register-with-gateway.sh

# Hot migrate (update functions without restart)
DB_GATEWAY_URL=http://<VM_IP>:9000 \
PLATFORM_ID=myplatform \
./scripts/migrate-schema.sh
```

---

## Schema Management Workflow

For detailed schema structure, see [README: Schema Structure](../README.md#schema-tar-gz-structure).

### Quick Reference

```
postgresql/
├── extensions/   → PostgreSQL extensions (uuid-ossp, pgvector, postgis)
├── types/        → Custom types (ENUM, composite, domain)
├── tables/       → Declarative table definitions (for validation)
├── migrations/   → DDL migrations (run once, tracked by checksum)
├── functions/    → Stored functions (redeployed on changes)
└── seeders/      → Seed data (run on empty tables)
```

**Processing order:** Extensions → Types → Tables (validate) → Migrations → Functions → Seeders

**For detailed examples:**
- [PostgreSQL Extensions](../README.md#postgresql-extensions) - uuid-ossp, pgvector, postgis
- [Custom Types](../README.md#custom-types) - ENUM, composite, domain types
- [Function Deployment](../README.md#function-deployment) - Checksum tracking & orphan cleanup
- [Gateway Tracking Tables](../README.md#gateway-tracking-tables) - How checksums skip unchanged deployments

---

## Development Workflow

### Initial Setup

1. **Create your schema:**
   ```bash
   mkdir -p src/postgresql/{extensions,types,tables,migrations,functions,seeders}
   ```

2. **Add extensions** (if needed):
   ```sql
   -- src/postgresql/extensions/uuid-ossp.sql
   -- UUID generation functions
   ```

3. **Add functions:**
   ```sql
   -- functions/get_something.pssql
   CREATE OR REPLACE FUNCTION get_something(...) ...
   ```

4. **Register with gateway:**
   ```bash
   DB_GATEWAY_URL=http://<VM_IP>:9000 \
   PLATFORM_ID=myapp \
   ./scripts/register-with-gateway.sh
   ```

### Adding New Features

1. **Add migration file** (if schema changes needed):
   ```sql
   -- migrations/005_add_comments.pssql
   CREATE TABLE IF NOT EXISTS comments (...);
   ```

2. **Add function:**
   ```sql
   -- functions/create_comment.pssql
   CREATE OR REPLACE FUNCTION create_comment(...) ...
   ```

3. **Deploy changes:**
   ```bash
   ./scripts/migrate-schema.sh
   ```

**What happens:**
- New migrations run (005_add_comments.pssql)
- All functions redeployed (unchanged = skipped via checksum)
- Seeders validated (if present)

### Updating Functions Only (Hot Deploy)

To update function logic without running migrations:

```bash
# Edit function
vim src/postgresql/functions/get_user.pssql

# Deploy
./scripts/migrate-schema.sh
```

Only changed functions are redeployed (checksum-based detection).

---

## Multi-Tenant Setup

### Per-Tenant Databases

Each tenant gets an isolated database with identical schema:

```
myplatform_main         # Main/admin database
myplatform_tenant_001   # Tenant 001
myplatform_tenant_002   # Tenant 002
```

### Registering Tenants

```bash
# Register new tenant
curl -X POST http://<VM_IP>:9000/register \
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
curl -X POST http://<VM_IP>:9000/migrate \
  -F "platform=myplatform" \
  -F "schema=@schema.tar.gz"
```

---

## Docker Compose Integration

### Basic Setup

```yaml
version: '3.8'

services:
  myapp:
    image: myapp:latest
    environment:
      - DB_GATEWAY_URL=http://<VM_IP>:9000
      - PLATFORM_ID=myapp
    networks:
      - default
```

### With Health Check

```yaml
services:
  myapp:
    image: myapp:latest
    environment:
      - DB_GATEWAY_URL=http://<VM_IP>:9000
      - PLATFORM_ID=myapp
    healthcheck:
      test: ["CMD", "curl", "-f", "http://<VM_IP>:9000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
```

### Multi-Service Stack

```yaml
services:
  api:
    image: myapp-api:latest
    environment:
      - DB_GATEWAY_URL=http://<VM_IP>:9000
      - PLATFORM_ID=myapp

  worker:
    image: myapp-worker:latest
    environment:
      - DB_GATEWAY_URL=http://<VM_IP>:9000
      - PLATFORM_ID=myapp

  admin:
    image: myapp-admin:latest
    environment:
      - DB_GATEWAY_URL=http://<VM_IP>:9000
      - PLATFORM_ID=myapp_admin
```

---

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Deploy Schema

on:
  push:
    paths:
      - 'src/postgresql/**'

jobs:
  deploy-schema:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Create schema archive
        run: tar -czf schema.tar.gz -C src postgresql

      - name: Deploy to gateway
        env:
          GATEWAY_URL: ${{ secrets.GATEWAY_URL }}
        run: |
          curl -X POST ${GATEWAY_URL}/migrate \
            -F "platform=myapp" \
            -F "schema=@schema.tar.gz"
```

### GitLab CI Example

```yaml
deploy_schema:
  stage: deploy
  only:
    changes:
      - src/postgresql/**
  script:
    - tar -czf schema.tar.gz -C src postgresql
    - |
      curl -X POST ${GATEWAY_URL}/migrate \
        -F "platform=${CI_PROJECT_NAME}" \
        -F "schema=@schema.tar.gz"
```

---

## Register vs Migrate

### `/register` - First Time Setup

Use when:
- Creating database for the first time
- Setting up a new tenant

What it does:
- Creates database if doesn't exist
- Runs ALL migrations
- Deploys ALL functions
- Runs seeders (if table is empty)

### `/migrate` - Update Existing

Use when:
- Updating schema on existing databases
- Hot-deploying function changes

What it does:
- Runs NEW migrations only (tracked by checksum)
- Redeploys changed functions (checksum-based)
- Validates seeders exist (doesn't re-run)

---

## API Quick Reference

For full API documentation, see [README: API Endpoints](../README.md#api-endpoints).

### Key Endpoints

| Endpoint | Method | Use Case |
|----------|--------|----------|
| `/register` | POST | Create new database + deploy schema |
| `/migrate` | POST | Update existing databases |
| `/call` | POST | Execute database function |
| `/health` | GET | Check gateway status |
| `/admin/databases` | GET | List databases for platform |
| `/admin/create-tenant` | POST | Create tenant database |

### POST /call

```json
{
  "platform": "myapp",
  "tenant_id": "tenant_001",  // optional
  "function": "get_user_by_id",
  "params": [123]
}
```

**Response:**
```json
{
  "rows": [
    {"user_id": 123, "email": "user@example.com", "name": "John Doe"}
  ],
  "row_count": 1
}
```

---

## Troubleshooting

### Common Issues

**Gateway unreachable from container:**
```bash
# Test connectivity
docker run --rm alpine sh -c "apk add curl && curl -v http://<VM_IP>:9000/health"
```

**Schema not deploying:**
```bash
# Check logs on gateway VM
ssh <VM_IP> "sudo journalctl -u stonescriptdb-gateway -n 100"
```

**Function call fails:**
```bash
# Verify function exists
curl "http://<VM_IP>:9000/admin/databases?platform=myapp"
```

### Viewing Logs

**On Gateway VM:**
```bash
# journalctl logs
sudo journalctl -u stonescriptdb-gateway -f

# File logs
tail -f /var/log/stonescriptdb-gateway/stonescriptdb-gateway.log
```

### Testing Connection

```bash
curl http://<VM_IP>:9000/health
```

---

## Best Practices

1. **Version your migrations:** Use sequential numbering (001, 002, ...)
2. **Keep functions focused:** One function per file, single responsibility
3. **Test migrations locally:** Use `/register` with test data before production
4. **Use seeders for reference data:** Roles, statuses, default configs
5. **Monitor gateway logs:** Set up log aggregation for production
6. **Use v2 API for multi-tenant:** Store schemas once, create databases on demand
7. **Implement health checks:** Ensure gateway is reachable before starting your app
8. **Use environment variables:** Don't hardcode `DB_GATEWAY_URL`

---

## Related Documentation

- 📖 **[Main README](../README.md)** - Features, schema structure, tracking tables
- ⚡ **[Quick Start](QUICKSTART.md)** - Get running in 5 minutes
- 🏗️ **[Architecture (HLD)](../HLD.md)** - Technical design decisions
- 🛠️ **[Dev Environment](DEV-ENVIRONMENT.md)** - Local VM setup with libvirt
- 📡 **[API v2](API-V2.md)** - Multi-tenant platform management

---

**Questions or Issues?**
- GitHub: https://github.com/YOUR-ORG/stonescriptdb-gateway/issues
- Documentation: https://github.com/YOUR-ORG/stonescriptdb-gateway
