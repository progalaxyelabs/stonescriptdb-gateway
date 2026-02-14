#!/bin/bash
set -e

# Configuration
REMOTE_HOST="your-server"
TEST_GATEWAY_DIR="/opt/test-stonescriptdb-gateway"
TEST_PORT="9001"
GATEWAY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "======================================"
echo "Deploy Test Gateway to your-server"
echo "======================================"
echo ""
echo "Configuration:"
echo "  Remote Host: $REMOTE_HOST"
echo "  Test Gateway Dir: $TEST_GATEWAY_DIR"
echo "  Test Port: $TEST_PORT"
echo "  Source: $GATEWAY_ROOT"
echo ""

# Check if binary exists
if [ ! -f "$GATEWAY_ROOT/target/release/stonescriptdb-gateway" ]; then
    echo "ERROR: Release binary not found. Building..."
    cd "$GATEWAY_ROOT"
    cargo build --release
fi

echo "[1/5] Creating test gateway directory on your-server..."
ssh "$REMOTE_HOST" "mkdir -p $TEST_GATEWAY_DIR/logs"

echo "[2/5] Copying binary to your-server..."
scp "$GATEWAY_ROOT/target/release/stonescriptdb-gateway" "$REMOTE_HOST:$TEST_GATEWAY_DIR/"

echo "[3/5] Creating test configuration file..."
ssh "$REMOTE_HOST" "cat > $TEST_GATEWAY_DIR/.env << 'EOF'
# Test Gateway Configuration
GATEWAY_HOST=0.0.0.0
GATEWAY_PORT=$TEST_PORT
DATABASE_HOST=localhost
DATABASE_PORT=5432
DATABASE_USER=postgres
DATABASE_PASSWORD=p@ssw0rd
POOL_MAX_SIZE=10
POOL_TIMEOUT_SECONDS=30
POOL_MAX_LIFETIME_SECONDS=1800
LOG_LEVEL=info
RUST_LOG=stonescriptdb_gateway=debug,info
EOF
"

echo "[4/5] Creating start/stop scripts..."
ssh "$REMOTE_HOST" "cat > $TEST_GATEWAY_DIR/start.sh << 'EOF'
#!/bin/bash
cd $TEST_GATEWAY_DIR
if [ -f gateway.pid ] && ps -p \\\$(cat gateway.pid) > /dev/null 2>&1; then
    echo \"Test gateway is already running (PID: \\\$(cat gateway.pid))\"
    exit 1
fi

echo \"Starting test gateway on port $TEST_PORT...\"
nohup ./stonescriptdb-gateway > logs/gateway.log 2>&1 &
echo \\\$! > gateway.pid
sleep 2

if ps -p \\\$(cat gateway.pid) > /dev/null 2>&1; then
    echo \"✓ Test gateway started successfully (PID: \\\$(cat gateway.pid))\"
    echo \"✓ Listening on: http://0.0.0.0:$TEST_PORT\"
    echo \"✓ Logs: $TEST_GATEWAY_DIR/logs/gateway.log\"
else
    echo \"✗ Failed to start gateway. Check logs:\"
    tail -20 logs/gateway.log
    exit 1
fi
EOF
"

ssh "$REMOTE_HOST" "cat > $TEST_GATEWAY_DIR/stop.sh << 'EOF'
#!/bin/bash
cd $TEST_GATEWAY_DIR
if [ ! -f gateway.pid ]; then
    echo \"No PID file found\"
    exit 1
fi

PID=\\\$(cat gateway.pid)
if ps -p \\\$PID > /dev/null 2>&1; then
    echo \"Stopping test gateway (PID: \\\$PID)...\"
    kill \\\$PID
    sleep 2
    if ps -p \\\$PID > /dev/null 2>&1; then
        echo \"Force killing...\"
        kill -9 \\\$PID
    fi
    rm gateway.pid
    echo \"✓ Test gateway stopped\"
else
    echo \"Gateway not running\"
    rm gateway.pid
fi
EOF
"

ssh "$REMOTE_HOST" "chmod +x $TEST_GATEWAY_DIR/*.sh"

echo "[5/5] Starting test gateway..."
ssh "$REMOTE_HOST" "cd $TEST_GATEWAY_DIR && ./start.sh"

echo ""
echo "======================================"
echo "Deployment Complete!"
echo "======================================"
echo ""
echo "Test gateway is now running at:"
echo "  http://localhost:$TEST_PORT"
echo ""
echo "Management commands (run via SSH):"
echo "  Start:  ssh $REMOTE_HOST '$TEST_GATEWAY_DIR/start.sh'"
echo "  Stop:   ssh $REMOTE_HOST '$TEST_GATEWAY_DIR/stop.sh'"
echo "  Logs:   ssh $REMOTE_HOST 'tail -f $TEST_GATEWAY_DIR/logs/gateway.log'"
echo "  Status: ssh $REMOTE_HOST 'ps aux | grep test-stonescriptdb-gateway'"
echo ""
echo "To run E2E test:"
echo "  GATEWAY_URL=http://localhost:$TEST_PORT ./tests/e2e-return-type-change/test-return-type-migration.sh"
echo ""
