#!/bin/bash
# =============================================================================
# DB Gateway Installation Script
# =============================================================================
# Installs db-gateway as a systemd service on Ubuntu/Debian
#
# Usage:
#   sudo ./install.sh
#
# Prerequisites:
#   - PostgreSQL server running
#   - Rust/Cargo installed for building (or pre-built binary)
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Configuration
INSTALL_DIR="/opt/db-gateway"
SERVICE_USER="db-gateway"
SERVICE_GROUP="db-gateway"

echo -e "${GREEN}=== DB Gateway Installation ===${NC}"

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Error: Please run as root (sudo ./install.sh)${NC}"
    exit 1
fi

# Check if this is the source directory
if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}Error: Run this script from the db-gateway source directory${NC}"
    exit 1
fi

# Step 1: Build release binary
echo -e "${YELLOW}Building release binary...${NC}"
cargo build --release

# Step 2: Create service user
if ! id "$SERVICE_USER" &>/dev/null; then
    echo -e "${YELLOW}Creating service user: $SERVICE_USER${NC}"
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
else
    echo "Service user $SERVICE_USER already exists"
fi

# Step 3: Create installation directory
echo -e "${YELLOW}Creating installation directory: $INSTALL_DIR${NC}"
mkdir -p "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR/schemas"
mkdir -p "$INSTALL_DIR/logs"

# Step 4: Copy binary
echo -e "${YELLOW}Installing binary...${NC}"
cp target/release/db-gateway "$INSTALL_DIR/"
chmod 755 "$INSTALL_DIR/db-gateway"

# Step 5: Create .env file if not exists
if [ ! -f "$INSTALL_DIR/.env" ]; then
    echo -e "${YELLOW}Creating default .env file...${NC}"
    cp .env.example "$INSTALL_DIR/.env"
    chmod 600 "$INSTALL_DIR/.env"
    echo -e "${YELLOW}IMPORTANT: Edit $INSTALL_DIR/.env with your PostgreSQL credentials${NC}"
fi

# Step 6: Set ownership
echo -e "${YELLOW}Setting ownership...${NC}"
chown -R "$SERVICE_USER:$SERVICE_GROUP" "$INSTALL_DIR"

# Step 7: Install systemd service
echo -e "${YELLOW}Installing systemd service...${NC}"
cp deploy/db-gateway.service /etc/systemd/system/
systemctl daemon-reload

# Step 8: Enable service
echo -e "${YELLOW}Enabling service...${NC}"
systemctl enable db-gateway

echo ""
echo -e "${GREEN}=== Installation Complete ===${NC}"
echo ""
echo "Next steps:"
echo "  1. Edit /opt/db-gateway/.env with your PostgreSQL credentials"
echo "  2. Start the service: sudo systemctl start db-gateway"
echo "  3. Check status: sudo systemctl status db-gateway"
echo "  4. View logs: sudo journalctl -u db-gateway -f"
echo ""
echo "Service commands:"
echo "  sudo systemctl start db-gateway"
echo "  sudo systemctl stop db-gateway"
echo "  sudo systemctl restart db-gateway"
echo "  sudo systemctl status db-gateway"
echo ""
