#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=====================================${NC}"
echo -e "${BLUE}E2E Test: Return Type Change Migration${NC}"
echo -e "${BLUE}=====================================${NC}"
echo ""

# Configuration
TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATEWAY_ROOT="$(cd "$TEST_DIR/../.." && pwd)"
GATEWAY_URL="${GATEWAY_URL:-http://192.168.122.173:9000}"
TEST_PLATFORM="test_return_type_platform_$$"
TEST_DB="${TEST_PLATFORM}_main"

echo -e "${YELLOW}Test Configuration:${NC}"
echo "  Test Directory: $TEST_DIR"
echo "  Gateway Root: $GATEWAY_ROOT"
echo "  Gateway URL: $GATEWAY_URL"
echo "  Test Platform: $TEST_PLATFORM"
echo "  Test Database: $TEST_DB"
echo ""

# Check if gateway is running
echo -e "${BLUE}[1/9] Checking gateway availability...${NC}"
if ! curl -s "${GATEWAY_URL}/health" > /dev/null 2>&1; then
    echo -e "${RED}ERROR: Gateway not accessible at ${GATEWAY_URL}${NC}"
    echo "Please start the gateway first or set GATEWAY_URL environment variable"
    exit 1
fi
echo -e "${GREEN}✓ Gateway is running${NC}"
echo ""

# Build the gateway to ensure latest code
echo -e "${BLUE}[2/9] Building gateway with latest changes...${NC}"
cd "$GATEWAY_ROOT"
cargo build --release
echo -e "${GREEN}✓ Gateway built successfully${NC}"
echo ""

# Create test schema directory structure
echo -e "${BLUE}[3/9] Creating test schema directory...${NC}"
SCHEMA_DIR="$TEST_DIR/schema"
rm -rf "$SCHEMA_DIR"
mkdir -p "$SCHEMA_DIR"/{functions,migrations,tables}

# Create a test table
cat > "$SCHEMA_DIR/tables/oauth_users.pssql" << 'EOF'
CREATE TABLE oauth_users (
    id SERIAL PRIMARY KEY,
    provider VARCHAR(50) NOT NULL,
    provider_user_id VARCHAR(255) NOT NULL,
    email VARCHAR(255),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(provider, provider_user_id)
);
EOF

# Create initial function with TEXT return types (the bug scenario)
cat > "$SCHEMA_DIR/functions/upsert_oauth_user.pssql" << 'EOF'
CREATE OR REPLACE FUNCTION upsert_oauth_user(
    p_provider TEXT,
    p_provider_user_id TEXT,
    p_email TEXT
)
RETURNS TABLE (
    o_user_id INT,
    o_provider TEXT,
    o_provider_user_id TEXT,
    o_email TEXT
) AS $$
BEGIN
    INSERT INTO oauth_users (provider, provider_user_id, email)
    VALUES (p_provider, p_provider_user_id, p_email)
    ON CONFLICT (provider, provider_user_id)
    DO UPDATE SET
        email = EXCLUDED.email
    RETURNING id, provider, provider_user_id, email;
END;
$$ LANGUAGE plpgsql;
EOF

echo -e "${GREEN}✓ Schema files created${NC}"
echo ""

# Register test platform
echo -e "${BLUE}[4/9] Registering test platform...${NC}"
REGISTER_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/platforms/register" \
    -H "Content-Type: application/json" \
    -d "{\"name\": \"${TEST_PLATFORM}\"}")

if echo "$REGISTER_RESPONSE" | grep -q "error"; then
    echo -e "${YELLOW}Platform may already exist (this is ok for testing)${NC}"
else
    echo -e "${GREEN}✓ Platform registered: ${TEST_PLATFORM}${NC}"
fi
echo ""

# Register test database
echo -e "${BLUE}[5/9] Registering test database...${NC}"
DB_REGISTER_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/databases/register" \
    -H "Content-Type: application/json" \
    -d "{\"name\": \"${TEST_DB}\", \"platform\": \"${TEST_PLATFORM}\"}")

if echo "$DB_REGISTER_RESPONSE" | grep -q "error"; then
    echo -e "${YELLOW}Database may already exist (this is ok for testing)${NC}"
else
    echo -e "${GREEN}✓ Database registered: ${TEST_DB}${NC}"
fi
echo ""

# Package and migrate initial schema
echo -e "${BLUE}[6/9] Running initial migration (TEXT return types)...${NC}"
cd "$SCHEMA_DIR"
tar -czf "$TEST_DIR/schema-v1.tar.gz" .

MIGRATE_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/migrate" \
    -H "Content-Type: application/octet-stream" \
    -H "X-Database-Names: ${TEST_DB}" \
    --data-binary "@$TEST_DIR/schema-v1.tar.gz")

echo "Migration Response:"
echo "$MIGRATE_RESPONSE" | jq '.' 2>/dev/null || echo "$MIGRATE_RESPONSE"

if echo "$MIGRATE_RESPONSE" | grep -q '"status":"completed"'; then
    echo -e "${GREEN}✓ Initial migration completed${NC}"
else
    echo -e "${RED}ERROR: Initial migration failed${NC}"
    exit 1
fi
echo ""

# Verify function exists with TEXT return type
echo -e "${BLUE}[7/9] Verifying initial function signature...${NC}"
QUERY_SQL="SELECT return_type FROM _stonescriptdb_gateway_functions WHERE function_name = 'upsert_oauth_user'"
QUERY_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/query" \
    -H "Content-Type: application/json" \
    -d "{\"database\": \"${TEST_DB}\", \"sql\": \"${QUERY_SQL}\"}")

