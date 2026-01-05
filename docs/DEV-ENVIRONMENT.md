# Development Environment Setup

---
**📖 Navigation:** [Home](../README.md) | [Quick Start](QUICKSTART.md) | [Integration](INTEGRATION.md) | [HLD](../HLD.md) | **Dev Setup** | [API v2](API-V2.md)

---

This document describes how to set up a local development environment for stonescriptdb-gateway using a libvirt VM.

**Note:** This guide uses `192.168.122.173` as an example IP. Your VM may get a different IP address from the libvirt DHCP server. Replace this IP throughout the guide with your actual VM IP.

## Overview

The dev environment mirrors production:
- Gateway runs on a VM (not in Docker) to avoid rootless Docker networking issues
- PostgreSQL runs on the same VM as the gateway
- Docker containers connect to the gateway via the VM's IP
- Clean separation between compute (containers) and data (database)

## VM Setup

### Create libvirt VM

1. Create Ubuntu 22.04 VM using virt-manager or virsh
2. Name: `devvmlocal` (or any name you prefer)
3. Specs: 2 vCPUs, 4GB RAM, 40GB disk (minimum)
4. Network: Default NAT (192.168.122.0/24 or your libvirt default network)

**Find your VM's IP:**
```bash
# After VM is running
virsh net-dhcp-leases default
# Or check from VM console:
# ip addr show
```

### Initial VM Configuration

```bash
# On the VM
sudo apt update && sudo apt upgrade -y
sudo apt install -y openssh-server postgresql curl build-essential

# Create devops user if not exists
sudo useradd -m -s /bin/bash devops
echo "devops:devops" | sudo chpasswd
sudo usermod -aG sudo devops
```

### SSH Access Setup

On your host machine:

```bash
# Generate SSH key for dev VM access
ssh-keygen -t ed25519 -f ~/.ssh/id_devvm -C "devvm-access"

# Copy key to VM (use password 'devops')
ssh-copy-id -i ~/.ssh/id_devvm.pub devops@192.168.122.173

# Add to SSH config
cat >> ~/.ssh/config << 'EOF'

Host devvmlocal
    HostName 192.168.122.173
    User devops
    IdentityFile ~/.ssh/id_devvm
EOF
```

Test connection:
```bash
ssh devvmlocal "hostname"
```

## PostgreSQL Configuration

### Create Gateway User and Database

```bash
ssh devvmlocal "sudo -u postgres psql -c \"CREATE USER gateway_user WITH PASSWORD 'gateway_password' CREATEDB;\""
ssh devvmlocal "sudo -u postgres psql -c \"CREATE DATABASE gateway_test OWNER gateway_user;\""
```

### Enable Remote Connections

```bash
# Edit postgresql.conf to listen on all interfaces
ssh devvmlocal "sudo sed -i \"s/#listen_addresses = 'localhost'/listen_addresses = '*'/\" /etc/postgresql/*/main/postgresql.conf"

# Add pg_hba.conf entry for remote connections
ssh devvmlocal "sudo bash -c 'echo \"host all all 0.0.0.0/0 md5\" >> /etc/postgresql/*/main/pg_hba.conf'"

# Restart PostgreSQL
ssh devvmlocal "sudo systemctl restart postgresql"
```

### Verify PostgreSQL

```bash
# Test local connection on VM
ssh devvmlocal "psql -U gateway_user -d gateway_test -c 'SELECT version();'"

# Test remote connection from host
PGPASSWORD='gateway_password' psql -h 192.168.122.173 -U gateway_user -d gateway_test -c 'SELECT 1;'
```

## Gateway Deployment

### Build Gateway Binary

```bash
# In your project directory
cd stonescriptdb-gateway

# Build release binary
cargo build --release

# Or use Docker for cross-compilation (Ubuntu 22.04 compatible)
docker build -f Dockerfile.build -t stonescriptdb-gateway-builder .
docker run --rm -v "$PWD/output:/output" stonescriptdb-gateway-builder
```

### Deploy to VM

```bash
# Create directories on VM
ssh devvmlocal "sudo mkdir -p /opt/stonescriptdb-gateway && sudo chown devops:devops /opt/stonescriptdb-gateway"
ssh devvmlocal "sudo mkdir -p /var/log/stonescriptdb-gateway && sudo chown devops:devops /var/log/stonescriptdb-gateway"

# Copy binary
scp target/release/stonescriptdb-gateway devvmlocal:/opt/stonescriptdb-gateway/

# Create .env file
ssh devvmlocal "cat > /opt/stonescriptdb-gateway/.env << 'EOF'
# Gateway Configuration
GATEWAY_HOST=0.0.0.0
GATEWAY_PORT=9000

# PostgreSQL Connection
DB_HOST=127.0.0.1
DB_PORT=5432
DB_USER=gateway_user
DB_PASSWORD=gateway_password

# Connection Pool
MAX_CONNECTIONS_PER_POOL=10
MAX_TOTAL_CONNECTIONS=200

# Logging
RUST_LOG=info

# Network Security (allow local Docker networks)
ALLOWED_NETWORKS=192.168.0.0/16,172.16.0.0/12,10.0.0.0/8
EOF"
```

