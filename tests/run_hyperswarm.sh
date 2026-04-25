#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M8: Hyperswarm ==="

command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

echo "Running M8 unit tests (peeroxide crate)..."
cargo test -p peeroxide --lib --manifest-path "$ROOT/Cargo.toml"

echo "Running M8 live interop test (Hyperswarm cross-language connect)..."
cargo test -p peeroxide --test hyperswarm_interop --manifest-path "$ROOT/Cargo.toml"

echo "Running full workspace test suite (370 tests)..."
cargo test --workspace --manifest-path "$ROOT/Cargo.toml"

echo "Running clippy..."
cargo clippy --workspace --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo ""
echo "M8 gate: PASS"
echo ""
