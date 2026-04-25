#![deny(clippy::all)]

mod common;

use common::{create_runtime, random_payload, verify_payload, with_timeout};
use std::net::SocketAddr;
use std::time::Duration;

const LOCALHOST: &str = "127.0.0.1:0";

fn addr() -> SocketAddr {
    LOCALHOST.parse().expect("parse localhost")
}

#[tokio::test]
async fn relay_basic_forward() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(10), async {
        let rt = create_runtime();

        let relay_sock_1 = rt.create_socket().await.expect("relay_sock_1");
        relay_sock_1.bind(addr()).await.expect("bind relay_1");
        let relay_addr_1 = relay_sock_1.local_addr().await.expect("addr relay_1");

        let relay_sock_2 = rt.create_socket().await.expect("relay_sock_2");
        relay_sock_2.bind(addr()).await.expect("bind relay_2");
        let relay_addr_2 = relay_sock_2.local_addr().await.expect("addr relay_2");

        let relay_stream_1 = rt.create_stream(10).await.expect("relay_stream_1");
        let relay_stream_2 = rt.create_stream(20).await.expect("relay_stream_2");

        let peer_a_sock = rt.create_socket().await.expect("peer_a_sock");
        peer_a_sock.bind(addr()).await.expect("bind peer_a");
        let peer_a_addr = peer_a_sock.local_addr().await.expect("addr peer_a");
        let peer_a_stream = rt.create_stream(1).await.expect("peer_a_stream");

        let peer_b_sock = rt.create_socket().await.expect("peer_b_sock");
        peer_b_sock.bind(addr()).await.expect("bind peer_b");
        let peer_b_addr = peer_b_sock.local_addr().await.expect("addr peer_b");
        let mut peer_b_stream = rt.create_stream(2).await.expect("peer_b_stream");

        relay_stream_1.relay_to(&relay_stream_2).expect("relay 1→2");
        relay_stream_2.relay_to(&relay_stream_1).expect("relay 2→1");

        relay_stream_1.connect(&relay_sock_1, 1, peer_a_addr).await.expect("connect relay_1");
        relay_stream_2.connect(&relay_sock_2, 2, peer_b_addr).await.expect("connect relay_2");
        peer_a_stream.connect(&peer_a_sock, 10, relay_addr_1).await.expect("connect peer_a");
        peer_b_stream.connect(&peer_b_sock, 20, relay_addr_2).await.expect("connect peer_b");

        peer_a_stream.write(b"relay forward test").await.expect("write");
        let data = peer_b_stream.read().await.expect("read").expect("EOF");
        assert_eq!(data, b"relay forward test");
    })
    .await;
}

