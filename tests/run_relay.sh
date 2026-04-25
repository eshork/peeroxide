#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M7: Relay ==="

command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

echo "Regenerating Protomux and Blind-Relay golden fixtures..."
(cd "$ROOT/tests/node" && node generate-protomux-golden.js)
(cd "$ROOT/tests/node" && node generate-blind-relay-golden.js)

echo "Running M7 unit tests..."
cargo test -p peeroxide-dht --lib protomux --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib blind_relay --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib establish_stream --manifest-path "$ROOT/Cargo.toml"

echo "Running M7 golden interop tests..."
cargo test -p peeroxide-dht --test protomux_golden_interop --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --test blind_relay_golden_interop --manifest-path "$ROOT/Cargo.toml"

echo "Running M7 integration test (Protomux cross-language interop)..."
cargo test -p peeroxide-dht --test protomux_interop --manifest-path "$ROOT/Cargo.toml"

echo "Running full workspace test suite (342 tests)..."
cargo test --workspace --manifest-path "$ROOT/Cargo.toml"

echo "Running clippy..."
cargo clippy --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo ""
echo "M7 gate: PASS"
echo ""
