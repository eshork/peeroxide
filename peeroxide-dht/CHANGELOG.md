# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
