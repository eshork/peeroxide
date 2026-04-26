#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Rust port of [HyperDHT](https://github.com/holepunchto/hyperdht) — a
//! Kademlia distributed hash table with NAT hole-punching, Noise-encrypted
//! connections, and blind-relay fallback.
//!
//! This crate implements the full HyperDHT protocol stack, wire-compatible
//! with the Node.js implementation on the public Hyperswarm network.
//!
//! # Protocol layers
//!
//! From bottom to top:
//!
//! | Layer | Module | Reference |
//! |---|---|---|
//! | Wire encoding | [`compact_encoding`] | [compact-encoding](https://github.com/holepunchto/compact-encoding) |
//! | DHT RPC | [`rpc`], [`io`], [`query`], [`routing_table`] | [dht-rpc](https://github.com/mafintosh/dht-rpc) |
//! | Peer operations | [`hyperdht`], [`hyperdht_messages`] | [hyperdht](https://github.com/holepunchto/hyperdht) |
//! | Noise XX handshake | [`noise`], [`noise_wrap`] | [noise-handshake](https://github.com/holepunchto/noise-handshake) |
//! | Encrypted streams | [`secret_stream`], [`secretstream`] | [@hyperswarm/secret-stream](https://github.com/holepunchto/hyperswarm-secret-stream) |
//! | NAT traversal | [`nat`], [`holepuncher`] | hyperdht/lib/holepuncher.js |
//! | Relay | [`blind_relay`], [`protomux`] | [blind-relay](https://github.com/holepunchto/blind-relay) |
//!
//! # Typical usage
//!
//! Most users should depend on the higher-level [`peeroxide`](https://docs.rs/peeroxide)
//! crate, which wraps this DHT layer with topic-based peer discovery and
//! connection management. Use `peeroxide-dht` directly when you need
//! low-level DHT operations (custom commands, mutable/immutable storage,
//! manual hole-punching).
//!
//! ```rust,no_run
//! use libudx::UdxRuntime;
//! use peeroxide_dht::hyperdht::{self, HyperDhtConfig, KeyPair};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = UdxRuntime::new()?;
//! let config = HyperDhtConfig::with_public_bootstrap();
//! let (_task, dht, _server_rx) = hyperdht::spawn(&runtime, config).await?;
//!
//! dht.bootstrapped().await?;
//!
//! let key_pair = KeyPair::generate();
//! let topic = peeroxide_dht::crypto::hash(b"my-app");
//! dht.announce(topic, &key_pair, &[]).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Interoperability
//!
//! Every protocol layer is validated against the Node.js reference via
//! golden byte fixtures and live cross-language interop tests. The crate
//! connects to the public HyperDHT bootstrap nodes and participates in
//! the same network as Node.js peers.

#![deny(clippy::all)]

/// Blind relay for proxying encrypted traffic between peers behind restrictive NATs.
pub mod blind_relay;
/// Compact binary encoding primitives compatible with the
/// [compact-encoding](https://github.com/holepunchto/compact-encoding) wire format.
pub mod compact_encoding;
/// BLAKE2b hashing, Ed25519 signing, and namespace derivation helpers.
pub mod crypto;
/// High-level HyperDHT node: peer discovery, announce/unannounce, mutable/immutable
/// storage, and Noise-encrypted connections.
pub mod hyperdht;
/// Wire-format message types for HyperDHT peer handshake, holepunch, and relay
/// operations.
pub mod hyperdht_messages;
/// DHT RPC request/response message encoding and decoding.
pub mod messages;
/// Noise IK handshake for establishing shared secrets between peers.
pub mod noise;
/// Noise handshake wrapper that adds framing and key splitting for stream encryption.
pub mod noise_wrap;
/// Lightweight multiplexer for running multiple channels over a single connection.
pub mod protomux;
/// DHT RPC transport layer: request dispatch, reply handling, and node communication.
pub mod rpc;
/// Noise-encrypted bidirectional byte stream over any `AsyncRead + AsyncWrite` transport.
pub mod secret_stream;

// Internal protocol modules — public for advanced use but hidden from
// top-level docs. Access via `peeroxide_dht::<module>` if needed.
#[doc(hidden)]
pub mod holepuncher;
#[doc(hidden)]
pub mod io;
#[doc(hidden)]
pub mod nat;
#[doc(hidden)]
pub mod peer;
#[doc(hidden)]
pub mod persistent;
#[doc(hidden)]
pub mod query;
#[doc(hidden)]
pub mod router;
#[doc(hidden)]
pub mod routing_table;
#[doc(hidden)]
pub mod secretstream;
#[doc(hidden)]
pub mod secure_payload;
#[doc(hidden)]
pub mod socket_pool;
