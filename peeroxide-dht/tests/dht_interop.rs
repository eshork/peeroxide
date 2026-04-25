//! Live interop test: Rust DhtNode ↔ Node.js dht-rpc bootstrapper.
//!
//! Verifies: bootstrap completes, ping returns valid response.

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::Duration;

use libudx::UdxRuntime;
use peeroxide_dht::peer::peer_id;
use peeroxide_dht::rpc::DhtConfig;

fn node_server_path() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    format!("{manifest}/../tests/node/dht-rpc-server.js")
}

#[tokio::test]
async fn rust_node_bootstraps_and_pings_js() {
    let _ = tracing_subscriber::fmt::try_init();

    let result = tokio::time::timeout(Duration::from_secs(30), run_interop()).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("interop test failed: {e}"),
        Err(_) => panic!("interop test timed out after 30s"),
    }
}

async fn run_interop() -> Result<(), Box<dyn std::error::Error>> {
    // ── Spawn JS bootstrapper ────────────────────────────────────────────
    let mut child = Command::new("node")
        .arg(node_server_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let stdout = child.stdout.take().expect("no stdout");
    let stdin = child.stdin.take().expect("no stdin");

    let mut reader = std::io::BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let info: serde_json::Value = serde_json::from_str(line.trim())?;
    let js_port = info["port"].as_u64().expect("missing port") as u16;
    assert!(info["ready"].as_bool().unwrap_or(false), "JS node not ready");
    tracing::info!(js_port, "JS bootstrapper ready");

    // ── Create Rust DHT node that bootstraps from the JS node ────────────
    let runtime = UdxRuntime::new()?;

    let config = DhtConfig {
        bootstrap: vec![format!("127.0.0.1:{js_port}")],
        port: 0,
        host: "127.0.0.1".to_string(),
        firewalled: true,
        ..DhtConfig::default()
    };

    let (task, handle) = peeroxide_dht::rpc::spawn(&runtime, config).await?;

    // ── Bootstrap ────────────────────────────────────────────────────────
    tracing::info!("waiting for bootstrap to complete...");
    handle.bootstrapped().await?;
    tracing::info!("bootstrap complete");

    let table_sz = handle.table_size().await?;
    tracing::info!(table_sz, "routing table size after bootstrap");
    assert!(table_sz >= 1, "expected at least 1 node in table, got {table_sz}");

    // ── Ping the JS bootstrapper ─────────────────────────────────────────
    tracing::info!("sending ping to JS bootstrapper...");
    let ping_resp = handle.ping("127.0.0.1", js_port).await?;

    tracing::info!(?ping_resp, "ping response received");
    assert_eq!(ping_resp.from.host, "127.0.0.1");
    assert_eq!(ping_resp.from.port, js_port);
    assert!(ping_resp.rtt < Duration::from_secs(5), "RTT too high: {:?}", ping_resp.rtt);

    let expected_id = peer_id("127.0.0.1", js_port);
    assert_eq!(
        ping_resp.id,
        Some(expected_id),
        "JS bootstrapper should return its computed peer_id"
    );

    // ── Cleanup ──────────────────────────────────────────────────────────
    handle.destroy().await?;
    let _ = task.await;

    drop(stdin);
    drop(reader);
    let _ = child.kill();
    let _ = child.wait();

    drop(runtime);
    Ok(())
}
