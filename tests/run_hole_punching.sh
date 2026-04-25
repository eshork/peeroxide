#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== M6: Hole-Punching ==="

command -v cargo >/dev/null || { echo "ERROR: cargo not found"; exit 1; }
command -v node  >/dev/null || { echo "ERROR: node not found"; exit 1; }

if [ ! -d "$ROOT/tests/node/node_modules" ]; then
  echo "Installing Node.js test dependencies..."
  (cd "$ROOT/tests/node" && npm install)
fi

echo "Regenerating Noise IK golden fixtures..."
(cd "$ROOT/tests/node" && node generate-noise-ik-golden.js)

echo "Running M6 unit tests..."
cargo test -p peeroxide-dht --lib nat --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib socket_pool --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib router --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib holepuncher --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib noise_wrap --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --lib secure_payload --manifest-path "$ROOT/Cargo.toml"

echo "Running M6 golden interop tests..."
cargo test -p peeroxide-dht --test noise_ik_golden_interop --manifest-path "$ROOT/Cargo.toml"
cargo test -p peeroxide-dht --test secure_payload_golden_interop --manifest-path "$ROOT/Cargo.toml"

echo "Running M6 integration test (2-node Noise IK handshake)..."
cargo test -p peeroxide-dht --test hyperdht_connect_interop --manifest-path "$ROOT/Cargo.toml"

echo "Running clippy..."
cargo clippy -p peeroxide-dht --all-targets --manifest-path "$ROOT/Cargo.toml" -- -D warnings

echo ""
echo "=== M6 core tests PASSED ==="
echo ""
echo "NOTE: Docker NAT simulation tests are available but require Docker with"
echo "Linux containers and NET_ADMIN capability. Run manually with:"
echo "  bash peeroxide/tests/docker/run-nat-test.sh"
