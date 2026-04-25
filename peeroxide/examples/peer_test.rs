#![deny(clippy::all)]

//! peeroxide network test — run on any number of machines, they all find
//! each other on the public DHT and exchange messages.
//!
//!   ./peer_test
//!
//! Set RUST_LOG=info for verbose DHT output.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use peeroxide::{spawn, JoinOpts, KeyPair, SwarmConfig, SwarmConnection};
use peeroxide_dht::crypto::hash;

fn test_topic() -> [u8; 32] {
    hash(b"peeroxide-test")
}

static PEER_COUNT: AtomicU32 = AtomicU32::new(0);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let topic = test_topic();
    let key_pair = KeyPair::generate();
    let my_id = short_id(&key_pair.public_key);

    println!("peeroxide network test");
    println!("node:  {my_id}");
    println!("topic: {}", hex::encode(topic));
    println!();
    println!("[*] bootstrapping...");

    let config = SwarmConfig {
        key_pair: Some(key_pair),
        ..SwarmConfig::with_public_bootstrap()
    };
    let (_join, handle, mut conn_rx) = spawn(config).await.expect("swarm spawn failed");

    handle
        .join(topic, JoinOpts { server: true, client: true })
        .await
        .expect("join failed");
    handle.flush().await.expect("flush failed");

    println!("[*] joined — waiting for peers (Ctrl-C to stop)");
    println!();

    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_signal = Arc::clone(&shutdown);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_signal.notify_waiters();
    });

    loop {
        tokio::select! {
            conn = conn_rx.recv() => {
                let Some(conn) = conn else { break };
                let n = PEER_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                let remote = short_id(conn.remote_public_key());
                let dir = if conn.is_initiator { "→ outgoing" } else { "← incoming" };
                println!("[+] peer #{n} ({dir}): {remote}");
                let id = my_id.clone();
                tokio::spawn(handle_peer(conn, id));
            }
            () = shutdown.notified() => {
                break;
            }
        }
    }

    let total = PEER_COUNT.load(Ordering::Relaxed);
    println!();
    println!("[*] shutting down ({total} peers seen)");
    handle.destroy().await.ok();
}

async fn handle_peer(mut conn: SwarmConnection, my_id: String) {
    let remote = short_id(conn.remote_public_key());

    let greeting = format!("hi from {my_id}");
    if let Err(e) = conn.peer.stream.write(greeting.as_bytes()).await {
        println!("    [{remote}] send failed: {e}");
        return;
    }
    println!("    [{remote}] sent: {greeting}");

    loop {
        match conn.peer.stream.read().await {
            Ok(Some(data)) => {
                let msg = String::from_utf8_lossy(&data);
                println!("    [{remote}] recv: {msg}");

                let reply = format!("ack from {my_id}");
                if let Err(e) = conn.peer.stream.write(reply.as_bytes()).await {
                    println!("    [{remote}] send failed: {e}");
                    break;
                }
            }
            Ok(None) => {
                println!("    [{remote}] disconnected");
                break;
            }
            Err(e) => {
                println!("    [{remote}] error: {e}");
                break;
            }
        }
    }
}

fn short_id(key: &[u8; 32]) -> String {
    hex::encode(&key[..4])
}
