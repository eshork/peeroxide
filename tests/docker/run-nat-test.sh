#!/usr/bin/env bash
# M6 Docker NAT simulation test.
# Builds containers, sets up NAT'd networks, and verifies that
# Rust and Node.js peers can establish connections through simulated NAT.
#
# Prerequisites: Docker with compose plugin, Linux containers
# Usage: bash run-nat-test.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== M6 Docker NAT Simulation Test ==="

command -v docker >/dev/null || { echo "ERROR: docker not found"; exit 1; }
docker compose version >/dev/null 2>&1 || { echo "ERROR: docker compose not found"; exit 1; }

COMPOSE_FILE="$SCRIPT_DIR/docker-compose.nat.yml"

cleanup() {
    echo "Cleaning up containers..."
    docker compose -f "$COMPOSE_FILE" down --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "Building containers..."
docker compose -f "$COMPOSE_FILE" build

echo "Starting NAT simulation..."
docker compose -f "$COMPOSE_FILE" up --abort-on-container-exit --timeout 60

echo "=== M6 Docker NAT Test PASSED ==="
