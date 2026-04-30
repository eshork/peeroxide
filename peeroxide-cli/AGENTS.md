# AGENTS.md — peeroxide-cli/

This crate implements the `peeroxide` CLI binary with five subcommands: `lookup`, `announce`, `ping`, `cp`, `deaddrop`.

## Source Layout

```
src/
├── main.rs           — CLI entry point, subcommand dispatch
├── cmd/
│   ├── mod.rs        — Shared helpers: parse_topic, build_dht_config, to_hex, discovery_key
│   ├── lookup.rs     — lookup subcommand
│   ├── announce.rs   — announce subcommand + echo protocol server
│   ├── ping.rs       — ping subcommand (bootstrap check, direct, pubkey, topic, --connect)
│   ├── cp.rs         — cp subcommand (send/recv file transfer over swarm)
│   └── deaddrop.rs   — deaddrop subcommand (mutable DHT store-and-forward)
```

## Key Shared Helpers (cmd/mod.rs)

- `parse_topic(s)`: 64-char hex → raw 32-byte key; anything else → `discovery_key(s.as_bytes())` (BLAKE2b-256).
- `build_dht_config(args)`: Constructs `DhtConfig` from CLI flags.
- `to_hex(bytes)`: Lowercase hex encoding.
- `discovery_key(data)`: BLAKE2b-256 hash, returns `[u8; 32]`.

## Important Constants

| Constant | Value | File |
|---|---|---|
| `PING_MAGIC` | `b"PING"` | announce.rs |
| `PONG_MAGIC` | `b"PONG"` | announce.rs |
| `MAX_ECHO_SESSIONS` | 64 | announce.rs |
| `HANDSHAKE_TIMEOUT` | 5s | announce.rs |
| `IDLE_TIMEOUT` | 30s | announce.rs |
| `ECHO_MSG_LEN` | 16 | announce.rs |
| `ECHO_TIMEOUT` | 5s | ping.rs |
| `MAX_CHUNKS` | 65535 | deaddrop.rs |
| `MAX_PAYLOAD` | 1000 | deaddrop.rs |
| `ROOT_HEADER_SIZE` | 39 | deaddrop.rs |
| `NON_ROOT_HEADER_SIZE` | 33 | deaddrop.rs |
| `CHUNK_SIZE` | 65536 | cp.rs |

## Known Issues

See `ISSUES.md` at the workspace root for tracked source-level issues discovered during documentation.

## Documentation

Full CLI documentation lives in `../docs/`. Build with `mdbook build docs/` from the workspace root.

## Testing

```bash
cargo test -p peeroxide-cli
```

Integration tests are in `tests/`. They require network access (bootstrap nodes) for DHT-dependent tests.
