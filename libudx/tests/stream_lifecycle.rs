#![deny(clippy::all)]

mod common;

use common::{
    create_connected_pair, create_runtime, random_payload, verify_payload, with_timeout,
    DEFAULT_TIMEOUT,
};
use std::time::Duration;

#[tokio::test]
async fn stream_connect_basic() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (_stream_a, _stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;
    })
    .await;
}

#[tokio::test]
async fn stream_write_read_small() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload = random_payload(100);
        stream_a.write(&payload).await.expect("write failed");

        let received = stream_b.read().await.expect("read failed").expect("unexpected EOF");
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn stream_write_read_1kb() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload = random_payload(1024);
        stream_a.write(&payload).await.expect("write failed");

        let mut received = Vec::new();
        while received.len() < payload.len() {
            let chunk = stream_b.read().await.expect("read failed").expect("unexpected EOF");
            received.extend_from_slice(&chunk);
        }
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn stream_write_read_64kb() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload = random_payload(64 * 1024);
        stream_a.write(&payload).await.expect("write failed");

        let mut received = Vec::new();
        while received.len() < payload.len() {
            let chunk = stream_b.read().await.expect("read failed").expect("unexpected EOF");
            received.extend_from_slice(&chunk);
        }
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn stream_write_read_1mb() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload = random_payload(1024 * 1024);
        stream_a.write(&payload).await.expect("write failed");

        let mut received = Vec::new();
        while received.len() < payload.len() {
            let chunk = stream_b.read().await.expect("read failed").expect("unexpected EOF");
            received.extend_from_slice(&chunk);
        }
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn stream_write_read_10mb() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(60), async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload = random_payload(10 * 1024 * 1024);
        stream_a.write(&payload).await.expect("write failed");

        let mut received = Vec::new();
        while received.len() < payload.len() {
            let chunk = stream_b.read().await.expect("read failed").expect("unexpected EOF");
            received.extend_from_slice(&chunk);
        }
        verify_payload(&payload, &received);
    })
    .await;
}

#[tokio::test]
async fn stream_multiple_writes() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let mut all_sent = Vec::new();
        for i in 0u32..100 {
            let size = ((i % 10) as usize + 1) * 100;
            let payload = random_payload(size);
            all_sent.extend_from_slice(&payload);
            stream_a.write(&payload).await.expect("write failed");
        }

        let mut all_received = Vec::new();
        while all_received.len() < all_sent.len() {
            let chunk = stream_b.read().await.expect("read failed").expect("unexpected EOF");
            all_received.extend_from_slice(&chunk);
        }
        verify_payload(&all_sent, &all_received);
    })
    .await;
}

#[tokio::test]
async fn stream_bidirectional() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let runtime = create_runtime();
        let (stream_a, stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload_a = random_payload(10_000);
        let payload_b = random_payload(10_000);

        let payload_a_clone = payload_a.clone();
        let payload_b_clone = payload_b.clone();

        let write_a = tokio::spawn({
            let sa = stream_a;
            let pb = payload_a_clone;
            async move {
                sa.write(&pb).await.expect("write_a failed");
                sa
            }
        });

        let write_b = tokio::spawn({
            let sb = stream_b;
            let pa = payload_b_clone;
            async move {
                sb.write(&pa).await.expect("write_b failed");
                sb
            }
        });

        let mut stream_a = write_a.await.expect("join write_a");
        let mut stream_b = write_b.await.expect("join write_b");

        let mut recv_a = Vec::new();
        while recv_a.len() < payload_b.len() {
            let chunk = stream_a.read().await.expect("read_a failed").expect("unexpected EOF");
            recv_a.extend_from_slice(&chunk);
        }
        verify_payload(&payload_b, &recv_a);

        let mut recv_b = Vec::new();
        while recv_b.len() < payload_a.len() {
            let chunk = stream_b.read().await.expect("read_b failed").expect("unexpected EOF");
            recv_b.extend_from_slice(&chunk);
        }
        verify_payload(&payload_a, &recv_b);
    })
    .await;
}

#[tokio::test]
async fn stream_shutdown_graceful() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        let payload = b"before shutdown";
        stream_a.write(payload).await.expect("write failed");

        let received = stream_b.read().await.expect("read failed").expect("unexpected EOF");
        assert_eq!(received, payload);

        stream_a.shutdown().await.expect("shutdown failed");

        let eof = stream_b.read().await.expect("read after shutdown failed");
        assert!(eof.is_none(), "expected EOF after remote shutdown");
    })
    .await;
}

#[tokio::test]
async fn stream_read_after_eof() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (stream_a, mut stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        stream_a.shutdown().await.expect("shutdown failed");

        let first = stream_b.read().await.expect("first read failed");
        assert!(first.is_none(), "expected None on first read after EOF");

        let second = stream_b.read().await;
        assert!(second.is_err() || second.unwrap().is_none(),
            "expected error or None on repeated read after EOF");
    })
    .await;
}

#[tokio::test]
async fn stream_destroy_immediate() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (stream_a, _stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        stream_a.destroy().await.expect("destroy failed");
    })
    .await;
}

#[tokio::test]
async fn stream_write_after_shutdown() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (stream_a, _stream_b, _socket_a, _socket_b) =
            create_connected_pair(&runtime).await;

        stream_a.shutdown().await.expect("shutdown failed");

        let result = stream_a.write(b"after shutdown").await;
        assert!(result.is_err(), "write after shutdown should fail");
    })
    .await;
}
