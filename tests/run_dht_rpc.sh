#!/usr/bin/env bash
# M3 Gate: DHT-RPC
# Regenerates fixtures, runs golden + live interop tests.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M3: DHT-RPC ==="

# Prerequisites
command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

# Install node deps if needed
if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

# Regenerate fixtures
echo "Regenerating DHT-RPC golden fixtures..."
(cd "$ROOT/tests/node" && node generate-dht-golden.js)

# Unit tests (rpc, messages, routing_table, peer, io)
echo "Running unit tests..."
cargo test -p peeroxide-dht --lib --manifest-path "$ROOT/Cargo.toml"

# Golden fixture tests
echo "Running golden interop tests..."
cargo test -p peeroxide-dht --test dht_golden_interop --manifest-path "$ROOT/Cargo.toml"

# Live interop
echo "Running live interop (Rust DHT ↔ Node.js dht-rpc)..."
cargo test -p peeroxide-dht --test dht_interop --manifest-path "$ROOT/Cargo.toml"

# Clippy
echo "Running clippy..."
cargo clippy -p peeroxide-dht --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo "=== M3 PASSED ==="
