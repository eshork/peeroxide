#![deny(clippy::all)]

//! Live network relay test — proves end-to-end encrypted communication
//! through a real blind-relay on the public HyperDHT network.
//!
//! Spawns a Node.js blind-relay server, then two Rust Hyperswarm instances:
//! a server (with relay_through) and a client. The client's connection is
//! routed through the relay. Both exchange data to verify the relay path.
//!
//!   cargo run --example relay_test
//!
//! Requires: `npm install` in tests/node/
//! Set RUST_LOG=debug for verbose output.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use peeroxide::{spawn, JoinOpts, KeyPair, SwarmConfig};
use peeroxide_dht::crypto::hash;

fn test_topic() -> [u8; 32] {
    hash(b"peeroxide-relay-test")
}

struct NodeRelay {
    child: Child,
    public_key: [u8; 32],
    addr: std::net::SocketAddr,
}

impl NodeRelay {
    fn start() -> Self {
        let node_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("tests/node");

        let mut child = Command::new("node")
            .arg("blind-relay-server.js")
            .current_dir(&node_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn Node.js relay server");

        let stdout = child.stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read relay output");

        let info: serde_json::Value = serde_json::from_str(line.trim()).expect("parse JSON");
        assert!(info["ready"].as_bool().unwrap_or(false), "relay not ready");

        let pk_hex = info["publicKey"].as_str().expect("publicKey");
        let pk_bytes = hex::decode(pk_hex).expect("decode pk");
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&pk_bytes);

        let port = info["port"].as_u64().expect("port") as u16;
        let host = info["host"].as_str().unwrap_or("0.0.0.0");
        let ip: std::net::IpAddr = if host == "0.0.0.0" {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            host.parse().expect("parse relay host")
        };
        let addr = std::net::SocketAddr::new(ip, port);

        println!("[relay] started — pk: {pk_hex}, addr: {addr}");
        Self { child, public_key, addr }
    }
}

impl Drop for NodeRelay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    println!("=== peeroxide live relay test ===\n");

    // 1. Start Node.js blind-relay server on public DHT.
    let relay = NodeRelay::start();

    // Give the relay a moment to fully bootstrap.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let topic = test_topic();

    // 2. Start Rust server with relay_through pointing to the Node.js relay.
    let server_kp = KeyPair::generate();
    let server_pk = server_kp.public_key;
    println!(
        "[server] pk: {}",
        hex::encode(&server_pk[..8])
    );

    let server_config = SwarmConfig {
        key_pair: Some(server_kp),
        relay_through: Some(relay.public_key),
        relay_address: Some(relay.addr),
        ..SwarmConfig::with_public_bootstrap()
    };

    let (server_join, server_handle, mut server_rx) =
        spawn(server_config).await.expect("server spawn");

    server_handle
        .join(
            topic,
            JoinOpts {
                server: true,
                client: false,
            },
        )
        .await
        .expect("server join");
    server_handle.flush().await.expect("server flush");
    println!("[server] announced on topic\n");

    // 3. Start Rust client.
    let client_kp = KeyPair::generate();
    println!(
        "[client] pk: {}",
        hex::encode(&client_kp.public_key[..8])
    );

    let client_config = SwarmConfig {
        key_pair: Some(client_kp),
        ..SwarmConfig::with_public_bootstrap()
    };

    let (client_join, client_handle, mut client_rx) =
        spawn(client_config).await.expect("client spawn");

    client_handle
        .join(
            topic,
            JoinOpts {
                server: false,
                client: true,
            },
        )
        .await
        .expect("client join");

    println!("[client] looking up topic...\n");

    // 4. Wait for connections on both sides with a timeout.
    let timeout = Duration::from_secs(60);

    let (mut server_conn, mut client_conn) = tokio::try_join!(
        async {
            tokio::time::timeout(timeout, server_rx.recv())
                .await
                .map_err(|_| "server: timed out waiting for connection")?
                .ok_or("server: channel closed")
        },
        async {
            tokio::time::timeout(timeout, client_rx.recv())
                .await
                .map_err(|_| "client: timed out waiting for connection")?
                .ok_or("client: channel closed")
        },
    )
    .expect("connection failed");

    println!(
        "[server] got connection from {}",
        hex::encode(&server_conn.peer.remote_public_key[..8])
    );
    println!(
        "[client] got connection to {}",
        hex::encode(&client_conn.peer.remote_public_key[..8])
    );

    // 5. Exchange data through the relay.
    let client_msg = b"hello through relay from client";
    client_conn
        .peer
        .stream
        .write(client_msg)
        .await
        .expect("client write");
    println!("[client] sent: {}", String::from_utf8_lossy(client_msg));

    let server_recv = tokio::time::timeout(Duration::from_secs(10), server_conn.peer.stream.read())
        .await
        .expect("server read timeout")
        .expect("server read error")
        .expect("server got EOF");
    println!(
        "[server] recv: {}",
        String::from_utf8_lossy(&server_recv)
    );
    assert_eq!(server_recv, client_msg);

    let server_msg = b"hello through relay from server";
    server_conn
        .peer
        .stream
        .write(server_msg)
        .await
        .expect("server write");
    println!("[server] sent: {}", String::from_utf8_lossy(server_msg));

    let client_recv = tokio::time::timeout(Duration::from_secs(10), client_conn.peer.stream.read())
        .await
        .expect("client read timeout")
        .expect("client read error")
        .expect("client got EOF");
    println!(
        "[client] recv: {}",
        String::from_utf8_lossy(&client_recv)
    );
    assert_eq!(client_recv, server_msg);

    println!("\n=== PASS: bidirectional data exchange through relay ===");

    // 6. Cleanup.
    server_handle.destroy().await.ok();
    client_handle.destroy().await.ok();
    drop(relay);
    let _ = server_join.await;
    let _ = client_join.await;
}
