#![allow(dead_code)]

pub mod proxy;

use std::net::SocketAddr;
use std::time::Duration;

use libudx::{UdxRuntime, UdxSocket, UdxStream};
use sha2::{Digest, Sha256};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

const LOCALHOST: &str = "127.0.0.1:0";

pub fn create_runtime() -> UdxRuntime {
    UdxRuntime::new().expect("failed to create UdxRuntime")
}

pub async fn create_bound_socket(runtime: &UdxRuntime) -> (UdxSocket, SocketAddr) {
    let socket = runtime.create_socket().await.expect("failed to create socket");
    socket
        .bind(LOCALHOST.parse::<SocketAddr>().expect("parse localhost"))
        .await
        .expect("failed to bind socket");
    let addr = socket.local_addr().await.expect("failed to get local addr");
    (socket, addr)
}

/// Returns `(stream_a, stream_b, socket_a, socket_b)` where stream_a (local_id=1)
/// connects to stream_b (local_id=2) and vice versa, each on its own socket.
pub async fn create_connected_pair(
    runtime: &UdxRuntime,
) -> (UdxStream, UdxStream, UdxSocket, UdxSocket) {
    let (socket_a, addr_a) = create_bound_socket(runtime).await;
    let (socket_b, addr_b) = create_bound_socket(runtime).await;

    let stream_a = runtime.create_stream(1).await.expect("failed to create stream_a");
    let stream_b = runtime.create_stream(2).await.expect("failed to create stream_b");

    stream_a
        .connect(&socket_a, 2, addr_b)
        .await
        .expect("stream_a connect failed");

    stream_b
        .connect(&socket_b, 1, addr_a)
        .await
        .expect("stream_b connect failed");

    (stream_a, stream_b, socket_a, socket_b)
}

/// Deterministic pseudo-random payload via LCG PRNG seeded with `size`.
pub fn random_payload(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let mut state: u64 = size as u64 ^ 0xDEAD_BEEF_CAFE_BABE;
    for _ in 0..size {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        buf.push((state >> 33) as u8);
    }
    buf
}

pub fn verify_payload(sent: &[u8], received: &[u8]) {
    assert_eq!(
        sent.len(),
        received.len(),
        "payload size mismatch: sent {} bytes, received {} bytes",
        sent.len(),
        received.len()
    );

    let sent_hash = Sha256::digest(sent);
    let received_hash = Sha256::digest(received);

    assert_eq!(
        sent_hash, received_hash,
        "payload integrity mismatch: sent SHA256={:x}, received SHA256={:x} ({} bytes)",
        sent_hash,
        received_hash,
        sent.len()
    );
}

pub async fn with_timeout<F: std::future::Future>(duration: Duration, fut: F) -> F::Output {
    tokio::time::timeout(duration, fut)
        .await
        .expect("test timed out")
}

pub fn node_script_path(name: &str) -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    format!("{manifest}/../tests/node/{name}")
}

pub async fn create_lossy_pair(
    runtime: &UdxRuntime,
    config: proxy::ProxyConfig,
) -> (UdxStream, UdxStream, UdxSocket, UdxSocket, proxy::UdpProxy) {
    let (socket_a, addr_a) = create_bound_socket(runtime).await;
    let (socket_b, addr_b) = create_bound_socket(runtime).await;

    let proxy = proxy::UdpProxy::start(addr_a, addr_b, config).await;

    let stream_a = runtime.create_stream(1).await.expect("failed to create stream_a");
    let stream_b = runtime.create_stream(2).await.expect("failed to create stream_b");

    stream_a
        .connect(&socket_a, 2, proxy.addr_for_a)
        .await
        .expect("stream_a connect failed");

    stream_b
        .connect(&socket_b, 1, proxy.addr_for_b)
        .await
        .expect("stream_b connect failed");

    (stream_a, stream_b, socket_a, socket_b, proxy)
}
