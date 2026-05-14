# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.3.0](https://github.com/Rightbracket/peeroxide/compare/peeroxide-dht-v1.2.0...peeroxide-dht-v1.3.0) - 2026-05-13

### Added

- `WireCounters` struct — provides atomic, shareable counters for tracking total bytes sent and received. Includes `new()` for initialization and `snapshot()` for retrieving current totals.
- `Io::wire` field — public access to the IO layer's `WireCounters`.
- `Io::wire_counters()` — returns a handle to the IO layer's wire byte counters.
- `DhtHandle::wire_stats()` — returns a snapshot of cumulative wire bytes `(sent, received)` for the DHT node.
- `DhtHandle::wire_counters()` — returns a handle to the node-wide `WireCounters`.
- `HyperDhtHandle::wire_stats()` — returns a snapshot of total wire bytes processed by the DHT.
- `HyperDhtHandle::wire_counters()` — returns a handle to the shared wire byte counters for the running instance.

## [1.2.0](https://github.com/Rightbracket/peeroxide/compare/peeroxide-dht-v1.1.0...peeroxide-dht-v1.2.0) - 2026-04-30

### Added

- `DhtHandle::table_id()` — returns the node's current routing table ID; useful for server nodes that need to derive their own `NodeId` after bootstrapping.
- `DhtHandle::server_socket()` — returns a shared `Arc<UdxSocket>` for the primary socket, enabling UDX stream multiplexing by callers.
- `DhtHandle::listen_socket()` — returns a shared `Arc<UdxSocket>` for the socket bound to the advertised port, used for inbound UDX stream connections.
- `PersistentStats` struct with `records`, `record_topics`, `mutables`, `immutables`, and `router_entries` fields; returned by the new `stats()` method on node server handles.
- `RoutingTable::rebuild_with_id()` — rebuilds the routing table under a new node ID, mirroring the Node.js `_updateNetworkState` rebuild in `dht-rpc`.
- `SecretStream::shutdown()` — gracefully closes the write half of a secret stream, sending a FIN to the remote peer.
- `PingResponse` now includes `to` (reflexive address as seen by the remote) and `closer_nodes` (closer nodes returned by the remote's routing table).

### Changed

- Router forward-entry TTL corrected from 30 seconds to 20 minutes, matching the Node.js HyperDHT reference implementation. Entries for running servers (`has_server = true`) no longer expire via TTL or GC — they persist until the server is explicitly unregistered.
- Bootstrap ping now uses `CMD_FIND_NODE` with the local node's table ID as the target, instead of `CMD_PING` with no target. This causes bootstrap nodes to return closer nodes, accelerating routing table population.
- Non-ephemeral nodes with a known public address now derive a deterministic node ID from `hash(host, port)` at spawn time, matching Node.js DHT identity behaviour. Nodes bound to a wildcard address instead collect reflexive address samples during bootstrapping and update their ID once consensus is reached.
- Announce handler now populates a `ForwardEntry` in the router for newly-seen peers (when no server entry already exists), so inbound `PEER_HANDSHAKE` requests can be relayed to recently-announced peers even before they connect.

## [1.1.0](https://github.com/Rightbracket/peeroxide/compare/peeroxide-dht-v1.0.1...peeroxide-dht-v1.1.0) - 2026-04-28

### Other

- Add #[non_exhaustive] to public structs and enums ([#10](https://github.com/Rightbracket/peeroxide/pull/10))

## [1.0.1](https://github.com/Rightbracket/peeroxide/compare/peeroxide-dht-v1.0.0...peeroxide-dht-v1.0.1) - 2026-04-26

### Other

- Add doc comments to all public API items and enforce deny(missing_docs) ([#2](https://github.com/Rightbracket/peeroxide/pull/2))

## [1.0.0] - 2025-04-25

Initial release. Pure Rust implementation of HyperDHT, wire-compatible with
the existing Node.js network.

### Added

- Full HyperDHT implementation (Kademlia DHT, hole-punching, blind relay)
- Noise XX and Noise IK handshake patterns (Ed25519 DH, ChaChaPoly)
- SecretStream transport (pure-Rust libsodium `crypto_secretstream_xchacha20poly1305`)
- Protomux channel multiplexer (actor model, ~1460 lines)
- Blind relay client for NAT traversal
- Compact encoding (all types from the Node.js `compact-encoding` package)
- Server-side record storage with LRU+TTL eviction
- `HyperDhtHandle` client API: `lookup`, `announce`, `find_peer`, `unannounce`, `immutable_put/get`, `mutable_put/get`, `connect`, `register_server`
- NAT classification (OPEN/CONSISTENT/RANDOM/UNKNOWN)
- Socket pool with multi-socket management
- Holepunch strategy selection (direct, birthday paradox)
- Async DNS resolution for bootstrap nodes
- `HyperDhtConfig::with_public_bootstrap()` for live network use

### Tested

- 314 unit tests passing
- 66 integration tests passing (protocol handshakes, DHT queries, relay, holepunch)
- 6 live network tests (ignored by default — require public HyperDHT bootstrap connectivity)
- Golden byte fixtures verified against Node.js HyperDHT, dht-rpc, and hyperswarm-secret-stream reference implementations
- Live cross-language interop tests (Rust ↔ Node.js) at every protocol layer

### Dependencies

- `libudx` — reliable UDP transport
- `tokio` — async runtime
- `tracing`, `thiserror` — logging and error handling
- Pure Rust crypto stack (RustCrypto): `blake2`, `ed25519-dalek`, `curve25519-dalek`, `sha2`, `chacha20poly1305`, `chacha20`, `poly1305`, `xsalsa20poly1305`
- `rand` — key generation

### Compatibility

- Wire-compatible with Node.js HyperDHT and dht-rpc
- Rust edition 2024, MSRV 1.85
- Dual-licensed: MIT OR Apache-2.0

[1.0.0]: https://github.com/Rightbracket/peeroxide/releases/tag/v1.0.0
