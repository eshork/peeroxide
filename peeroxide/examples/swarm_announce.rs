#![deny(clippy::all)]

//! Swarm announce — announces a topic on the public DHT, accepts connections,
//! and echoes back any received data prefixed with "echo: ".
//!
//! Usage:
//!   cargo run --example swarm_announce -p peeroxide [-- <hex-topic>]
//!
//! If no topic is given, a random one is generated and printed.
//! The remote side can connect with `swarm_join` or the Node.js counterpart.

use std::env;

use peeroxide::{spawn, JoinOpts, SwarmConfig, SwarmConnection};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let topic: [u8; 32] = match env::args().nth(1) {
        Some(hex) => {
            let bytes = hex::decode(&hex).expect("topic must be 64 hex chars");
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        None => {
            let t: [u8; 32] = rand::random();
            t
        }
    };

    tracing::info!(topic = %hex::encode(topic), "announcing topic");

    let config = SwarmConfig::with_public_bootstrap();
    let (_join, handle, mut conn_rx) = spawn(config).await.expect("swarm spawn failed");

    handle
        .join(
            topic,
            {
                let mut opts = JoinOpts::default();
                opts.client = false;
                opts
            },
        )
        .await
        .expect("join failed");

    handle.flush().await.expect("flush failed");
    tracing::info!(topic = %hex::encode(topic), "announced — waiting for connections");

    while let Some(conn) = conn_rx.recv().await {
        tracing::info!(
            remote = %hex::encode(conn.remote_public_key()),
            initiator = conn.is_initiator,
            "new connection"
        );
        tokio::spawn(handle_connection(conn));
    }
}

async fn handle_connection(mut conn: SwarmConnection) {
    loop {
        match conn.peer.stream.read().await {
            Ok(Some(data)) => {
                let msg = String::from_utf8_lossy(&data);
                tracing::info!(msg = %msg, "received");
                let reply = format!("echo: {msg}");
                if let Err(e) = conn.peer.stream.write(reply.as_bytes()).await {
                    tracing::error!(err = %e, "write failed");
                    break;
                }
            }
            Ok(None) => {
                tracing::info!("connection closed by remote");
                break;
            }
            Err(e) => {
                tracing::error!(err = %e, "read error");
                break;
            }
        }
    }
}
