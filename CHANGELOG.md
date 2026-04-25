# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2025-04-25

Initial release. Pure Rust implementation of the Hyperswarm P2P networking
stack, wire-compatible with the existing Node.js network.

### Added

#### peeroxide

- Topic-based peer discovery and connection management (Hyperswarm port)
- `SwarmConfig` with configurable max peers, firewall callback, backoff, and jitter
- `spawn()` returns `(JoinHandle, SwarmHandle, Receiver<SwarmConnection>)`
- `SwarmHandle`: `join()`, `leave()`, `status()`, `destroy()`
- Connection deduplication by remote public key
- Peer state machine with priority scoring and exponential retry backoff
- `SwarmConfig::with_public_bootstrap()` for connecting to the live HyperDHT network
- `discovery_key()` helper for topic hashing

#### peeroxide-dht

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

#### libudx

- Pure Rust UDX protocol implementation (replaced C FFI bindings)
- BBR congestion control (faithful port of C `udx_bbr.c`)
- Reliability: cumulative ACK, SACK, retransmission with RTO, fast retransmit
- RTT estimation (Jacobson/Karels per RFC 6298)
- Rate sampling for BBR bandwidth estimation
- Token bucket pacing
- MTU probing (base 1200, max 1500, step 32)
- Relay packet forwarding (header rewriting, DESTROY propagation)
- `UdxAsyncStream`: `AsyncRead + AsyncWrite + Unpin` adapter for tokio
- Multiplexing: multiple streams per socket with independent congestion state
- Heartbeat keepalive (1s interval)
- Graceful shutdown with buffered write drain

### Tested

- 497 tests passing (0 failed, 6 ignored)
- Golden byte fixtures verified against Node.js reference implementations
- Live cross-language interop tests (Rust <-> Node.js) at every protocol layer
- Network simulation tests with configurable loss, delay, jitter, reorder, and MTU clamping
- Public HyperDHT bootstrap + announce/lookup smoke tests (ignored by default)

### Dependencies

- Pure Rust crypto stack (RustCrypto: blake2, ed25519-dalek, curve25519-dalek,
  sha2, chacha20poly1305, xsalsa20poly1305, hmac, poly1305)
- tokio async runtime
- No C dependencies

### Compatibility

- Wire-compatible with Node.js Hyperswarm, HyperDHT, and libudx
- Rust edition 2024, MSRV 1.85
- Dual-licensed: MIT OR Apache-2.0

[1.0.0]: https://github.com/Rightbracket/peeroxide/releases/tag/v1.0.0