### Create systemd Service

```bash
ssh devvmlocal "sudo tee /etc/systemd/system/stonescriptdb-gateway.service << 'EOF'
[Unit]
Description=StoneScriptDB Gateway
After=network.target postgresql.service
Wants=postgresql.service

[Service]
Type=simple
User=devops
WorkingDirectory=/opt/stonescriptdb-gateway
EnvironmentFile=/opt/stonescriptdb-gateway/.env
ExecStart=/opt/stonescriptdb-gateway/stonescriptdb-gateway
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF"

ssh devvmlocal "sudo systemctl daemon-reload"
ssh devvmlocal "sudo systemctl enable stonescriptdb-gateway"
ssh devvmlocal "sudo systemctl start stonescriptdb-gateway"
```

### Verify Gateway

```bash
# Check service status
ssh devvmlocal "sudo systemctl status stonescriptdb-gateway"

# Test health endpoint from VM
ssh devvmlocal "curl -s http://localhost:9000/health"

# Test from host
curl -s http://192.168.122.173:9000/health
```

## Docker Integration

### Configure Docker Containers

In your `docker-compose.yaml`, use `extra_hosts` or set environment variables to point to the gateway:

```yaml
services:
  myapp:
    image: myapp:latest
    environment:
      - DB_GATEWAY_URL=http://192.168.122.173:9000
    extra_hosts:
      - "gateway:192.168.122.173"
```

### Test from Docker Container

```bash
# Create test container
docker run --rm -it alpine sh -c "apk add curl && curl -s http://192.168.122.173:9000/health"
```

## Network Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Host Machine                                                │
│  ┌─────────────────────┐    ┌─────────────────────────────┐ │
│  │  Docker Containers  │    │  libvirt VM (devvmlocal)    │ │
│  │  172.17.0.0/16      │    │  192.168.122.173            │ │
│  │                     │    │                             │ │
│  │  ┌───────────────┐  │    │  ┌─────────────────────┐   │ │
│  │  │ App Container │──┼────┼──│ Gateway :9000       │   │ │
│  │  └───────────────┘  │    │  └──────────┬──────────┘   │ │
│  │                     │    │             │              │ │
│  │                     │    │  ┌──────────▼──────────┐   │ │
│  │                     │    │  │ PostgreSQL :5432    │   │ │
│  │                     │    │  └─────────────────────┘   │ │
│  └─────────────────────┘    └─────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## Troubleshooting

### Cannot connect to PostgreSQL from host

```bash
# Check PostgreSQL is listening
ssh devvmlocal "ss -tlnp | grep 5432"

# Check firewall
ssh devvmlocal "sudo ufw status"
# If active, allow PostgreSQL:
ssh devvmlocal "sudo ufw allow 5432/tcp"
```

### Cannot connect to gateway from Docker

```bash
# Check gateway is running
ssh devvmlocal "sudo systemctl status stonescriptdb-gateway"

# Check firewall
ssh devvmlocal "sudo ufw allow 9000/tcp"

# Check from container
docker run --rm alpine sh -c "apk add curl && curl -v http://192.168.122.173:9000/health"
```

### Gateway cannot connect to PostgreSQL

```bash
# Check logs
ssh devvmlocal "sudo journalctl -u stonescriptdb-gateway -f"

# Verify PostgreSQL credentials
ssh devvmlocal "PGPASSWORD='gateway_password' psql -U gateway_user -d gateway_test -c 'SELECT 1;'"
```

## Quick Reference

| Component   | Location/Port                    |
|-------------|----------------------------------|
| VM IP       | 192.168.122.173                  |
| Gateway     | http://192.168.122.173:9000      |
| PostgreSQL  | 192.168.122.173:5432             |
| SSH         | ssh devvmlocal                   |
| Binary      | /opt/stonescriptdb-gateway/      |
| Logs (systemd) | journalctl -u stonescriptdb-gateway |
| Logs (files)   | /var/log/stonescriptdb-gateway/     |

## Credentials (Dev Only)

| Service     | User         | Password          |
|-------------|--------------|-------------------|
| VM SSH      | devops       | devops            |
| PostgreSQL  | gateway_user | gateway_password  |
