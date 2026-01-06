#!/bin/bash
# =============================================================================
# Hot Migrate Schema to DB Gateway
# =============================================================================
# Updates schema on running databases without container restart.
# Creates a timestamped tar.gz, sends to /migrate endpoint, cleans up.
#
# Usage:
#   ./migrate-schema.sh [options]
#
# Environment variables (required):
#   DB_GATEWAY_URL    - Gateway URL (e.g., http://localhost:9000)
#   PLATFORM_ID       - Platform identifier (e.g., myapp)
#
# Environment variables (optional):
#   TENANT_ID         - Specific tenant (omit to migrate ALL tenants)
#   POSTGRESQL_PATH   - Path to postgresql folder (default: ./src/postgresql)
#   CACHE_DIR         - Temp directory for tar files (default: ./.cache)
#
# Example:
#   # Migrate all tenants
#   DB_GATEWAY_URL=http://localhost:9000 PLATFORM_ID=myapp ./migrate-schema.sh
#
#   # Migrate specific tenant
#   DB_GATEWAY_URL=http://localhost:9000 PLATFORM_ID=myapp TENANT_ID=clinic_001 ./migrate-schema.sh
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
TENANT_ID="${TENANT_ID:-}"
POSTGRESQL_PATH="${POSTGRESQL_PATH:-./src/postgresql}"
CACHE_DIR="${CACHE_DIR:-./.cache}"
RETRY_COUNT="${RETRY_COUNT:-3}"
RETRY_DELAY="${RETRY_DELAY:-5}"

# Validate required environment variables
if [ -z "$DB_GATEWAY_URL" ]; then
    echo -e "${RED}ERROR: DB_GATEWAY_URL environment variable is required${NC}"
    exit 1
fi

if [ -z "$PLATFORM_ID" ]; then
    echo -e "${RED}ERROR: PLATFORM_ID environment variable is required${NC}"
    exit 1
fi

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
        echo -e "${YELLOW}Cleaned up: $TAR_FILE${NC}"
    fi
}

# Trap to ensure cleanup on exit
trap cleanup EXIT

echo -e "${CYAN}=== DB Gateway Schema Migration ===${NC}"
echo "Platform: $PLATFORM_ID"
if [ -n "$TENANT_ID" ]; then
    echo "Tenant: $TENANT_ID (single tenant)"
else
    echo "Tenant: ALL (will migrate all tenant databases)"
fi
echo "Gateway: $DB_GATEWAY_URL"
echo "PostgreSQL path: $POSTGRESQL_PATH"
echo ""

# Step 1: Create tar.gz
echo -e "${YELLOW}Creating schema archive...${NC}"
tar -czf "$TAR_FILE" -C "$(dirname "$POSTGRESQL_PATH")" "$(basename "$POSTGRESQL_PATH")"
TAR_SIZE=$(du -h "$TAR_FILE" | cut -f1)
echo "Created: $TAR_FILE ($TAR_SIZE)"

# Count files in archive
FUNC_COUNT=$(tar -tzf "$TAR_FILE" | grep -c "functions/.*\.pssql$" || echo 0)
MIGRATION_COUNT=$(tar -tzf "$TAR_FILE" | grep -c "migrations/.*\.pssql$" || echo 0)
echo "  Functions: $FUNC_COUNT files"
echo "  Migrations: $MIGRATION_COUNT files"
echo ""

# Step 2: Send to gateway with retry logic
echo -e "${YELLOW}Sending migration to gateway...${NC}"

ATTEMPT=1
while [ $ATTEMPT -le $RETRY_COUNT ]; do
    echo "Attempt $ATTEMPT of $RETRY_COUNT..."

    # Build curl command
    CURL_CMD="curl -s -w '\n%{http_code}' -X POST ${DB_GATEWAY_URL}/migrate"
    CURL_CMD="$CURL_CMD -F platform=$PLATFORM_ID"
    CURL_CMD="$CURL_CMD -F schema=@$TAR_FILE"

    if [ -n "$TENANT_ID" ]; then
        CURL_CMD="$CURL_CMD -F tenant_id=$TENANT_ID"
    fi

    # Execute request
    RESPONSE=$(eval $CURL_CMD 2>&1) || true

    # Extract HTTP status code (last line)
    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    # Extract body (all but last line)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        echo -e "${GREEN}Migration successful!${NC}"
        echo "$BODY" | jq . 2>/dev/null || echo "$BODY"

        # Extract and display key info
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

            # List updated databases
            echo ""
            echo "Databases:"
            echo "$BODY" | jq -r '.databases_updated[]' 2>/dev/null | while read db; do
                echo "  - $db"
            done
        fi

        exit 0
    elif [ "$HTTP_CODE" = "000" ]; then
        echo -e "${RED}Connection failed (gateway not reachable)${NC}"
    else
        echo -e "${RED}Request failed with HTTP $HTTP_CODE${NC}"
        echo "$BODY"
    fi

    if [ $ATTEMPT -lt $RETRY_COUNT ]; then
        echo "Retrying in ${RETRY_DELAY}s..."
        sleep $RETRY_DELAY
    fi

    ATTEMPT=$((ATTEMPT + 1))
done

echo -e "${RED}Failed after $RETRY_COUNT attempts${NC}"
exit 1
