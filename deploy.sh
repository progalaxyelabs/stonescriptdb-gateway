#!/bin/bash
# =============================================================================
# StoneScriptDB Gateway Deploy Script
# =============================================================================
# Deploys the gateway binary to /opt/stonescriptdb-gateway
#
# Usage:
#   sudo ./deploy.sh          # Deploy pre-built binary
#   sudo ./deploy.sh --stop   # Stop the service
#   sudo ./deploy.sh --status # Show service status
#   sudo ./deploy.sh --force  # Deploy even if older version
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
INSTALL_DIR="/opt/stonescriptdb-gateway"
LOG_DIR="/var/log/stonescriptdb-gateway"
SERVICE_USER="stonescriptdb-gateway"
BINARY_PATH="output/stonescriptdb-gateway"
INSTALLED_BINARY="$INSTALL_DIR/stonescriptdb-gateway"

# Function to extract version from binary
get_binary_version() {
    local binary="$1"
    if [ -f "$binary" ] && [ -x "$binary" ]; then
        # Try to get version from health endpoint or --version flag
        # For now, check the binary's embedded version via strings
        strings "$binary" 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | head -1 || echo "unknown"
    else
        echo "not-found"
    fi
}

# Function to get version from running service
get_running_version() {
    if systemctl is-active --quiet stonescriptdb-gateway 2>/dev/null; then
        curl -s http://127.0.0.1:9000/health 2>/dev/null | grep -o '"version":"[^"]*"' | cut -d'"' -f4 || echo "unknown"
    else
        echo "not-running"
    fi
}

# Function to get code version from Cargo.toml
get_code_version() {
    if [ -f "Cargo.toml" ]; then
        grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/'
    else
        echo "unknown"
    fi
}

# Function to compare versions (returns 0 if v1 >= v2, 1 if v1 < v2)
version_gte() {
    local v1="$1"
    local v2="$2"

    # Handle special cases
    if [ "$v1" = "unknown" ] || [ "$v1" = "not-found" ]; then
        return 1
    fi
    if [ "$v2" = "unknown" ] || [ "$v2" = "not-running" ] || [ "$v2" = "not-found" ]; then
        return 0
    fi

    # Compare versions using sort -V
    [ "$(printf '%s\n%s' "$v1" "$v2" | sort -V | tail -1)" = "$v1" ]
}

# Parse arguments
FORCE_DEPLOY=false
case "$1" in
    --stop|-s)
        if [ "$EUID" -ne 0 ]; then
            echo -e "${RED}Error: --stop requires root (sudo ./deploy.sh --stop)${NC}"
            exit 1
        fi
        echo -e "${YELLOW}Stopping stonescriptdb-gateway service...${NC}"
        if systemctl is-active --quiet stonescriptdb-gateway 2>/dev/null; then
            systemctl stop stonescriptdb-gateway
            echo -e "${GREEN}Service stopped.${NC}"
        else
            echo -e "${YELLOW}Service is not running.${NC}"
        fi
        exit 0
        ;;
    --status)
        echo -e "${BLUE}=== StoneScriptDB Gateway Status ===${NC}"
        echo ""

        # Code version
        CODE_VERSION=$(get_code_version)
        echo -e "Code version (Cargo.toml):     ${BLUE}$CODE_VERSION${NC}"

        # Binary version
        if [ -f "$BINARY_PATH" ]; then
            echo -e "Built binary (output/):        ${BLUE}exists${NC}"
        else
            echo -e "Built binary (output/):        ${YELLOW}not found - run ./build.sh${NC}"
        fi

        # Installed version
        if [ -f "$INSTALLED_BINARY" ]; then
            echo -e "Installed binary:              ${BLUE}exists${NC}"
        else
            echo -e "Installed binary:              ${YELLOW}not installed${NC}"
        fi

        # Running version
        RUNNING_VERSION=$(get_running_version)
        if [ "$RUNNING_VERSION" = "not-running" ]; then
            echo -e "Running version:               ${YELLOW}service not running${NC}"
        else
            echo -e "Running version:               ${GREEN}$RUNNING_VERSION${NC}"
        fi

        echo ""
        systemctl status stonescriptdb-gateway --no-pager 2>/dev/null || echo "Service not installed"
        exit 0
        ;;
    --force|-f)
        FORCE_DEPLOY=true
        ;;
    --help|-h)
        echo "Usage: sudo ./deploy.sh [OPTION]"
        echo ""
        echo "Options:"
        echo "  (none)        Deploy pre-built binary"
        echo "  --force, -f   Deploy even if downgrading version"
        echo "  --stop, -s    Stop the service"
        echo "  --status      Show version and service status"
        echo "  --help, -h    Show this help message"
        echo ""
        echo "Build first: ./build.sh"
        exit 0
        ;;
esac

# Deploy mode - requires root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Error: Please run as root (sudo ./deploy.sh)${NC}"
    exit 1
fi

# Check if binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo -e "${RED}Error: Binary not found at $BINARY_PATH${NC}"
    echo ""
    echo "Run './build.sh' first to build the binary"
    exit 1
fi

# Get versions
CODE_VERSION=$(get_code_version)
RUNNING_VERSION=$(get_running_version)

