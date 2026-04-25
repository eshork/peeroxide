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

pub mod blind_relay;
pub mod compact_encoding;
pub mod crypto;
pub mod holepuncher;
pub mod hyperdht;
pub mod hyperdht_messages;
pub mod io;
pub mod messages;
pub mod nat;
pub mod noise;
pub mod noise_wrap;
pub mod peer;
pub mod persistent;
pub mod protomux;
pub mod query;
pub mod routing_table;
pub mod router;
pub mod rpc;
pub mod secret_stream;
pub mod secretstream;
pub mod secure_payload;
pub mod socket_pool;
