# peeroxide

[![crates.io](https://img.shields.io/crates/v/peeroxide.svg)](https://crates.io/crates/peeroxide)
[![docs.rs](https://docs.rs/peeroxide/badge.svg)](https://docs.rs/peeroxide)
[![CI](https://github.com/Rightbracket/peeroxide/actions/workflows/ci.yml/badge.svg)](https://github.com/Rightbracket/peeroxide/actions/workflows/ci.yml)

Rust port of Hyperswarm — topic-based P2P peer discovery and encrypted connections.

Peeroxide provides a high-level entry point for P2P networking. It discovers peers by topic on the public HyperDHT network, establishes Noise-encrypted connections, and manages connection lifecycles. It is wire-compatible with the Node.js Hyperswarm network.

## Quick Start

```rust
use peeroxide::{spawn, discovery_key, JoinOpts, SwarmConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SwarmConfig::with_public_bootstrap();
    let (_task, handle, mut conn_rx) = spawn(config).await?;

    let topic = discovery_key(b"my-application");
    handle.join(topic, JoinOpts::default()).await?;

    while let Some(conn) = conn_rx.recv().await {
        println!("connected to {}", hex::encode(conn.remote_public_key()));
        // conn.peer.stream provides encrypted read/write
    }
    Ok(())
}
```

## How It Works

1. Join a topic: Announces presence and looks up peers on a 32-byte topic hash.
2. Peer discovery: DHT returns peers on the same topic and swarm connects automatically with retry backoff.
3. Encrypted connection: Noise IK handshake and ChaCha20-Poly1305 SecretStream (AEAD) secure the link.
4. Receive connections: SwarmConnection arrives on a channel with the remote public key and encrypted stream.

## Key Types

- SwarmConfig: Configuration for the swarm and DHT.
- SwarmHandle: Control interface to join or leave topics.
- SwarmConnection: Handle to an established, encrypted peer connection.
- JoinOpts: Options for topic announcement and lookup.
- discovery_key: Helper to hash a human-readable string into a 32-byte topic.

## Crate Stack

```
peeroxide          <- this crate
└── peeroxide-dht  <- Kademlia DHT, Noise, hole-punching, relay
    └── libudx     <- reliable UDP with BBR congestion control
```

Peeroxide is part of the [peeroxide workspace](https://github.com/Rightbracket/peeroxide) and joins the same network as Node.js Hyperswarm peers.

## License

MIT OR Apache-2.0
