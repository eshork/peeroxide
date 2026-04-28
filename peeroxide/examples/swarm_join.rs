#![deny(clippy::all)]

//! Swarm join — joins a topic on the public DHT, connects to a peer,
//! sends a message, and prints the echo reply.
//!
//! Usage:
//!   cargo run --example swarm_join -p peeroxide -- <hex-topic> [message]
//!
//! The announce side should already be running (Rust `swarm_announce` or
//! the Node.js `hyperswarm-announce.js` counterpart).

use std::env;

use peeroxide::{spawn, JoinOpts, SwarmConfig};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let topic_hex = env::args()
        .nth(1)
        .expect("usage: swarm_join <hex-topic> [message]");
    let message = env::args().nth(2).unwrap_or_else(|| "hello from peeroxide".to_string());

    let topic_bytes = hex::decode(&topic_hex).expect("topic must be 64 hex chars");
    let mut topic = [0u8; 32];
    topic.copy_from_slice(&topic_bytes);

    tracing::info!(topic = %topic_hex, "joining topic");

    let config = SwarmConfig::with_public_bootstrap();
    let (_join, handle, mut conn_rx) = spawn(config).await.expect("swarm spawn failed");

    handle
        .join(
            topic,
            {
                let mut opts = JoinOpts::default();
                opts.server = false;
                opts
            },
        )
        .await
        .expect("join failed");

    handle.flush().await.expect("flush failed");
    tracing::info!("flushed — waiting for connection");

    let Some(mut conn) = conn_rx.recv().await else {
        tracing::error!("no connection received");
        return;
    };

    tracing::info!(
        remote = %hex::encode(conn.remote_public_key()),
        initiator = conn.is_initiator,
        "connected"
    );

    tracing::info!(msg = %message, "sending");
    conn.peer
        .stream
        .write(message.as_bytes())
        .await
        .expect("write failed");

    match conn.peer.stream.read().await {
        Ok(Some(data)) => {
            let reply = String::from_utf8_lossy(&data);
            tracing::info!(reply = %reply, "received echo");
        }
        Ok(None) => {
            tracing::info!("connection closed before reply");
        }
        Err(e) => {
            tracing::error!(err = %e, "read error");
        }
    }

    handle.destroy().await.ok();
}
