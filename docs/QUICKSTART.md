# Quick Start Guide

## For Platform Developers

### 1. Create Your Schema Structure

```bash
mkdir -p src/postgresql/{extensions,tables,migrations,functions,seeders}
```

### 2. Add Extensions (Optional)

```sql
-- src/postgresql/extensions/uuid-ossp.sql
-- UUID generation functions
```

### 3. Add Table Definitions

```sql
-- src/postgresql/tables/users.pssql
CREATE TABLE users (
    user_id SERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(100) NOT NULL,
    created_on TIMESTAMPTZ DEFAULT NOW()
);
```

### 4. Add Your First Migration

```sql
-- src/postgresql/migrations/001_initial.pssql
CREATE TABLE IF NOT EXISTS users (
    user_id SERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(100) NOT NULL,
    created_on TIMESTAMPTZ DEFAULT NOW()
);
```

### 5. Add Your First Function

```sql
-- src/postgresql/functions/get_user_by_id.pssql
CREATE OR REPLACE FUNCTION get_user_by_id(p_user_id INT)
RETURNS TABLE (user_id INT, email TEXT, name TEXT) AS $$
BEGIN
    RETURN QUERY
    SELECT u.user_id, u.email::TEXT, u.name::TEXT
    FROM users u WHERE u.user_id = p_user_id;
END;
$$ LANGUAGE plpgsql;
```

### 6. Register with Gateway

```bash
# Create archive
tar -czf schema.tar.gz -C src postgresql

# Register
curl -X POST http://localhost:9000/register \
  -F "platform=myapp" \
  -F "schema=@schema.tar.gz"
```

### 7. Call Your Function

```bash
curl -X POST http://localhost:9000/call \
  -H "Content-Type: application/json" \
  -d '{"platform": "myapp", "function": "get_user_by_id", "params": [1]}'
```

---

## Folder Structure Explained

```
postgresql/
├── extensions/    # PostgreSQL extensions (uuid-ossp, pgvector)
├── types/         # Custom types (ENUM, composite, domain)
├── tables/        # Declarative schema (for validation)
├── migrations/    # DDL changes (run once, tracked)
├── functions/     # Stored functions (redeployed on changes)
└── seeders/       # Initial data (run on empty tables)
```

**Processing order:** Extensions → Types → Tables (validate) → Migrations → Functions → Seeders

---

## For StoneScriptPHP-Server Users

Just add to your `.env`:

```env
DB_GATEWAY_URL=http://localhost:9000
PLATFORM_ID=myapp
```

The server handles everything automatically on startup.

---

## Updating Schema

### Add New Migration

```sql
-- src/postgresql/migrations/002_add_posts.pssql
CREATE TABLE IF NOT EXISTS posts (...);
```

### Deploy Changes

```bash
tar -czf schema.tar.gz -C src postgresql
curl -X POST http://localhost:9000/migrate \
  -F "platform=myapp" \
  -F "schema=@schema.tar.gz"
```

Functions are always redeployed. Migrations only run if new files are added.
