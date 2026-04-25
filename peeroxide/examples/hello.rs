#![deny(clippy::all)]

//! Minimal peeroxide example — join a topic and print peer connections.
//!
//! Usage:
//!   cargo run --example hello -p peeroxide

use peeroxide::{discovery_key, spawn, JoinOpts, SwarmConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let config = SwarmConfig::with_public_bootstrap();
    let (_task, handle, mut conn_rx) = spawn(config).await?;

    let topic = discovery_key(b"hello-peeroxide");
    handle.join(topic, JoinOpts::default()).await?;
    handle.flush().await?;

    tracing::info!(topic = %hex::encode(topic), "joined — waiting for peers");

    while let Some(conn) = conn_rx.recv().await {
        tracing::info!(
            remote = %hex::encode(conn.remote_public_key()),
            "connected"
        );
    }
    Ok(())
}