#[tokio::test]
async fn relay_large_transfer() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let rt = create_runtime();

        let relay_sock_1 = rt.create_socket().await.expect("relay_sock_1");
        relay_sock_1.bind(addr()).await.expect("bind relay_1");
        let relay_addr_1 = relay_sock_1.local_addr().await.expect("addr relay_1");

        let relay_sock_2 = rt.create_socket().await.expect("relay_sock_2");
        relay_sock_2.bind(addr()).await.expect("bind relay_2");
        let relay_addr_2 = relay_sock_2.local_addr().await.expect("addr relay_2");

        let relay_stream_1 = rt.create_stream(10).await.expect("relay_stream_1");
        let relay_stream_2 = rt.create_stream(20).await.expect("relay_stream_2");

        let peer_a_sock = rt.create_socket().await.expect("peer_a_sock");
        peer_a_sock.bind(addr()).await.expect("bind peer_a");
        let peer_a_addr = peer_a_sock.local_addr().await.expect("addr peer_a");
        let peer_a_stream = rt.create_stream(1).await.expect("peer_a_stream");

        let peer_b_sock = rt.create_socket().await.expect("peer_b_sock");
        peer_b_sock.bind(addr()).await.expect("bind peer_b");
        let peer_b_addr = peer_b_sock.local_addr().await.expect("addr peer_b");
        let mut peer_b_stream = rt.create_stream(2).await.expect("peer_b_stream");

        relay_stream_1.relay_to(&relay_stream_2).expect("relay 1→2");
        relay_stream_2.relay_to(&relay_stream_1).expect("relay 2→1");

        relay_stream_1.connect(&relay_sock_1, 1, peer_a_addr).await.expect("connect relay_1");
        relay_stream_2.connect(&relay_sock_2, 2, peer_b_addr).await.expect("connect relay_2");
        peer_a_stream.connect(&peer_a_sock, 10, relay_addr_1).await.expect("connect peer_a");
        peer_b_stream.connect(&peer_b_sock, 20, relay_addr_2).await.expect("connect peer_b");

        let payload = random_payload(1024 * 1024);
        peer_a_stream.write(&payload).await.expect("write");

        let mut received = Vec::new();
        while received.len() < payload.len() {
            let chunk = peer_b_stream.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn relay_bidirectional_concurrent() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let rt = create_runtime();

        let relay_sock_1 = rt.create_socket().await.expect("relay_sock_1");
        relay_sock_1.bind(addr()).await.expect("bind relay_1");
        let relay_addr_1 = relay_sock_1.local_addr().await.expect("addr relay_1");

        let relay_sock_2 = rt.create_socket().await.expect("relay_sock_2");
        relay_sock_2.bind(addr()).await.expect("bind relay_2");
        let relay_addr_2 = relay_sock_2.local_addr().await.expect("addr relay_2");

        let relay_stream_1 = rt.create_stream(10).await.expect("relay_stream_1");
        let relay_stream_2 = rt.create_stream(20).await.expect("relay_stream_2");

        let peer_a_sock = rt.create_socket().await.expect("peer_a_sock");
        peer_a_sock.bind(addr()).await.expect("bind peer_a");
        let peer_a_addr = peer_a_sock.local_addr().await.expect("addr peer_a");
        let peer_a_stream = rt.create_stream(1).await.expect("peer_a_stream");

        let peer_b_sock = rt.create_socket().await.expect("peer_b_sock");
        peer_b_sock.bind(addr()).await.expect("bind peer_b");
        let peer_b_addr = peer_b_sock.local_addr().await.expect("addr peer_b");
        let peer_b_stream = rt.create_stream(2).await.expect("peer_b_stream");

        relay_stream_1.relay_to(&relay_stream_2).expect("relay 1→2");
        relay_stream_2.relay_to(&relay_stream_1).expect("relay 2→1");

        relay_stream_1.connect(&relay_sock_1, 1, peer_a_addr).await.expect("connect relay_1");
        relay_stream_2.connect(&relay_sock_2, 2, peer_b_addr).await.expect("connect relay_2");
        peer_a_stream.connect(&peer_a_sock, 10, relay_addr_1).await.expect("connect peer_a");
        peer_b_stream.connect(&peer_b_sock, 20, relay_addr_2).await.expect("connect peer_b");

        let payload_a = random_payload(10 * 1024);
        let payload_b = random_payload(10 * 1024);
        let payload_a_clone = payload_a.clone();
        let payload_b_clone = payload_b.clone();

        let write_a = tokio::spawn(async move {
            peer_a_stream.write(&payload_a_clone).await.expect("write_a");
            peer_a_stream
        });
        let write_b = tokio::spawn(async move {
            peer_b_stream.write(&payload_b_clone).await.expect("write_b");
            peer_b_stream
        });

        let mut peer_a_stream = write_a.await.expect("join write_a");
        let mut peer_b_stream = write_b.await.expect("join write_b");

        let mut recv_b = Vec::new();
        while recv_b.len() < payload_a.len() {
            let chunk = peer_b_stream.read().await.expect("read_b").expect("EOF");
            recv_b.extend_from_slice(&chunk);
        }
        verify_payload(&payload_a, &recv_b);

        let mut recv_a = Vec::new();
        while recv_a.len() < payload_b.len() {
            let chunk = peer_a_stream.read().await.expect("read_a").expect("EOF");
            recv_a.extend_from_slice(&chunk);
        }
        verify_payload(&payload_b, &recv_a);
    })
    .await;
}

