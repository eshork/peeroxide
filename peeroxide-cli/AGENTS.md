# AGENTS.md — peeroxide-cli/

This crate implements the `peeroxide` CLI binary with five subcommands: `lookup`, `announce`, `ping`, `cp`, `dd`.

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
│   └── deaddrop/
│       ├── mod.rs    — dd subcommand dispatch + shared helpers (MAX_PAYLOAD, version detection)
│       ├── v1.rs     — v1 single linked-list format
│       └── v2.rs     — v2 two-chain format (immutable data + mutable index)
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
| `MAX_PAYLOAD` | 1000 | deaddrop/mod.rs |
| `MAX_CHUNKS` (v1) | 65535 | deaddrop/v1.rs |
| `ROOT_HEADER_SIZE` (v1) | 39 | deaddrop/v1.rs |
| `NON_ROOT_HEADER_SIZE` (v1) | 33 | deaddrop/v1.rs |
| `VERSION` (v1) | 0x01 | deaddrop/v1.rs |
| `VERSION` (v2) | 0x02 | deaddrop/v2.rs |
| `DATA_PAYLOAD_MAX` (v2) | 999 | deaddrop/v2.rs |
| `ROOT_INDEX_HEADER` (v2) | 41 | deaddrop/v2.rs |
| `NON_ROOT_INDEX_HEADER` (v2) | 33 | deaddrop/v2.rs |
| `PTRS_PER_ROOT` (v2) | 29 | deaddrop/v2.rs |
| `PTRS_PER_NON_ROOT` (v2) | 30 | deaddrop/v2.rs |
| `MAX_DATA_CHUNKS` (v2) | 1,966,079 | deaddrop/v2.rs |
| `MAX_FILE_SIZE` (v2) | 1,964,112,921 | deaddrop/v2.rs |
| `PARALLEL_FETCH_CAP` (v2) | 64 | deaddrop/v2.rs |
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
