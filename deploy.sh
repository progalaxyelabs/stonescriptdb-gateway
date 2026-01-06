#!/bin/bash
# =============================================================================
# StoneScriptDB Gateway Deployment Script
# =============================================================================
# Deploys the gateway binary to the VM and restarts the service
#
# Usage:
#   ./deploy.sh [TARGET]
#
# Targets:
#   dev   - Deploy to development server (default)
#   prod  - Deploy to production server
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Configuration
TARGET=${1:-dev}
BINARY_SOURCE="target/release/stonescriptdb-gateway"
BINARY_DEST="/usr/local/bin/stonescriptdb-gateway"
SERVICE_NAME="stonescriptdb-gateway"

echo -e "${YELLOW}Deploying to: ${TARGET}${NC}"

# Check if binary exists
if [ ! -f "$BINARY_SOURCE" ]; then
    echo -e "${RED}Error: Binary not found at $BINARY_SOURCE${NC}"
    echo "Run 'cargo build --release' first"
    exit 1
fi

# Copy binary to VM
echo -e "${YELLOW}Copying binary to ${TARGET}...${NC}"
scp "$BINARY_SOURCE" "${TARGET}:/tmp/stonescriptdb-gateway"

# Deploy on VM
echo -e "${YELLOW}Deploying on ${TARGET}...${NC}"
ssh "$TARGET" << 'ENDSSH'
set -e

echo "Stopping service..."
sudo systemctl stop stonescriptdb-gateway

echo "Installing binary..."
sudo cp /tmp/stonescriptdb-gateway /opt/stonescriptdb-gateway/
sudo chmod +x /opt/stonescriptdb-gateway/stonescriptdb-gateway
sudo chown stonescriptdb-gateway:stonescriptdb-gateway /opt/stonescriptdb-gateway/stonescriptdb-gateway

echo "Starting service..."
sudo systemctl start stonescriptdb-gateway

echo "Waiting for service to start..."
sleep 2

echo "Service status:"
sudo systemctl status stonescriptdb-gateway --no-pager -l | head -20

ENDSSH

echo ""
echo -e "${GREEN}=== Deployment Successful ===${NC}"
echo "Service is running on ${TARGET}"
echo ""
echo "To view logs:"
echo "  ssh ${TARGET} 'sudo journalctl -u stonescriptdb-gateway -f'"
echo "  ssh ${TARGET} 'sudo tail -f /var/log/stonescriptdb-gateway/stonescriptdb-gateway.log'"
