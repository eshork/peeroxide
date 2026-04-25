#!/usr/bin/env bash
# M1 Gate: compact-encoding
# Regenerates golden fixtures and runs all M1 tests.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M1: compact-encoding ==="

# Prerequisites
command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

# Install node deps if needed
if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

# Regenerate fixtures
echo "Regenerating golden fixtures..."
(cd "$ROOT/tests/node" && node generate-golden.js)

# Unit tests
echo "Running unit tests..."
cargo test -p peeroxide-dht --lib compact_encoding --manifest-path "$ROOT/Cargo.toml"

# Golden fixture tests
echo "Running golden interop tests..."
cargo test -p peeroxide-dht --test golden_interop --manifest-path "$ROOT/Cargo.toml"

# Clippy
echo "Running clippy on peeroxide-dht..."
cargo clippy -p peeroxide-dht --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo "=== M1 PASSED ==="
