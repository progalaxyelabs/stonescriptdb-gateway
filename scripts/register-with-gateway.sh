#!/bin/bash
# =============================================================================
# Register Platform with StoneScriptDB Gateway (V2 API)
# =============================================================================
# Three-step registration flow:
#   1. Register platform (idempotent)
#   2. Upload schema archive
#   3. Create database from stored schema
#
# Usage:
#   ./register-with-gateway.sh [options]
#
# Environment variables (required):
#   DB_GATEWAY_URL    - Gateway URL (e.g., http://gateway:9000)
#   PLATFORM_ID       - Platform identifier (e.g., myapp)
#   ADMIN_TOKEN       - Admin token for /admin/* endpoints
#   SCHEMA_NAME       - Schema version name (e.g., v1.0, latest)
#
# Environment variables (optional):
#   DATABASE_ID       - Database identifier (default: main)
#   POSTGRESQL_PATH   - Path to postgresql folder (default: ./src/postgresql)
#   CACHE_DIR         - Temp directory for tar files (default: ./.cache)
#
# Example:
#   DB_GATEWAY_URL=http://gateway:9000 PLATFORM_ID=myapp ADMIN_TOKEN=abc123 \
#     SCHEMA_NAME=v1.0 ./register-with-gateway.sh
# =============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Configuration with defaults
DB_GATEWAY_URL="${DB_GATEWAY_URL:-}"
PLATFORM_ID="${PLATFORM_ID:-}"
ADMIN_TOKEN="${ADMIN_TOKEN:-}"
SCHEMA_NAME="${SCHEMA_NAME:-}"
DATABASE_ID="${DATABASE_ID:-main}"
POSTGRESQL_PATH="${POSTGRESQL_PATH:-./src/postgresql}"
CACHE_DIR="${CACHE_DIR:-./.cache}"
RETRY_COUNT="${RETRY_COUNT:-3}"
RETRY_DELAY="${RETRY_DELAY:-5}"

# Validate required environment variables
for VAR in DB_GATEWAY_URL PLATFORM_ID ADMIN_TOKEN SCHEMA_NAME; do
    if [ -z "${!VAR}" ]; then
        echo -e "${RED}ERROR: $VAR environment variable is required${NC}"
        exit 1
    fi
done

# Check postgresql folder exists
if [ ! -d "$POSTGRESQL_PATH" ]; then
    echo -e "${RED}ERROR: PostgreSQL folder not found at: $POSTGRESQL_PATH${NC}"
    exit 1
fi

# Create cache directory
mkdir -p "$CACHE_DIR"

# Generate timestamped filename
TIMESTAMP=$(date +%Y%m%d_%H%M%S_%N)
TAR_FILE="${CACHE_DIR}/postgresql_${PLATFORM_ID}_${TIMESTAMP}.tar.gz"

# Cleanup function
cleanup() {
    if [ -f "$TAR_FILE" ]; then
        rm -f "$TAR_FILE"
    fi
}
trap cleanup EXIT

echo -e "${GREEN}=== StoneScriptDB Gateway Registration (V2) ===${NC}"
echo "Platform:    $PLATFORM_ID"
echo "Schema:      $SCHEMA_NAME"
echo "Database ID: $DATABASE_ID"
echo "Gateway:     $DB_GATEWAY_URL"
echo ""

# ─── Step 1: Register platform (idempotent) ──────────────────────────────────
echo -e "${YELLOW}Step 1/3: Registering platform...${NC}"
RESPONSE=$(curl -s -w '\n%{http_code}' -X POST "${DB_GATEWAY_URL}/platform/register" \
    -H "Content-Type: application/json" \
    -d "{\"platform\": \"${PLATFORM_ID}\"}" 2>&1) || true

HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
BODY=$(echo "$RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "409" ]; then
    echo -e "${GREEN}  Platform registered (or already exists)${NC}"
else
    echo -e "${RED}  Failed to register platform (HTTP $HTTP_CODE)${NC}"
    echo "  $BODY"
    exit 1
fi

# ─── Step 2: Upload schema archive ───────────────────────────────────────────
echo -e "${YELLOW}Step 2/3: Uploading schema...${NC}"

# Create tar.gz
tar -czf "$TAR_FILE" -C "$(dirname "$POSTGRESQL_PATH")" "$(basename "$POSTGRESQL_PATH")"
TAR_SIZE=$(du -h "$TAR_FILE" | cut -f1)
echo "  Archive: $TAR_FILE ($TAR_SIZE)"

RESPONSE=$(curl -s -w '\n%{http_code}' -X POST \
    "${DB_GATEWAY_URL}/platform/${PLATFORM_ID}/schema" \
    -F "schema_name=${SCHEMA_NAME}" \
    -F "schema=@${TAR_FILE}" 2>&1) || true

HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
BODY=$(echo "$RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    echo -e "${GREEN}  Schema uploaded${NC}"
    if command -v jq &> /dev/null; then
        echo "  $(echo "$BODY" | jq -c '{has_tables, has_functions, has_migrations, checksum}' 2>/dev/null)"
    fi
else
    echo -e "${RED}  Failed to upload schema (HTTP $HTTP_CODE)${NC}"
    echo "  $BODY"
    exit 1
fi

# ─── Step 3: Create database from stored schema ──────────────────────────────
echo -e "${YELLOW}Step 3/3: Creating database...${NC}"

ATTEMPT=1
while [ $ATTEMPT -le $RETRY_COUNT ]; do
    RESPONSE=$(curl -s -w '\n%{http_code}' -X POST \
        "${DB_GATEWAY_URL}/admin/database/create" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${ADMIN_TOKEN}" \
        -d "{\"platform\": \"${PLATFORM_ID}\", \"schema_name\": \"${SCHEMA_NAME}\", \"database_id\": \"${DATABASE_ID}\"}" 2>&1) || true

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        echo -e "${GREEN}  Database created!${NC}"
        echo "$BODY" | jq . 2>/dev/null || echo "  $BODY"
        echo ""
        echo -e "${GREEN}Registration complete.${NC}"
        exit 0
    elif [ "$HTTP_CODE" = "409" ]; then
        echo -e "${YELLOW}  Database already exists${NC}"
        echo "  $BODY"
        echo ""
        echo -e "${GREEN}Registration complete (database existed).${NC}"
        exit 0
    elif [ "$HTTP_CODE" = "000" ]; then
        echo -e "${RED}  Connection failed (gateway not reachable)${NC}"
    else
        echo -e "${RED}  Failed (HTTP $HTTP_CODE)${NC}"
        echo "  $BODY"
    fi

    if [ $ATTEMPT -lt $RETRY_COUNT ]; then
        echo "  Retrying in ${RETRY_DELAY}s..."
        sleep $RETRY_DELAY
    fi
    ATTEMPT=$((ATTEMPT + 1))
done

echo -e "${RED}Failed after $RETRY_COUNT attempts${NC}"
exit 1
