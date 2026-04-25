#!/usr/bin/env bash
# M2 Gate: libudx FFI
# Runs libudx unit tests and live interop with Node.js udx-native.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M2: libudx FFI ==="

# Prerequisites
command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

# Install node deps if needed
if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

# Unit tests
echo "Running libudx-sys unit tests..."
cargo test -p libudx-sys --lib --manifest-path "$ROOT/Cargo.toml"

echo "Running libudx unit tests..."
cargo test -p libudx --lib --manifest-path "$ROOT/Cargo.toml"

# Live interop
echo "Running live interop (Rust ↔ Node.js UDX echo)..."
cargo test -p libudx --test udx_interop --manifest-path "$ROOT/Cargo.toml"

# Clippy
echo "Running clippy on libudx crates..."
cargo clippy -p libudx-sys -p libudx --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo "=== M2 PASSED ==="