echo -e "${BLUE}=== Version Check ===${NC}"
echo -e "Code version:    ${BLUE}$CODE_VERSION${NC}"
if [ "$RUNNING_VERSION" = "not-running" ]; then
    echo -e "Running version: ${YELLOW}service not running${NC}"
else
    echo -e "Running version: ${GREEN}$RUNNING_VERSION${NC}"
fi
echo ""

# Version comparison warnings
if [ "$RUNNING_VERSION" != "not-running" ] && [ "$RUNNING_VERSION" != "unknown" ]; then
    if ! version_gte "$CODE_VERSION" "$RUNNING_VERSION"; then
        echo -e "${RED}WARNING: You are deploying an OLDER version!${NC}"
        echo -e "  Deploying: $CODE_VERSION"
        echo -e "  Currently: $RUNNING_VERSION"
        echo ""
        if [ "$FORCE_DEPLOY" = false ]; then
            echo -e "${RED}Use --force to deploy anyway${NC}"
            exit 1
        fi
        echo -e "${YELLOW}Proceeding with --force flag...${NC}"
        echo ""
    elif [ "$CODE_VERSION" = "$RUNNING_VERSION" ]; then
        echo -e "${YELLOW}NOTE: Deploying same version ($CODE_VERSION)${NC}"
        echo ""
    else
        echo -e "${GREEN}Upgrading: $RUNNING_VERSION -> $CODE_VERSION${NC}"
        echo ""
    fi
fi

echo -e "${GREEN}=== StoneScriptDB Gateway Deployment ===${NC}"

# Stop service if running
if systemctl is-active --quiet stonescriptdb-gateway 2>/dev/null; then
    echo -e "${YELLOW}Stopping existing service...${NC}"
    systemctl stop stonescriptdb-gateway
fi

# Create service user if not exists
if ! id "$SERVICE_USER" &>/dev/null; then
    echo -e "${YELLOW}Creating service user: $SERVICE_USER${NC}"
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi

# Create directories
echo -e "${YELLOW}Creating directories...${NC}"
mkdir -p "$INSTALL_DIR/schemas"
mkdir -p "$INSTALL_DIR/data"     # Schema storage for v2 API
mkdir -p "$INSTALL_DIR/logs"
mkdir -p "$LOG_DIR"

# Copy binary
echo -e "${YELLOW}Installing binary...${NC}"
cp "$BINARY_PATH" "$INSTALL_DIR/"
chmod 755 "$INSTALL_DIR/stonescriptdb-gateway"

# Create .env file if not exists
if [ ! -f "$INSTALL_DIR/.env" ]; then
    echo -e "${YELLOW}Creating default .env file...${NC}"
    cat > "$INSTALL_DIR/.env" << 'EOF'
# PostgreSQL connection
DB_HOST=localhost
DB_PORT=5432
DB_NAME=postgres
DB_USER=gateway_user
DB_PASSWORD=p@ssw0rd

# Server settings
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=9000

# Connection pool settings
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200
POOL_IDLE_TIMEOUT_SECS=1800
POOL_MAX_LIFETIME_SECS=3600

# Security
ALLOWED_NETWORKS=127.0.0.0/8,::1/128,10.0.1.0/24

# Schema storage (v2 API)
DATA_DIR=/opt/stonescriptdb-gateway/data

# Logging
LOG_DIR=/var/log/stonescriptdb-gateway
RUST_LOG=info,stonescriptdb_gateway=debug
EOF
    chmod 600 "$INSTALL_DIR/.env"
    echo -e "${YELLOW}NOTE: Edit $INSTALL_DIR/.env if you need different settings${NC}"
else
    echo -e "${YELLOW}Existing .env preserved.${NC}"
fi

# Set ownership
echo -e "${YELLOW}Setting ownership...${NC}"
chown -R "$SERVICE_USER:$SERVICE_USER" "$INSTALL_DIR"
chown -R "$SERVICE_USER:$SERVICE_USER" "$LOG_DIR"

# Install systemd service
echo -e "${YELLOW}Installing systemd service...${NC}"
cp deploy/stonescriptdb-gateway.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable stonescriptdb-gateway

# Start service
echo -e "${YELLOW}Starting service...${NC}"
systemctl start stonescriptdb-gateway

# Wait a moment and check status
sleep 2

if systemctl is-active --quiet stonescriptdb-gateway; then
    # Get new running version
    NEW_RUNNING_VERSION=$(get_running_version)

    echo ""
    echo -e "${GREEN}=== Deployment Successful ===${NC}"
    echo ""
    echo -e "Deployed version: ${GREEN}$NEW_RUNNING_VERSION${NC}"
    echo ""
    systemctl status stonescriptdb-gateway --no-pager
    echo ""
    echo "Service is running on http://127.0.0.1:9000"
    echo ""
    echo "Commands:"
    echo "  ./deploy.sh --status"
    echo "  systemctl status stonescriptdb-gateway"
    echo "  journalctl -u stonescriptdb-gateway -f"
    echo "  tail -f $LOG_DIR/stonescriptdb-gateway.log"
else
    echo ""
    echo -e "${RED}=== Deployment Failed ===${NC}"
    echo ""
    echo "Check logs with: journalctl -u stonescriptdb-gateway -n 50"
    exit 1
fi
