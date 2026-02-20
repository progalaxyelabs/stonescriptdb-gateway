#!/bin/bash
# =============================================================================
# Hot Migrate Schema via StoneScriptDB Gateway (V2 API)
# =============================================================================
# Two-step migration flow:
#   1. Upload schema archive (stores new version)
#   2. Migrate database using stored schema (JSON body, no tar.gz)
#
# Usage:
#   ./migrate-schema.sh [options]
#
# Environment variables (required):
#   DB_GATEWAY_URL    - Gateway URL (e.g., http://gateway:9000)
#   PLATFORM_ID       - Platform identifier (e.g., myapp)
#   SCHEMA_NAME       - Schema version name (e.g., v1.0, latest)
#   DATABASE_ID       - Database to migrate (e.g., main, tenant_abc123)
#
# Environment variables (optional):
#   FORCE             - Set to "true" to bypass DATALOSS checks
#   POSTGRESQL_PATH   - Path to postgresql folder (default: ./src/postgresql)
#   CACHE_DIR         - Temp directory for tar files (default: ./.cache)
#
# Example:
#   # Migrate main database
#   DB_GATEWAY_URL=http://gateway:9000 PLATFORM_ID=myapp SCHEMA_NAME=v1.0 \
#     DATABASE_ID=main ./migrate-schema.sh
#
#   # Migrate specific tenant
#   DB_GATEWAY_URL=http://gateway:9000 PLATFORM_ID=myapp SCHEMA_NAME=v1.0 \
#     DATABASE_ID=tenant_abc123 ./migrate-schema.sh
# =============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Configuration with defaults
DB_GATEWAY_URL="${DB_GATEWAY_URL:-}"
PLATFORM_ID="${PLATFORM_ID:-}"
SCHEMA_NAME="${SCHEMA_NAME:-}"
DATABASE_ID="${DATABASE_ID:-}"
FORCE="${FORCE:-false}"
POSTGRESQL_PATH="${POSTGRESQL_PATH:-./src/postgresql}"
CACHE_DIR="${CACHE_DIR:-./.cache}"
RETRY_COUNT="${RETRY_COUNT:-3}"
RETRY_DELAY="${RETRY_DELAY:-5}"

# Validate required environment variables
for VAR in DB_GATEWAY_URL PLATFORM_ID SCHEMA_NAME DATABASE_ID; do
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
TAR_FILE="${CACHE_DIR}/postgresql_${PLATFORM_ID}_migrate_${TIMESTAMP}.tar.gz"

# Cleanup function
cleanup() {
    if [ -f "$TAR_FILE" ]; then
        rm -f "$TAR_FILE"
    fi
}
trap cleanup EXIT

echo -e "${CYAN}=== StoneScriptDB Gateway Migration (V2) ===${NC}"
echo "Platform:    $PLATFORM_ID"
echo "Schema:      $SCHEMA_NAME"
echo "Database ID: $DATABASE_ID"
echo "Force:       $FORCE"
echo "Gateway:     $DB_GATEWAY_URL"
echo ""

# ─── Step 1: Upload schema archive ───────────────────────────────────────────
echo -e "${YELLOW}Step 1/2: Uploading schema...${NC}"

# Create tar.gz
tar -czf "$TAR_FILE" -C "$(dirname "$POSTGRESQL_PATH")" "$(basename "$POSTGRESQL_PATH")"
TAR_SIZE=$(du -h "$TAR_FILE" | cut -f1)
echo "  Archive: $TAR_FILE ($TAR_SIZE)"

# Count files
FUNC_COUNT=$(tar -tzf "$TAR_FILE" | grep -c "functions/.*\.pssql$" || echo 0)
MIGRATION_COUNT=$(tar -tzf "$TAR_FILE" | grep -c "migrations/.*\.pssql$" || echo 0)
echo "  Functions: $FUNC_COUNT files"
echo "  Migrations: $MIGRATION_COUNT files"

RESPONSE=$(curl -s -w '\n%{http_code}' -X POST \
    "${DB_GATEWAY_URL}/platform/${PLATFORM_ID}/schema" \
    -F "schema_name=${SCHEMA_NAME}" \
    -F "schema=@${TAR_FILE}" 2>&1) || true

HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
BODY=$(echo "$RESPONSE" | sed '$d')

if [ "$HTTP_CODE" = "200" ]; then
    echo -e "${GREEN}  Schema uploaded${NC}"
else
    echo -e "${RED}  Failed to upload schema (HTTP $HTTP_CODE)${NC}"
    echo "  $BODY"
    exit 1
fi
echo ""

# ─── Step 2: Migrate database ────────────────────────────────────────────────
echo -e "${YELLOW}Step 2/2: Migrating database '${DATABASE_ID}'...${NC}"

# Build force flag
FORCE_JSON="false"
if [ "$FORCE" = "true" ]; then
    FORCE_JSON="true"
fi

ATTEMPT=1
while [ $ATTEMPT -le $RETRY_COUNT ]; do
    echo "  Attempt $ATTEMPT of $RETRY_COUNT..."

    RESPONSE=$(curl -s -w '\n%{http_code}' -X POST "${DB_GATEWAY_URL}/v2/migrate" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"${PLATFORM_ID}\", \"schema_name\": \"${SCHEMA_NAME}\", \"database_id\": \"${DATABASE_ID}\", \"force\": ${FORCE_JSON}}" 2>&1) || true

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        echo -e "${GREEN}Migration successful!${NC}"
        echo "$BODY" | jq . 2>/dev/null || echo "$BODY"

        if command -v jq &> /dev/null; then
            STATUS=$(echo "$BODY" | jq -r '.status // "unknown"')
            DBS_UPDATED=$(echo "$BODY" | jq -r '.databases_updated | length // 0')
            MIGRATIONS=$(echo "$BODY" | jq -r '.migrations_applied // 0')
            FUNCTIONS=$(echo "$BODY" | jq -r '.functions_updated // 0')
            EXEC_TIME=$(echo "$BODY" | jq -r '.execution_time_ms // 0')

            echo ""
            echo -e "${GREEN}Summary:${NC}"
            echo "  Status: $STATUS"
            echo "  Databases updated: $DBS_UPDATED"
            echo "  Migrations applied: $MIGRATIONS"
            echo "  Functions updated: $FUNCTIONS"
            echo "  Execution time: ${EXEC_TIME}ms"
        fi

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
