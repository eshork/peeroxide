#![deny(clippy::all)]

mod common;

use common::{create_runtime, random_payload, verify_payload, with_timeout};
use std::net::SocketAddr;
use std::time::Duration;

const LOCALHOST: &str = "127.0.0.1:0";

fn addr() -> SocketAddr {
    LOCALHOST.parse().expect("parse localhost")
}

async fn create_stream_pair(
    runtime: &libudx::UdxRuntime,
    socket_a: &libudx::UdxSocket,
    socket_b: &libudx::UdxSocket,
    addr_a: SocketAddr,
    addr_b: SocketAddr,
    id_a: u32,
    id_b: u32,
) -> (libudx::UdxStream, libudx::UdxStream) {
    let stream_a = runtime.create_stream(id_a).await.expect("create stream_a");
    let stream_b = runtime.create_stream(id_b).await.expect("create stream_b");
    stream_a
        .connect(socket_a, id_b, addr_b)
        .await
        .expect("connect stream_a");
    stream_b
        .connect(socket_b, id_a, addr_a)
        .await
        .expect("connect stream_b");
    (stream_a, stream_b)
}

#[tokio::test]
async fn socket_multiple_streams_3() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let rt = create_runtime();
        let socket_a = rt.create_socket().await.expect("socket_a");
        socket_a.bind(addr()).await.expect("bind_a");
        let addr_a = socket_a.local_addr().await.expect("addr_a");

        let socket_b = rt.create_socket().await.expect("socket_b");
        socket_b.bind(addr()).await.expect("bind_b");
        let addr_b = socket_b.local_addr().await.expect("addr_b");

        let mut pairs = Vec::new();
        for i in 0u32..3 {
            let id_a = 100 + i * 2;
            let id_b = 100 + i * 2 + 1;
            let (sa, sb) =
                create_stream_pair(&rt, &socket_a, &socket_b, addr_a, addr_b, id_a, id_b).await;
            pairs.push((sa, sb));
        }

        for (i, pair) in pairs.iter_mut().enumerate() {
            let payload = random_payload(100 + i * 50);
            pair.0.write(&payload).await.expect("write");
            let received = pair.1.read().await.expect("read").expect("EOF");
            verify_payload(&payload, &received);
        }
    })
    .await;
}

#[tokio::test]
async fn socket_10_streams() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let rt = create_runtime();
        let socket_a = rt.create_socket().await.expect("socket_a");
        socket_a.bind(addr()).await.expect("bind_a");
        let addr_a = socket_a.local_addr().await.expect("addr_a");

        let socket_b = rt.create_socket().await.expect("socket_b");
        socket_b.bind(addr()).await.expect("bind_b");
        let addr_b = socket_b.local_addr().await.expect("addr_b");

        let mut pairs = Vec::new();
        for i in 0u32..10 {
            let id_a = 200 + i * 2;
            let id_b = 200 + i * 2 + 1;
            let (sa, sb) =
                create_stream_pair(&rt, &socket_a, &socket_b, addr_a, addr_b, id_a, id_b).await;
            pairs.push((sa, sb));
        }

        for (i, pair) in pairs.iter_mut().enumerate() {
            let payload = random_payload(1024 * (i + 1));
            pair.0.write(&payload).await.expect("write");

            let mut received = Vec::new();
            while received.len() < payload.len() {
                let chunk = pair.1.read().await.expect("read").expect("EOF");
                received.extend_from_slice(&chunk);
            }
            verify_payload(&payload, &received);
        }
    })
    .await;
}

#[tokio::test]
async fn socket_50_streams() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(60), async {
        let rt = create_runtime();
        let socket_a = rt.create_socket().await.expect("socket_a");
        socket_a.bind(addr()).await.expect("bind_a");
        let addr_a = socket_a.local_addr().await.expect("addr_a");

        let socket_b = rt.create_socket().await.expect("socket_b");
        socket_b.bind(addr()).await.expect("bind_b");
        let addr_b = socket_b.local_addr().await.expect("addr_b");

        let mut pairs = Vec::new();
        for i in 0u32..50 {
            let id_a = 500 + i * 2;
            let id_b = 500 + i * 2 + 1;
            let (sa, sb) =
                create_stream_pair(&rt, &socket_a, &socket_b, addr_a, addr_b, id_a, id_b).await;
            pairs.push((sa, sb));
        }

        for pair in pairs.iter_mut() {
            let payload = random_payload(256);
            pair.0.write(&payload).await.expect("write");
            let received = pair.1.read().await.expect("read").expect("EOF");
            verify_payload(&payload, &received);
        }
    })
    .await;
}

#[tokio::test]
async fn socket_stream_ids_sparse() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let rt = create_runtime();
        let socket_a = rt.create_socket().await.expect("socket_a");
        socket_a.bind(addr()).await.expect("bind_a");
        let addr_a = socket_a.local_addr().await.expect("addr_a");

        let socket_b = rt.create_socket().await.expect("socket_b");
        socket_b.bind(addr()).await.expect("bind_b");
        let addr_b = socket_b.local_addr().await.expect("addr_b");

        let sparse_ids: [(u32, u32); 4] = [(1, 1000), (42, 9999), (7777, 65535), (100_000, 200_000)];

        let mut pairs = Vec::new();
        for (id_a, id_b) in sparse_ids {
            let (sa, sb) =
                create_stream_pair(&rt, &socket_a, &socket_b, addr_a, addr_b, id_a, id_b).await;
            pairs.push((sa, sb));
        }

        for (idx, pair) in pairs.iter_mut().enumerate() {
            let msg = format!("sparse stream {idx}");
            pair.0.write(msg.as_bytes()).await.expect("write");
            let received = pair.1.read().await.expect("read").expect("EOF");
            assert_eq!(received, msg.as_bytes());
        }
    })
    .await;
}
