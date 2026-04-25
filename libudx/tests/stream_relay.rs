#![deny(clippy::all)]

use std::net::SocketAddr;

use libudx::UdxRuntime;

const LOCALHOST: &str = "127.0.0.1:0";

fn addr() -> SocketAddr {
    LOCALHOST.parse().unwrap()
}

/// Topology: peer_a ↔ relay_1 -- relay_to -- relay_2 ↔ peer_b
///
/// peer_a writes "hello" → relay_1 receives → relays to relay_2 → peer_b reads "hello"
/// peer_b writes "world" → relay_2 receives → relays to relay_1 → peer_a reads "world"
#[tokio::test]
async fn udx_relay_bidirectional() {
    let _ = tracing_subscriber::fmt::try_init();

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), run_relay_test()).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("relay test failed: {e}"),
        Err(_) => panic!("relay test timed out"),
    }
}

async fn run_relay_test() -> Result<(), Box<dyn std::error::Error>> {
    let rt = UdxRuntime::new()?;

    let relay_sock_1 = rt.create_socket().await?;
    relay_sock_1.bind(addr()).await?;
    let relay_addr_1 = relay_sock_1.local_addr().await?;

    let relay_sock_2 = rt.create_socket().await?;
    relay_sock_2.bind(addr()).await?;
    let relay_addr_2 = relay_sock_2.local_addr().await?;

    let relay_stream_1 = rt.create_stream(10).await?;
    let relay_stream_2 = rt.create_stream(20).await?;

    let peer_a_sock = rt.create_socket().await?;
    peer_a_sock.bind(addr()).await?;
    let peer_a_addr = peer_a_sock.local_addr().await?;
    let mut peer_a_stream = rt.create_stream(1).await?;

    let peer_b_sock = rt.create_socket().await?;
    peer_b_sock.bind(addr()).await?;
    let peer_b_addr = peer_b_sock.local_addr().await?;
    let mut peer_b_stream = rt.create_stream(2).await?;

    relay_stream_1.relay_to(&relay_stream_2)?;
    relay_stream_2.relay_to(&relay_stream_1)?;

    relay_stream_1.connect(&relay_sock_1, 1, peer_a_addr).await?;
    relay_stream_2.connect(&relay_sock_2, 2, peer_b_addr).await?;

    peer_a_stream.connect(&peer_a_sock, 10, relay_addr_1).await?;
    peer_b_stream.connect(&peer_b_sock, 20, relay_addr_2).await?;

    peer_a_stream.write(b"hello from A").await?;
    let data = peer_b_stream
        .read()
        .await?
        .expect("peer_b should receive data");
    assert_eq!(data, b"hello from A");

    peer_b_stream.write(b"hello from B").await?;
    let data = peer_a_stream
        .read()
        .await?
        .expect("peer_a should receive data");
    assert_eq!(data, b"hello from B");

    Ok(())
}
