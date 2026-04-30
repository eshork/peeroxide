# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0](https://github.com/Rightbracket/peeroxide/compare/peeroxide-v1.1.0...peeroxide-v1.2.0) - 2026-04-30

### Added

- `SwarmHandle::dht()` — exposes the underlying `HyperDhtHandle` for low-level DHT operations such as mutable/immutable record storage and manual peer lookup.
- `SwarmHandle::key_pair()` — exposes the Ed25519 key pair identifying this swarm node, for use with DHT mutable records or other identity operations.
- Re-exported `HyperDhtHandle`, `MutablePutResult`, `MutableGetResult`, and `ImmutablePutResult` from the `peeroxide` crate root, so callers no longer need to depend on `peeroxide-dht` directly for common DHT storage types.

### Changed

- Swarm nodes now self-announce under `hash(publicKey)` during each discovery refresh. This populates `ForwardEntry` records on the nodes closest to the peer's public key, enabling `PEER_HANDSHAKE` routing to work correctly — matching the behaviour of the Node.js reference implementation.
- Server registrations are now cleaned up properly on `leave()` (when the last server topic is left), on `destroy()`, and when the swarm handle is dropped. Previously, `ForwardEntry` records for the local server could persist in the router until TTL expiry.
- Incoming server connections now reuse the DHT's bound listen socket for the UDX stream, rather than creating a new socket. This ensures streams arrive on the same port that remote peers have on record, fixing connection establishment in NAT environments.
- Handshake replies no longer echo the client's address back in `peer_address`; the field is now correctly set to `None` in the reply, matching the wire protocol.

## [1.1.0](https://github.com/Rightbracket/peeroxide/compare/peeroxide-v1.0.1...peeroxide-v1.1.0) - 2026-04-28

### Other

- Add #[non_exhaustive] to public structs and enums ([#10](https://github.com/Rightbracket/peeroxide/pull/10))

## [1.0.1](https://github.com/Rightbracket/peeroxide/compare/peeroxide-v1.0.0...peeroxide-v1.0.1) - 2026-04-26

### Other

- Add doc comments to all public API items and enforce deny(missing_docs) ([#2](https://github.com/Rightbracket/peeroxide/pull/2))
