#!/usr/bin/env bash
# M5 Gate: Encrypted Connections
# Regenerates noise/secretstream fixtures, runs golden + live interop tests.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M5: Encrypted Connections ==="

# Prerequisites
command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

# Install node deps if needed
if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

# sodium-secretstream requires native addon — verify it built
if [ ! -d "$ROOT/tests/node/node_modules/sodium-secretstream" ]; then
  echo "ERROR: sodium-secretstream not installed. Run: cd tests/node && npm install"
  exit 1
fi

# Regenerate fixtures
echo "Regenerating Noise + secretstream golden fixtures..."
(cd "$ROOT/tests/node" && node generate-noise-golden.js)

# Unit tests (noise, secretstream modules)
echo "Running unit tests..."
cargo test -p peeroxide-dht --lib noise --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib secretstream --manifest-path "$ROOT/Cargo.toml"

# Golden fixture tests
echo "Running golden interop tests..."
cargo test -p peeroxide-dht --test noise_golden_interop --manifest-path "$ROOT/Cargo.toml"

# Live interop (Rust ↔ Node.js encrypted stream over TCP)
echo "Running live interop (Rust ↔ Node.js encrypted stream)..."
cargo test -p peeroxide-dht --test secret_stream_interop --manifest-path "$ROOT/Cargo.toml"

# Clippy
echo "Running clippy..."
cargo clippy -p peeroxide-dht --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo "=== M5 PASSED ==="