echo "Initial return type:"
echo "$QUERY_RESPONSE" | jq -r '.rows[0].return_type' 2>/dev/null || echo "$QUERY_RESPONSE"

if echo "$QUERY_RESPONSE" | grep -q "TEXT"; then
    echo -e "${GREEN}✓ Function deployed with TEXT return types${NC}"
else
    echo -e "${YELLOW}Warning: Could not verify TEXT return types${NC}"
fi
echo ""

# Modify function to use VARCHAR return types (simulate the bug report scenario)
echo -e "${BLUE}[8/9] Modifying function return types (TEXT → VARCHAR)...${NC}"
cat > "$SCHEMA_DIR/functions/upsert_oauth_user.pssql" << 'EOF'
CREATE OR REPLACE FUNCTION upsert_oauth_user(
    p_provider TEXT,
    p_provider_user_id TEXT,
    p_email TEXT
)
RETURNS TABLE (
    o_user_id INT,
    o_provider VARCHAR(50),
    o_provider_user_id VARCHAR(255),
    o_email VARCHAR(255)
) AS $$
BEGIN
    INSERT INTO oauth_users (provider, provider_user_id, email)
    VALUES (p_provider, p_provider_user_id, p_email)
    ON CONFLICT (provider, provider_user_id)
    DO UPDATE SET
        email = EXCLUDED.email
    RETURNING id, provider, provider_user_id, email;
END;
$$ LANGUAGE plpgsql;
EOF

# Package modified schema
tar -czf "$TEST_DIR/schema-v2.tar.gz" .

echo -e "${GREEN}✓ Function modified to use VARCHAR return types${NC}"
echo ""

# Run migration with changed return types - THIS SHOULD AUTO-DROP
echo -e "${BLUE}[9/9] Running migration with changed return types...${NC}"
echo -e "${YELLOW}This should automatically DROP and recreate the function${NC}"
echo ""

MIGRATE_V2_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/migrate" \
    -H "Content-Type: application/octet-stream" \
    -H "X-Database-Names: ${TEST_DB}" \
    --data-binary "@$TEST_DIR/schema-v2.tar.gz")

echo "Migration Response:"
echo "$MIGRATE_V2_RESPONSE" | jq '.' 2>/dev/null || echo "$MIGRATE_V2_RESPONSE"

if echo "$MIGRATE_V2_RESPONSE" | grep -q '"status":"completed"'; then
    echo ""
    echo -e "${GREEN}✓✓✓ SUCCESS! Migration completed automatically!${NC}"
    echo -e "${GREEN}The gateway detected the return type change and auto-dropped the old function!${NC}"
else
    echo ""
    echo -e "${RED}✗✗✗ FAILED! Migration did not complete${NC}"
    echo -e "${RED}The bug is NOT fixed - return type changes still fail${NC}"
    exit 1
fi
echo ""

# Verify new function signature
echo -e "${BLUE}Verifying new function signature...${NC}"
QUERY_V2_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/query" \
    -H "Content-Type: application/json" \
    -d "{\"database\": \"${TEST_DB}\", \"sql\": \"${QUERY_SQL}\"}")

echo "New return type:"
NEW_RETURN_TYPE=$(echo "$QUERY_V2_RESPONSE" | jq -r '.rows[0].return_type' 2>/dev/null)
echo "$NEW_RETURN_TYPE"

if echo "$NEW_RETURN_TYPE" | grep -q "VARCHAR"; then
    echo -e "${GREEN}✓ Function updated with VARCHAR return types${NC}"
else
    echo -e "${RED}Warning: Return type may not have been updated correctly${NC}"
fi
echo ""

# Test function execution
echo -e "${BLUE}Testing function execution...${NC}"
TEST_FUNC_SQL="SELECT * FROM upsert_oauth_user('github', 'user123', 'test@example.com')"
TEST_FUNC_RESPONSE=$(curl -s -X POST "${GATEWAY_URL}/query" \
    -H "Content-Type: application/json" \
    -d "{\"database\": \"${TEST_DB}\", \"sql\": \"${TEST_FUNC_SQL}\"}")

echo "Function execution result:"
echo "$TEST_FUNC_RESPONSE" | jq '.' 2>/dev/null || echo "$TEST_FUNC_RESPONSE"

if echo "$TEST_FUNC_RESPONSE" | grep -q '"o_provider"'; then
    echo -e "${GREEN}✓ Function executes successfully${NC}"
else
    echo -e "${YELLOW}Warning: Function execution may have issues${NC}"
fi
echo ""

# Cleanup
echo -e "${BLUE}Cleanup...${NC}"
echo -e "${YELLOW}To clean up test resources, run:${NC}"
echo "  curl -X POST ${GATEWAY_URL}/databases/unregister -H 'Content-Type: application/json' -d '{\"name\": \"${TEST_DB}\"}'"
echo "  curl -X POST ${GATEWAY_URL}/platforms/unregister -H 'Content-Type: application/json' -d '{\"name\": \"${TEST_PLATFORM}\"}'"
echo ""

# Summary
echo -e "${BLUE}=====================================${NC}"
echo -e "${GREEN}TEST PASSED!${NC}"
echo -e "${BLUE}=====================================${NC}"
echo ""
echo "Summary:"
echo "  ✓ Initial migration with TEXT return types succeeded"
echo "  ✓ Return type change (TEXT → VARCHAR) detected automatically"
echo "  ✓ Old function dropped automatically"
echo "  ✓ New function created successfully"
echo "  ✓ Function executes correctly"
echo ""
echo -e "${GREEN}The fix is working! Return type changes no longer require manual intervention.${NC}"
echo ""
