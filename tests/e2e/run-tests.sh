#!/bin/bash
set -e

# StoneScriptDB Gateway E2E Test Runner
# This script runs the Playwright E2E tests for the identity system

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=========================================="
echo "StoneScriptDB Gateway E2E Test Runner"
echo "=========================================="
echo ""

# Check if Node.js is installed
if ! command -v node &> /dev/null; then
    echo -e "${RED}Error: Node.js is not installed${NC}"
    echo "Please install Node.js 18+ to run these tests"
    exit 1
fi

echo -e "${GREEN}✓${NC} Node.js version: $(node --version)"

# Check if npm is installed
if ! command -v npm &> /dev/null; then
    echo -e "${RED}Error: npm is not installed${NC}"
    exit 1
fi

echo -e "${GREEN}✓${NC} npm version: $(npm --version)"

# Check if .env file exists
if [ ! -f .env ]; then
    echo -e "${YELLOW}⚠${NC} .env file not found, using defaults from .env.example"
    cp .env.example .env
fi

# Load environment variables
if [ -f .env ]; then
    export $(grep -v '^#' .env | xargs)
fi

GATEWAY_URL=${GATEWAY_URL:-http://localhost:9000}

# Check if gateway is accessible
echo ""
echo "Checking gateway connectivity..."
if curl -sf "$GATEWAY_URL/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} Gateway is accessible at $GATEWAY_URL"
else
    echo -e "${RED}✗${NC} Gateway is NOT accessible at $GATEWAY_URL"
    echo -e "${YELLOW}⚠${NC} Tests may fail if gateway is not running"
    echo ""
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Install dependencies if node_modules doesn't exist
if [ ! -d "node_modules" ]; then
    echo ""
    echo "Installing dependencies..."
    npm install
    echo -e "${GREEN}✓${NC} Dependencies installed"
fi

# Install Playwright browsers if needed
if [ ! -d "$HOME/.cache/ms-playwright" ] && [ ! -d "$HOME/Library/Caches/ms-playwright" ]; then
    echo ""
    echo "Installing Playwright browsers..."
    npx playwright install chromium firefox webkit
    echo -e "${GREEN}✓${NC} Browsers installed"
fi

# Parse command line arguments
MODE="test"
HEADED=""
UI=""
DEBUG=""
SPECIFIC_TEST=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --headed)
            HEADED="--headed"
            shift
            ;;
        --ui)
            UI="--ui"
            shift
            ;;
        --debug)
            DEBUG="--debug"
            shift
            ;;
        --test)
            SPECIFIC_TEST="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--headed] [--ui] [--debug] [--test <test-file>]"
            exit 1
            ;;
    esac
done

# Run tests
echo ""
echo "=========================================="
echo "Running E2E Tests"
echo "=========================================="
echo "Gateway URL: $GATEWAY_URL"
echo ""

if [ -n "$SPECIFIC_TEST" ]; then
    npx playwright test "$SPECIFIC_TEST" $HEADED $UI $DEBUG
elif [ -n "$UI" ]; then
    npx playwright test $UI
elif [ -n "$DEBUG" ]; then
    npx playwright test $DEBUG
else
    npm test
fi

TEST_EXIT_CODE=$?

# Generate report
if [ $TEST_EXIT_CODE -eq 0 ]; then
    echo ""
    echo -e "${GREEN}=========================================="
    echo "✓ All tests passed!"
    echo -e "==========================================${NC}"
    echo ""
    echo "View detailed report: npm run test:report"
else
    echo ""
    echo -e "${RED}=========================================="
    echo "✗ Some tests failed"
    echo -e "==========================================${NC}"
    echo ""
    echo "View detailed report: npm run test:report"
    echo "Debug failed tests: npm run test:debug"
fi

exit $TEST_EXIT_CODE
