#![deny(clippy::all)]

//! Passive relay soak tool — joins the public HyperDHT network as a
//! non-ephemeral, non-firewalled node and logs all incoming traffic.
//!
//! Run: `RUST_LOG=info cargo run --example relay_soak -p peeroxide-dht`
//!
//! The node advertises its NodeId in outgoing messages (`ephemeral: false`)
//! and uses the server socket (`firewalled: false`), so other DHT peers
//! will add it to their routing tables and may relay handshakes or
//! holepunch messages through it.
//!
//! Every 30 seconds it prints routing-table size.  Relay events appear
//! as `info`-level tracing from the request handler (look for
//! "handshake RELAY" / "holepunch RELAY").
//!
//! Press Ctrl-C to shut down gracefully.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use peeroxide_dht::hyperdht::{self, HyperDhtConfig, ServerEvent, DEFAULT_BOOTSTRAP};
use peeroxide_dht::rpc::DhtConfig;

use libudx::UdxRuntime;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = HyperDhtConfig {
        dht: DhtConfig {
            bootstrap: DEFAULT_BOOTSTRAP
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            ephemeral: Some(false),
            firewalled: false,
            ..DhtConfig::default()
        },
        ..HyperDhtConfig::default()
    };

    let runtime = UdxRuntime::new().expect("failed to create UDX runtime");
    let (join_handle, handle, mut server_rx) =
        hyperdht::spawn(&runtime, config).await.expect("spawn failed");

    tracing::info!("waiting for bootstrap...");
    handle.bootstrapped().await.expect("bootstrap failed");

    let port = handle.local_port().await.unwrap_or(0);
    let table = handle.table_size().await.unwrap_or(0);
    tracing::info!(port, table, "bootstrapped — listening on public DHT");

    let handshake_count = Arc::new(AtomicU64::new(0));
    let holepunch_count = Arc::new(AtomicU64::new(0));

    let hs = Arc::clone(&handshake_count);
    let hp = Arc::clone(&holepunch_count);

    let event_task = tokio::spawn(async move {
        while let Some(event) = server_rx.recv().await {
            match event {
                ServerEvent::PeerHandshake { reply_tx, from, .. } => {
                    hs.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(
                        from = %format!("{}:{}", from.host, from.port),
                        "server: peer handshake (handled locally)"
                    );
                    let _ = reply_tx.send(None);
                }
                ServerEvent::PeerHolepunch {
                    reply_tx, from, ..
                } => {
                    hp.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(
                        from = %format!("{}:{}", from.host, from.port),
                        "server: peer holepunch (handled locally)"
                    );
                    let _ = reply_tx.send(None);
                }
            }
        }
    });

    let stats_handle = handle.clone();
    let hs2 = Arc::clone(&handshake_count);
    let hp2 = Arc::clone(&holepunch_count);
    let stats_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let table = stats_handle.table_size().await.unwrap_or(0);
            let hs = hs2.load(Ordering::Relaxed);
            let hp = hp2.load(Ordering::Relaxed);
            tracing::info!(
                table,
                local_handshakes = hs,
                local_holepunches = hp,
                "periodic stats"
            );
        }
    });

    tracing::info!("soak running — Ctrl-C to stop");
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutting down...");

    handle.destroy().await.ok();
    event_task.abort();
    stats_task.abort();
    join_handle.abort();
}
