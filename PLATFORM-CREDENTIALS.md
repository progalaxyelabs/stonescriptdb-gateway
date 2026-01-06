# Platform Credential Isolation

## Overview

StoneScriptDB Gateway now supports **per-platform PostgreSQL credentials** to provide database-level isolation between platforms. This prevents one platform from accessing another platform's databases.

## Security Problem

**Before this feature:**
- All platforms shared the same PostgreSQL user credentials
- Platform A could deploy malicious functions that query Platform B's databases
- Example attack:
  ```sql
  -- Platform A deploys this function
  CREATE FUNCTION steal_data() RETURNS TEXT AS $$
  BEGIN
    RETURN (SELECT string_agg(email, ',') FROM platformB_main.users);
  END;
  $$ LANGUAGE plpgsql;
  ```
- No PostgreSQL-level access control

**After this feature:**
- Each platform can have dedicated PostgreSQL credentials
- PostgreSQL `GRANT` permissions enforce isolation
- Platform A's user cannot access Platform B's databases
- Isolation happens at the database engine level, not just logical naming

## How It Works

### 1. Platform Registration with Credentials

When registering a platform, you can now provide PostgreSQL credentials:

```bash
POST /platform/register
{
  "platform": "myplatform",
  "db_user": "myplatform_user",      # Optional
  "db_password": "secure_password"    # Optional
}
```

**Response:**
```json
{
  "status": "registered",
  "platform": "myplatform",
  "has_dedicated_credentials": true,
  "message": "Platform registered with dedicated PostgreSQL credentials. Database isolation enabled."
}
```

### 2. Credential Storage

Credentials are stored in `data/{platform}/platform.json`:

```json
{
  "name": "myplatform",
  "registered_at": "2026-01-06T...",
  "schemas": [],
  "databases": {},
  "db_user": "myplatform_user",
  "db_password": "secure_password"
}
```

**Security Note:** In production, encrypt `platform.json` files at rest or use a secrets manager.

### 3. Connection Pool Creation

When the gateway creates a connection pool for a database:

1. Extract platform name from database name (`platform_main` → `platform`)
2. Load platform credentials from `platform.json`
3. Build connection URL with platform-specific credentials:
   ```
   postgres://myplatform_user:secure_password@host:port/myplatform_main
   ```
4. If no credentials found, fall back to default gateway credentials

