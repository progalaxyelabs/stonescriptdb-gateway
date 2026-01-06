#!/bin/bash
# =============================================================================
# Register Platform with StoneScriptDB Gateway
# =============================================================================
# Creates a timestamped tar.gz of postgresql folder, sends to stonescriptdb-gateway,
# then cleans up.
#
# Usage:
#   ./register-with-gateway.sh [options]
#
# Environment variables (required):
#   DB_GATEWAY_URL    - Gateway URL (e.g., http://localhost:9000)
#   PLATFORM_ID       - Platform identifier (e.g., myapp)
#
# Environment variables (optional):
#   TENANT_ID         - Tenant identifier (omit for main DB)
#   POSTGRESQL_PATH   - Path to postgresql folder (default: ./src/postgresql)
#   CACHE_DIR         - Temp directory for tar files (default: ./.cache)
#
# Example:
#   DB_GATEWAY_URL=http://localhost:9000 PLATFORM_ID=myplatform ./register-with-gateway.sh
# =============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

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
TAR_FILE="${CACHE_DIR}/postgresql_${PLATFORM_ID}_${TIMESTAMP}.tar.gz"

# Cleanup function
cleanup() {
    if [ -f "$TAR_FILE" ]; then
        rm -f "$TAR_FILE"
        echo -e "${YELLOW}Cleaned up: $TAR_FILE${NC}"
    fi
}

# Trap to ensure cleanup on exit (success or failure)
trap cleanup EXIT

echo -e "${GREEN}=== StoneScriptDB Gateway Registration ===${NC}"
echo "Platform: $PLATFORM_ID"
echo "Tenant: ${TENANT_ID:-<main>}"
echo "Gateway: $DB_GATEWAY_URL"
echo "PostgreSQL path: $POSTGRESQL_PATH"
echo ""

# Step 1: Create tar.gz
echo -e "${YELLOW}Creating schema archive...${NC}"
tar -czf "$TAR_FILE" -C "$(dirname "$POSTGRESQL_PATH")" "$(basename "$POSTGRESQL_PATH")"
TAR_SIZE=$(du -h "$TAR_FILE" | cut -f1)
echo "Created: $TAR_FILE ($TAR_SIZE)"

# Step 2: Send to gateway with retry logic
echo -e "${YELLOW}Registering with gateway...${NC}"

ATTEMPT=1
while [ $ATTEMPT -le $RETRY_COUNT ]; do
    echo "Attempt $ATTEMPT of $RETRY_COUNT..."

    # Build curl command
    CURL_CMD="curl -s -w '\n%{http_code}' -X POST ${DB_GATEWAY_URL}/register"
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
        echo -e "${GREEN}Registration successful!${NC}"
        echo "$BODY" | jq . 2>/dev/null || echo "$BODY"

        # Extract and display key info
        if command -v jq &> /dev/null; then
            STATUS=$(echo "$BODY" | jq -r '.status // "unknown"')
            DATABASE=$(echo "$BODY" | jq -r '.database // "unknown"')
            MIGRATIONS=$(echo "$BODY" | jq -r '.migrations_applied // 0')
            FUNCTIONS=$(echo "$BODY" | jq -r '.functions_deployed // 0')

            echo ""
            echo -e "${GREEN}Summary:${NC}"
            echo "  Status: $STATUS"
            echo "  Database: $DATABASE"
            echo "  Migrations applied: $MIGRATIONS"
            echo "  Functions deployed: $FUNCTIONS"
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
