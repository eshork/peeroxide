#!/usr/bin/env bash
set -euo pipefail

ROLE="${1:?Usage: run-peer.sh <rust|node>}"
BOOTSTRAP_HOST="${BOOTSTRAP_HOST:-172.30.0.10}"
BOOTSTRAP_PORT="${BOOTSTRAP_PORT:-49737}"

ip route add default via "${GATEWAY_IP:-10.0.1.1}" 2>/dev/null || true

case "$ROLE" in
  rust)
    echo "Starting Rust peer (bootstrap=${BOOTSTRAP_HOST}:${BOOTSTRAP_PORT})"
    exec /usr/local/bin/peeroxide \
      --bootstrap "${BOOTSTRAP_HOST}:${BOOTSTRAP_PORT}" \
      --mode server
    ;;
  node)
    echo "Starting Node.js peer (bootstrap=${BOOTSTRAP_HOST}:${BOOTSTRAP_PORT})"
    exec node hyperdht-peer.js
    ;;
  *)
    echo "Unknown role: $ROLE"
    exit 1
    ;;
esac
