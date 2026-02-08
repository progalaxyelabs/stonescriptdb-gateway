#!/bin/bash
set -e

# Setup Test Environment for E2E Tests
# This script helps set up test tenants and verify the environment

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo "=========================================="
echo "StoneScriptDB Gateway E2E Test Setup"
echo "=========================================="
echo ""

# Load .env if exists
if [ -f "$SCRIPT_DIR/.env" ]; then
    export $(grep -v '^#' "$SCRIPT_DIR/.env" | xargs)
fi

GATEWAY_URL=${GATEWAY_URL:-http://192.168.122.173:9000}

# Step 1: Check gateway connectivity
echo -e "${BLUE}Step 1: Checking Gateway Connectivity${NC}"
if curl -sf "$GATEWAY_URL/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} Gateway is accessible at $GATEWAY_URL"
else
    echo -e "${RED}✗${NC} Gateway is NOT accessible at $GATEWAY_URL"
    echo ""
    echo "Please ensure:"
    echo "  1. Gateway service is running: sudo systemctl status stonescriptdb-gateway"
    echo "  2. Gateway is listening on port 9000"
    echo "  3. Firewall allows access to port 9000"
    exit 1
fi

# Step 2: Check JWKS endpoint
echo ""
echo -e "${BLUE}Step 2: Checking JWKS Endpoint${NC}"
JWKS_RESPONSE=$(curl -sf "$GATEWAY_URL/auth/jwks")
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} JWKS endpoint is working"
else
    echo -e "${RED}✗${NC} JWKS endpoint failed"
    exit 1
fi

# Step 3: Verify test environment
echo ""
echo -e "${BLUE}Step 3: Environment Configuration${NC}"
echo "Gateway URL: $GATEWAY_URL"
echo "Progalaxy Platform: ${PROGALAXY_PLATFORM_CODE:-progalaxy}"
echo "Progalaxy Tenant: ${PROGALAXY_TENANT_SLUG:-test-tenant}"
echo "BTechRecruiter Platform: ${BTECHRECRUITER_PLATFORM_CODE:-btechrecruiter}"
echo "BTechRecruiter Tenant: ${BTECHRECRUITER_TENANT_SLUG:-test-company}"

# Step 4: Tenant setup instructions
echo ""
echo -e "${BLUE}Step 4: Test Tenant Setup${NC}"
echo -e "${YELLOW}⚠${NC} Test tenants must be created manually before running tests:"
echo ""
echo "Required tenants:"
echo "  1. Platform: progalaxy, Slug: test-tenant"
echo "  2. Platform: btechrecruiter, Slug: test-company"
echo ""
echo "To create tenants, use one of these methods:"
echo "  • Via platform registration (first user creates tenant)"
echo "  • Via admin API (if available)"
echo "  • Via database migration scripts"
echo ""
read -p "Have you created the test tenants? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo ""
    echo -e "${YELLOW}Please create test tenants before running E2E tests${NC}"
    exit 1
fi

# Step 5: Test registration endpoint
echo ""
echo -e "${BLUE}Step 5: Testing Registration Endpoint${NC}"
TEST_EMAIL="setup-test-$(date +%s)@example.com"
TEST_PASSWORD="TestPass123!"

echo "Attempting test registration with: $TEST_EMAIL"

REGISTER_RESPONSE=$(curl -sf -X POST "$GATEWAY_URL/auth/register" \
  -H "Content-Type: application/json" \
  -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

if [ $? -eq 0 ]; then
    IDENTITY_ID=$(echo "$REGISTER_RESPONSE" | grep -o '"identity_id":"[^"]*"' | cut -d'"' -f4)
    if [ -n "$IDENTITY_ID" ]; then
        echo -e "${GREEN}✓${NC} Registration endpoint is working"
        echo "  Identity ID: $IDENTITY_ID"
    else
        echo -e "${YELLOW}⚠${NC} Registration returned unexpected response"
        echo "$REGISTER_RESPONSE"
    fi
else
    echo -e "${RED}✗${NC} Registration endpoint failed"
    exit 1
fi

# Step 6: Node.js dependencies
echo ""
echo -e "${BLUE}Step 6: Installing Node.js Dependencies${NC}"
cd "$SCRIPT_DIR"
if [ ! -d "node_modules" ]; then
    echo "Installing npm packages..."
    npm install
    echo -e "${GREEN}✓${NC} Dependencies installed"
else
    echo -e "${GREEN}✓${NC} Dependencies already installed"
fi

# Step 7: Playwright browsers
echo ""
echo -e "${BLUE}Step 7: Installing Playwright Browsers${NC}"
npx playwright install chromium
echo -e "${GREEN}✓${NC} Browsers installed"

# Summary
echo ""
echo -e "${GREEN}=========================================="
echo "✓ Test Environment Setup Complete!"
echo -e "==========================================${NC}"
echo ""
echo "You can now run E2E tests with:"
echo "  ./run-tests.sh"
echo ""
echo "Or run specific tests:"
echo "  npm test -- tests/01-registration.spec.ts"
echo ""
echo "For interactive debugging:"
echo "  npm run test:ui"
echo ""
