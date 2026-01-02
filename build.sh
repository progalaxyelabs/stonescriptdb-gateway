#!/bin/bash
# =============================================================================
# StoneScriptDB Gateway Build Script
# =============================================================================
# Builds the gateway binary using Docker for Ubuntu 22.04 compatibility
#
# Usage:
#   ./build.sh              # Build release binary
#   ./build.sh --clean      # Clean and rebuild
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Configuration
BINARY_PATH="output/stonescriptdb-gateway"

# Parse arguments
case "$1" in
    --clean|-c)
        echo -e "${YELLOW}Cleaning previous build...${NC}"
        rm -rf output/
        docker rmi stonescriptdb-gateway-builder 2>/dev/null || true
        ;;
    --help|-h)
        echo "Usage: ./build.sh [OPTION]"
        echo ""
        echo "Options:"
        echo "  (none)        Build release binary using Docker"
        echo "  --clean, -c   Clean and rebuild from scratch"
        echo "  --help, -h    Show this help message"
        exit 0
        ;;
esac

# Check for Dockerfile.build
if [ ! -f "Dockerfile.build" ]; then
    echo -e "${RED}Error: Dockerfile.build not found${NC}"
    echo "Copy Dockerfile.build.example to Dockerfile.build first"
    exit 1
fi

# Get version from Cargo.toml
CODE_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
echo -e "${YELLOW}Building version: ${CODE_VERSION}${NC}"

# Build the builder image
echo -e "${YELLOW}Building Docker image...${NC}"
docker build -f Dockerfile.build -t stonescriptdb-gateway-builder .

# Create output directory
mkdir -p output

# Run the build
echo -e "${YELLOW}Running build container...${NC}"
docker run --rm -v "$PWD/output:/output" stonescriptdb-gateway-builder

if [ -f "$BINARY_PATH" ]; then
    # Get the built binary version
    BUILT_VERSION=$("$BINARY_PATH" --version 2>/dev/null | awk '{print $2}' || echo "unknown")

    echo ""
    echo -e "${GREEN}=== Build Successful ===${NC}"
    echo "Binary: $BINARY_PATH"
    echo "Version: $CODE_VERSION"
    echo ""
    echo "Next step: sudo ./deploy.sh"
else
    echo -e "${RED}Build failed: binary not found${NC}"
    exit 1
fi