**Code:** [src/pool/manager.rs:119-166](src/pool/manager.rs#L119-L166)

### 4. PostgreSQL Setup

For each platform, create a PostgreSQL user and grant appropriate permissions:

```sql
-- 1. Create platform-specific user
CREATE USER myplatform_user WITH PASSWORD 'secure_password';

-- 2. Grant CONNECT to postgres database (for admin operations)
GRANT CONNECT ON DATABASE postgres TO myplatform_user;

-- 3. Grant ALL on platform's databases
GRANT ALL PRIVILEGES ON DATABASE myplatform_main TO myplatform_user;
GRANT ALL PRIVILEGES ON DATABASE myplatform_tenant1 TO myplatform_user;

-- 4. Revoke access to other platforms' databases
REVOKE ALL ON DATABASE otherplatform_main FROM myplatform_user;
```

**Important:** The gateway still needs a superuser/admin account for:
- Creating new databases (`CREATE DATABASE`)
- Dropping databases (`DROP DATABASE`)
- Administrative operations

## Setup Guide

### Step 1: Create PostgreSQL Users

For each platform you want to isolate:

```bash
# Connect as PostgreSQL superuser
psql -U postgres

# Create platform user
CREATE USER platformA_user WITH PASSWORD 'strong_password_A';
CREATE USER platformB_user WITH PASSWORD 'strong_password_B';

# Grant connection rights
GRANT CONNECT ON DATABASE postgres TO platformA_user;
GRANT CONNECT ON DATABASE postgres TO platformB_user;
```

### Step 2: Register Platform with Credentials

```bash
curl -X POST http://gateway:9000/platform/register \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "platformA",
    "db_user": "platformA_user",
    "db_password": "strong_password_A"
  }'
```

### Step 3: Register Schema and Create Database

```bash
# Register schema (multipart upload)
curl -X POST http://gateway:9000/platform/platformA/schema \
  -F "schema_name=main" \
  -F "schema=@schema.zip"

# Register database (creates platformA_main)
curl -X POST http://gateway:9000/register \
  -F "platform=platformA" \
  -F "schema=@schema.zip"
```

### Step 4: Grant Database Permissions

After the database is created, grant permissions:

```sql
-- Connect as superuser
psql -U postgres

-- Grant all privileges on the new database
GRANT ALL PRIVILEGES ON DATABASE platformA_main TO platformA_user;

-- For tenant databases (when created)
GRANT ALL PRIVILEGES ON DATABASE platformA_tenant123 TO platformA_user;
```

### Step 5: Verify Isolation

Test that Platform A cannot access Platform B's data:

```bash
# Try to connect as platformA_user to platformB_main
psql -U platformA_user -d platformB_main

# Should fail with:
# FATAL: permission denied for database "platformB_main"
```

## Backward Compatibility

**Platforms without credentials:** Continue using default gateway credentials

```bash
# Register without credentials (legacy mode)
POST /platform/register
{
  "platform": "legacyplatform"
}
```

**Response:**
```json
{
  "status": "registered",
  "platform": "legacyplatform",
  "has_dedicated_credentials": false,
  "message": "Platform registered using default gateway credentials. For better security, provide db_user and db_password."
}
```

## Database User Permissions Matrix

| Database          | Gateway Admin | Platform A User | Platform B User |
|-------------------|---------------|-----------------|-----------------|
| postgres          | ALL           | CONNECT         | CONNECT         |
| platformA_main    | ALL           | ALL             | ❌ NONE         |
| platformA_tenant1 | ALL           | ALL             | ❌ NONE         |
| platformB_main    | ALL           | ❌ NONE         | ALL             |
| platformB_tenant1 | ALL           | ❌ NONE         | ALL             |

## Automated Permission Management

For convenience, create a helper script to grant permissions:

```bash
#!/bin/bash
# grant-platform-permissions.sh

PLATFORM=$1
DB_USER="${PLATFORM}_user"

# Get all databases for this platform
DATABASES=$(psql -U postgres -t -c "SELECT datname FROM pg_database WHERE datname LIKE '${PLATFORM}_%'")

for DB in $DATABASES; do
    echo "Granting permissions on $DB to $DB_USER..."
    psql -U postgres -c "GRANT ALL PRIVILEGES ON DATABASE $DB TO $DB_USER"
done

echo "Done!"
```

Usage:
```bash
./grant-platform-permissions.sh platformA
```

## Migration Guide

### Migrating Existing Platforms to Credential Isolation

1. **Create PostgreSQL user** for the platform
2. **Grant permissions** on existing databases
3. **Update platform.json** manually or via API
4. **Restart gateway** to pick up new credentials

Example:
```bash
# 1. Create user
psql -U postgres -c "CREATE USER oldplatform_user WITH PASSWORD 'newpass'"

# 2. Grant permissions
psql -U postgres -c "GRANT ALL ON DATABASE oldplatform_main TO oldplatform_user"

# 3. Update platform.json
vi /opt/stonescriptdb-gateway/schemas/oldplatform/platform.json
# Add:
#   "db_user": "oldplatform_user",
#   "db_password": "newpass"

# 4. Restart gateway
sudo systemctl restart stonescriptdb-gateway
```

## Security Best Practices

1. **Use strong passwords:** Generate with `openssl rand -base64 32`
2. **Encrypt platform.json:** Use filesystem encryption or secrets manager
3. **Rotate credentials:** Periodically update passwords
4. **Least privilege:** Only grant necessary permissions
5. **Audit access:** Monitor PostgreSQL logs for unauthorized access attempts
6. **Separate admin user:** Don't use platform users for CREATE/DROP DATABASE

## Troubleshooting

### Connection Fails After Adding Credentials

**Error:** `FATAL: password authentication failed for user "platform_user"`

**Solution:**
- Verify credentials in `platform.json`
- Check PostgreSQL `pg_hba.conf` allows password authentication
- Ensure user exists: `psql -U postgres -c "\du"`

### Platform Can't Access Its Own Database

**Error:** `FATAL: permission denied for database "platform_main"`

**Solution:**
```sql
-- Grant permissions
GRANT ALL PRIVILEGES ON DATABASE platform_main TO platform_user;
```

### Other Platforms Can Still Access Data

**Issue:** Cross-platform access still works

**Diagnosis:**
- Check if platforms have credentials configured
- Verify PostgreSQL permissions: `psql -U postgres -c "\l"`
- Test connection: `psql -U platformA_user -d platformB_main`

**Solution:**
- Explicitly revoke cross-platform access:
  ```sql
  REVOKE ALL ON DATABASE platformB_main FROM platformA_user;
  ```

## API Reference

### POST /platform/register

Register a new platform with optional credentials.

**Request:**
```json
{
  "platform": "string (required)",
  "db_user": "string (optional)",
  "db_password": "string (optional)"
}
```

**Response:**
```json
{
  "status": "registered",
  "platform": "string",
  "has_dedicated_credentials": boolean,
  "message": "string"
}
```

**Status Codes:**
- `201 Created` - Platform registered successfully
- `400 Bad Request` - Invalid platform name or empty credentials
- `409 Conflict` - Platform already exists

## Implementation Details

### Files Modified

1. **src/registry/platform.rs**
   - Added `db_user` and `db_password` fields to `PlatformInfo`
   - Added `with_credentials()` constructor

2. **src/api/platform.rs**
   - Updated `RegisterPlatformRequest` to accept credentials
   - Modified response to indicate credential status

3. **src/pool/manager.rs**
   - Added `data_dir` field for loading platform info
   - Implemented `database_url_for()` to use platform-specific credentials
   - Added `database_url_with_default_creds()` fallback

### Connection URL Construction

**With platform credentials:**
```
postgres://platformA_user:password@localhost:5432/platformA_main
```

**Without credentials (legacy):**
```
postgres://gateway_user:password@localhost:5432/platformA_main
```

### Credential Lookup Flow

```
Database Request (platformA_main)
  ↓
Extract platform name ("platformA")
  ↓
Load platform.json from data/platformA/platform.json
  ↓
Check for db_user and db_password
  ↓
┌─────────────────────────────────────┐
│ Credentials Found?                   │
├─────────────────┬───────────────────┤
│ YES             │ NO                │
│ Use platform    │ Use default       │
│ credentials     │ gateway creds     │
└─────────────────┴───────────────────┘
  ↓
Build connection URL
  ↓
Create/reuse connection pool
```

## Future Enhancements

1. **Encrypted credential storage** using HashiCorp Vault or AWS Secrets Manager
2. **Automatic permission grants** when creating new tenant databases
3. **Credential rotation** with zero-downtime
4. **Role-based access control** for read-only vs read-write access
5. **Audit logging** of all cross-platform access attempts

## Related Documentation

- [Multi-Tenant Architecture](ARCHITECTURE.md#multi-tenant)
- [Security Model](SECURITY.md)
- [PostgreSQL Configuration](POSTGRESQL.md)