#[tokio::test]
async fn relay_chain() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let rt = create_runtime();

        let r1a_sock = rt.create_socket().await.expect("r1a_sock");
        r1a_sock.bind(addr()).await.expect("bind r1a");
        let r1a_addr = r1a_sock.local_addr().await.expect("addr r1a");

        let r1b_sock = rt.create_socket().await.expect("r1b_sock");
        r1b_sock.bind(addr()).await.expect("bind r1b");
        let r1b_addr = r1b_sock.local_addr().await.expect("addr r1b");

        let r2a_sock = rt.create_socket().await.expect("r2a_sock");
        r2a_sock.bind(addr()).await.expect("bind r2a");
        let r2a_addr = r2a_sock.local_addr().await.expect("addr r2a");

        let r2b_sock = rt.create_socket().await.expect("r2b_sock");
        r2b_sock.bind(addr()).await.expect("bind r2b");
        let r2b_addr = r2b_sock.local_addr().await.expect("addr r2b");

        let peer_a_sock = rt.create_socket().await.expect("peer_a_sock");
        peer_a_sock.bind(addr()).await.expect("bind peer_a");
        let peer_a_addr = peer_a_sock.local_addr().await.expect("addr peer_a");

        let peer_b_sock = rt.create_socket().await.expect("peer_b_sock");
        peer_b_sock.bind(addr()).await.expect("bind peer_b");
        let peer_b_addr = peer_b_sock.local_addr().await.expect("addr peer_b");

        let r1a_stream = rt.create_stream(10).await.expect("r1a_stream");
        let r1b_stream = rt.create_stream(20).await.expect("r1b_stream");
        let r2a_stream = rt.create_stream(30).await.expect("r2a_stream");
        let r2b_stream = rt.create_stream(40).await.expect("r2b_stream");
        let mut peer_a_stream = rt.create_stream(1).await.expect("peer_a_stream");
        let mut peer_b_stream = rt.create_stream(2).await.expect("peer_b_stream");

        r1a_stream.relay_to(&r1b_stream).expect("relay r1a→r1b");
        r1b_stream.relay_to(&r1a_stream).expect("relay r1b→r1a");
        r2a_stream.relay_to(&r2b_stream).expect("relay r2a→r2b");
        r2b_stream.relay_to(&r2a_stream).expect("relay r2b→r2a");

        r1a_stream.connect(&r1a_sock, 1, peer_a_addr).await.expect("connect r1a");
        r1b_stream.connect(&r1b_sock, 30, r2a_addr).await.expect("connect r1b");
        r2a_stream.connect(&r2a_sock, 20, r1b_addr).await.expect("connect r2a");
        r2b_stream.connect(&r2b_sock, 2, peer_b_addr).await.expect("connect r2b");
        peer_a_stream.connect(&peer_a_sock, 10, r1a_addr).await.expect("connect peer_a");
        peer_b_stream.connect(&peer_b_sock, 40, r2b_addr).await.expect("connect peer_b");

        peer_a_stream.write(b"chain test").await.expect("write a→b");
        let data = peer_b_stream.read().await.expect("read b").expect("EOF");
        assert_eq!(data, b"chain test");

        peer_b_stream.write(b"chain reply").await.expect("write b→a");
        let data = peer_a_stream.read().await.expect("read a").expect("EOF");
        assert_eq!(data, b"chain reply");
    })
    .await;
}

#[tokio::test]
async fn relay_destroy_propagation() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let rt = create_runtime();

        let relay_sock_1 = rt.create_socket().await.expect("relay_sock_1");
        relay_sock_1.bind(addr()).await.expect("bind relay_1");
        let relay_addr_1 = relay_sock_1.local_addr().await.expect("addr relay_1");

        let relay_sock_2 = rt.create_socket().await.expect("relay_sock_2");
        relay_sock_2.bind(addr()).await.expect("bind relay_2");
        let relay_addr_2 = relay_sock_2.local_addr().await.expect("addr relay_2");

        let relay_stream_1 = rt.create_stream(10).await.expect("relay_stream_1");
        let relay_stream_2 = rt.create_stream(20).await.expect("relay_stream_2");

        let peer_a_sock = rt.create_socket().await.expect("peer_a_sock");
        peer_a_sock.bind(addr()).await.expect("bind peer_a");
        let peer_a_addr = peer_a_sock.local_addr().await.expect("addr peer_a");
        let peer_a_stream = rt.create_stream(1).await.expect("peer_a_stream");

        let peer_b_sock = rt.create_socket().await.expect("peer_b_sock");
        peer_b_sock.bind(addr()).await.expect("bind peer_b");
        let peer_b_addr = peer_b_sock.local_addr().await.expect("addr peer_b");
        let mut peer_b_stream = rt.create_stream(2).await.expect("peer_b_stream");

        relay_stream_1.relay_to(&relay_stream_2).expect("relay 1→2");
        relay_stream_2.relay_to(&relay_stream_1).expect("relay 2→1");

        relay_stream_1.connect(&relay_sock_1, 1, peer_a_addr).await.expect("connect relay_1");
        relay_stream_2.connect(&relay_sock_2, 2, peer_b_addr).await.expect("connect relay_2");
        peer_a_stream.connect(&peer_a_sock, 10, relay_addr_1).await.expect("connect peer_a");
        peer_b_stream.connect(&peer_b_sock, 20, relay_addr_2).await.expect("connect peer_b");

        peer_a_stream.write(b"before destroy").await.expect("write");
        let data = peer_b_stream.read().await.expect("read").expect("EOF");
        assert_eq!(data, b"before destroy");

        peer_a_stream.destroy().await.expect("destroy");

        let read_result = tokio::time::timeout(
            Duration::from_secs(3),
            peer_b_stream.read(),
        )
        .await;

        match read_result {
            Ok(Ok(None)) => {}
            Ok(Err(_)) => {}
            Err(_) => panic!("timed out waiting for destroy propagation through relay"),
            Ok(Ok(Some(_))) => panic!("expected EOF or error after peer destroy"),
        }
    })
    .await;
}
