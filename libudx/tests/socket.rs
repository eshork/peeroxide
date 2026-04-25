#![deny(clippy::all)]

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::{create_bound_socket, create_runtime, with_timeout, DEFAULT_TIMEOUT};

#[tokio::test]
async fn socket_bind_and_local_addr() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (socket, addr) = create_bound_socket(&runtime).await;

        assert!(addr.port() > 0, "expected non-zero port, got {}", addr.port());
        assert_eq!(addr.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());

        socket.close().await.expect("close failed");
    })
    .await;
}

#[tokio::test]
async fn socket_send_recv_datagram() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let (socket_a, addr_a) = create_bound_socket(&runtime).await;
        let (socket_b, addr_b) = create_bound_socket(&runtime).await;

        let mut rx = socket_b.recv_start().expect("recv_start failed");

        let test_data = b"hello datagram";
        socket_a
            .send_to(test_data, addr_b)
            .expect("send_to failed");

        let datagram = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("recv timed out")
            .expect("channel closed");

        assert_eq!(datagram.data, test_data);
        assert_eq!(datagram.addr.port(), addr_a.port());

        socket_a.close().await.expect("close a failed");
        socket_b.close().await.expect("close b failed");
    })
    .await;
}

#[tokio::test]
async fn socket_close() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(DEFAULT_TIMEOUT, async {
        let runtime = create_runtime();
        let socket = runtime
            .create_socket()
            .await
            .expect("create socket failed");
        socket
            .bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .await
            .expect("bind failed");

        socket.close().await.expect("close failed");
    })
    .await;
}
