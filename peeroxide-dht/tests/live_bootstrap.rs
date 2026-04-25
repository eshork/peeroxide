//! M9.2: Live bootstrap smoke test against the public HyperDHT network.
//!
//! Run with: `cargo test -p peeroxide-dht --test live_bootstrap -- --ignored`

#![deny(clippy::all)]

use std::time::Duration;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht::{self, HyperDhtConfig};

#[tokio::test]
#[ignore = "requires internet — connects to public HyperDHT bootstrap nodes"]
async fn bootstrap_against_public_dht() {
    let _ = tracing_subscriber::fmt::try_init();

    let result = tokio::time::timeout(Duration::from_secs(30), run_bootstrap()).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("live bootstrap test failed: {e}"),
        Err(_) => panic!("live bootstrap test timed out after 30s"),
    }
}

async fn run_bootstrap() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = UdxRuntime::new()?;
    let config = HyperDhtConfig::with_public_bootstrap();

    let (task, handle, _server_rx) = hyperdht::spawn(&runtime, config).await?;

    tracing::info!("waiting for bootstrap against public DHT...");
    handle.bootstrapped().await?;
    tracing::info!("bootstrap complete");

    let table_sz = handle.dht().table_size().await?;
    tracing::info!(table_sz, "routing table size after bootstrap");
    assert!(
        table_sz >= 10,
        "expected ≥10 nodes in routing table after public bootstrap, got {table_sz}"
    );

    let port = handle.dht().local_port().await?;
    tracing::info!(port, "local UDP port");
    assert!(port > 0, "expected non-zero local port");

    handle.destroy().await?;
    let _ = task.await;
    drop(runtime);

    Ok(())
}
