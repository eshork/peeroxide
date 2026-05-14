# AGENTS.md — peeroxide-cli/

This crate implements the `peeroxide` CLI binary with eight subcommands: `init`, `node`, `lookup`, `announce`, `ping`, `cp`, `dd`, `chat`.

## Source Layout

```
src/
├── main.rs           — CLI entry point, global flag parsing, subcommand dispatch
├── config.rs         — TOML config schema + load precedence
├── manpage.rs        — roff man-page generation (peeroxide(1) + per-subcommand pages)
├── cmd/
│   ├── mod.rs        — Shared helpers: parse_topic, resolve_bootstrap, to_hex, discovery_key
│   ├── init.rs       — peeroxide init (config bootstrap + man-page install)
│   ├── node.rs       — node subcommand (long-running DHT bootstrap node)
│   ├── lookup.rs     — lookup subcommand
│   ├── announce.rs   — announce subcommand + echo protocol server
│   ├── ping.rs       — ping subcommand (bootstrap check, direct, pubkey, topic, --connect)
│   ├── cp.rs         — cp subcommand (send/recv file transfer over swarm)
│   ├── deaddrop/
│   │   ├── mod.rs    — dd subcommand dispatch + shared helpers
│   │   ├── v1.rs     — v1 (0x01) single linked-list format
│   │   ├── v2/       — v2 (0x02) tree-indexed protocol
│   │   │   ├── mod.rs, build.rs, fetch.rs, keys.rs, need.rs, publish.rs,
│   │   │   ├── queue.rs, stream.rs, tree.rs, wire.rs
│   │   └── progress/ — TTY-aware bar / JSON / log / off mode + state
│   └── chat/
│       ├── mod.rs, crypto.rs, debug.rs, display.rs, dm.rs, dm_cmd.rs,
│       ├── feed.rs, inbox.rs, inbox_cmd.rs, inbox_monitor.rs, join.rs,
│       ├── known_users.rs, name_resolver.rs, names.rs, nexus.rs,
│       ├── ordering.rs, post.rs, probe.rs, profile.rs, publisher.rs,
│       ├── reader.rs, session.rs, wire.rs
│       └── tui/{mod,commands,input,interactive,line,status,terminal}.rs
```

## Key Shared Helpers (cmd/mod.rs)

- `parse_topic(s)`: 64-char hex → raw 32-byte key; anything else → `discovery_key(s.as_bytes())` (BLAKE2b-256).
- `resolve_bootstrap(...)`: bootstrap-list resolution. CLI `--bootstrap` overrides the config file's `network.bootstrap` (it does not combine). After the base list is selected, `--public` adds the default public HyperDHT bootstrap nodes; an empty list auto-fills with the defaults; `--no-public` removes the defaults.
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
| `VERSION` (v2) | 0x02 | deaddrop/v2/wire.rs |
| `MAX_CHUNK_SIZE` (v2) | 1000 | deaddrop/v2/wire.rs |
| `DATA_HEADER_SIZE` (v2) | 2 | deaddrop/v2/wire.rs |
| `DATA_PAYLOAD_MAX` (v2) | 998 | deaddrop/v2/wire.rs |
| `NON_ROOT_INDEX_HEADER_SIZE` (v2) | 1 | deaddrop/v2/wire.rs |
| `NON_ROOT_INDEX_SLOT_CAP` (v2) | 31 | deaddrop/v2/wire.rs |
| `ROOT_INDEX_HEADER_SIZE` (v2) | 13 | deaddrop/v2/wire.rs |
| `ROOT_INDEX_SLOT_CAP` (v2) | 30 | deaddrop/v2/wire.rs |
| `NEED_LIST_HEADER_SIZE` (v2) | 3 | deaddrop/v2/wire.rs |
| `NEED_ENTRY_SIZE` (v2) | 8 | deaddrop/v2/wire.rs |
| `NEED_LIST_ENTRY_CAP` (v2) | 124 | deaddrop/v2/wire.rs |
| `HASH_LEN` (v2) | 32 | deaddrop/v2/wire.rs |
| `SOFT_DEPTH_CAP` (v2) | 4 | deaddrop/v2/mod.rs |
| `PARALLEL_FETCH_CAP` (v2) | 64 | deaddrop/v2/mod.rs |
| `PUT_TIMEOUT` (v2) | 30s | deaddrop/v2/publish.rs |
| `CHUNK_SIZE` | 65536 | cp.rs |
| `MAX_RECORD_SIZE` (chat) | 1000 | chat/wire.rs |
| `MAX_SCREEN_NAME_CONTENT` (chat) | 820 | chat/wire.rs |
| `FEED_EXPIRY_SECS` (chat) | 1200 | chat/feed.rs |
| `DEDUP_RING_CAPACITY` (chat) | 1000 | chat/ordering.rs |
| `GAP_TIMEOUT` (chat) | 5s | chat/ordering.rs |
| `HISTORY_CAP` (chat TUI) | 500 | chat/tui/interactive.rs |

## Known Issues

See `ISSUES.md` at the workspace root for tracked source-level issues discovered during documentation.

## Documentation

Full CLI documentation lives in `../docs/`. Build with `mdbook build docs/` from the workspace root.

## Testing

```bash
cargo test -p peeroxide-cli
```

Integration tests are in `tests/`. They require network access (bootstrap nodes) for DHT-dependent tests. The `live_commands.rs` suite is gated behind `#[ignore]` — run with `cargo test -p peeroxide-cli --test live_commands -- --ignored`.
