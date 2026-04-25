#!/usr/bin/env bash
# M4 Gate: HyperDHT Operations
# Regenerates fixtures, runs golden + live interop tests.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M4: HyperDHT Operations ==="

# Prerequisites
command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

# Install node deps if needed
if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

# Regenerate fixtures
echo "Regenerating HyperDHT golden fixtures..."
(cd "$ROOT/tests/node" && node generate-hyperdht-golden.js)

# Golden fixture tests
echo "Running golden interop tests..."
cargo test -p peeroxide-dht --test hyperdht_golden_interop --manifest-path "$ROOT/Cargo.toml"

# Live interop
echo "Running live interop (Rust HyperDHT ↔ Node.js hyperdht)..."
cargo test -p peeroxide-dht --test hyperdht_interop --manifest-path "$ROOT/Cargo.toml"

# Clippy
echo "Running clippy..."
cargo clippy -p peeroxide-dht --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo "=== M4 PASSED ==="
