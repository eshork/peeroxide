#![deny(clippy::all)]

mod common;

use common::{create_runtime, node_script_path, random_payload, verify_payload, with_timeout};
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::Duration;

fn spawn_echo_server() -> (std::process::Child, BufReader<std::process::ChildStdout>, std::process::ChildStdin, u16, u32, u32) {
    let mut child = Command::new("node")
        .arg(node_script_path("udx-echo-server.js"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn node");

    let stdout = child.stdout.take().expect("stdout");
    let stdin = child.stdin.take().expect("stdin");

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read handshake");
    let info: serde_json::Value = serde_json::from_str(line.trim()).expect("parse handshake");
    let port = info["port"].as_u64().expect("port") as u16;
    let local_id = info["localId"].as_u64().expect("localId") as u32;
    let remote_id = info["remoteId"].as_u64().expect("remoteId") as u32;

    (child, reader, stdin, port, local_id, remote_id)
}

async fn connect_to_echo(
    runtime: &libudx::UdxRuntime,
    node_port: u16,
    node_local_id: u32,
    node_remote_id: u32,
    mut stdin: std::process::ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
) -> (libudx::UdxStream, libudx::UdxSocket) {
    let socket = runtime.create_socket().await.expect("create socket");
    socket
        .bind("127.0.0.1:0".parse::<SocketAddr>().expect("parse"))
        .await
        .expect("bind");
    let local_addr = socket.local_addr().await.expect("local_addr");
    let rust_port = local_addr.port();

    let handshake = serde_json::json!({ "port": rust_port });
    writeln!(stdin, "{handshake}").expect("write handshake");
    drop(stdin);

    let mut ready_line = String::new();
    reader.read_line(&mut ready_line).expect("read ready");
    let ready: serde_json::Value = serde_json::from_str(ready_line.trim()).expect("parse ready");
    assert!(ready["ready"].as_bool().unwrap_or(false), "node not ready");

    let stream = runtime
        .create_stream(node_remote_id)
        .await
        .expect("create stream");
    let node_addr: SocketAddr = format!("127.0.0.1:{node_port}").parse().expect("parse addr");
    stream
        .connect(&socket, node_local_id, node_addr)
        .await
        .expect("connect");

    (stream, socket)
}

#[tokio::test]
async fn wire_compat_echo_small() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let (mut child, mut reader, stdin, port, local_id, remote_id) = spawn_echo_server();
        let runtime = create_runtime();
        let (mut stream, _socket) =
            connect_to_echo(&runtime, port, local_id, remote_id, stdin, &mut reader).await;

        let payload = random_payload(100);
        stream.write(&payload).await.expect("write");
        let received = stream.read().await.expect("read").expect("EOF");
        verify_payload(&payload, &received);

        stream.shutdown().await.expect("shutdown");
        let eof = stream.read().await.expect("read eof");
        assert!(eof.is_none(), "expected EOF");

        child.wait().expect("child exit");
    })
    .await;
}

#[tokio::test]
async fn wire_compat_echo_large() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let (mut child, mut reader, stdin, port, local_id, remote_id) = spawn_echo_server();
        let runtime = create_runtime();
        let (mut stream, _socket) =
            connect_to_echo(&runtime, port, local_id, remote_id, stdin, &mut reader).await;

        let payload = random_payload(100 * 1024);
        stream.write(&payload).await.expect("write");

        let mut received = Vec::new();
        while received.len() < payload.len() {
            let chunk = stream.read().await.expect("read").expect("EOF");
            received.extend_from_slice(&chunk);
        }
        verify_payload(&payload, &received);

        stream.shutdown().await.expect("shutdown");
        let eof = stream.read().await.expect("read eof");
        assert!(eof.is_none(), "expected EOF");

        child.wait().expect("child exit");
    })
    .await;
}

#[tokio::test]
async fn wire_compat_bidirectional() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let (mut child, mut reader, stdin, port, local_id, remote_id) = spawn_echo_server();
        let runtime = create_runtime();
        let (mut stream, _socket) =
            connect_to_echo(&runtime, port, local_id, remote_id, stdin, &mut reader).await;

        for i in 0..5 {
            let payload = random_payload(1024 + i * 100);
            stream.write(&payload).await.expect("write");

            let mut received = Vec::new();
            while received.len() < payload.len() {
                let chunk = stream.read().await.expect("read").expect("EOF");
                received.extend_from_slice(&chunk);
            }
            verify_payload(&payload, &received);
        }

        stream.shutdown().await.expect("shutdown");
        let eof = stream.read().await.expect("read eof");
        assert!(eof.is_none(), "expected EOF");

        child.wait().expect("child exit");
    })
    .await;
}

#[tokio::test]
async fn wire_compat_shutdown_sequence() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(15), async {
        let (mut child, mut reader, stdin, port, local_id, remote_id) = spawn_echo_server();
        let runtime = create_runtime();
        let (mut stream, _socket) =
            connect_to_echo(&runtime, port, local_id, remote_id, stdin, &mut reader).await;

        let payload = b"shutdown test data";
        stream.write(payload).await.expect("write");
        let received = stream.read().await.expect("read").expect("EOF");
        assert_eq!(received, payload);

        stream.shutdown().await.expect("shutdown");

        let eof = stream.read().await.expect("read after shutdown");
        assert!(eof.is_none(), "expected EOF after shutdown");

        let status = child.wait().expect("child exit");
        assert!(status.success(), "node exited with: {status}");
    })
    .await;
}

#[tokio::test]
async fn wire_compat_multiple_streams() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let mut child = Command::new("node")
            .arg(node_script_path("udx-echo-server-multi.js"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn node");

        let stdout = child.stdout.take().expect("stdout");
        let mut stdin = child.stdin.take().expect("stdin");
        let mut reader = BufReader::new(stdout);

        let mut line = String::new();
        reader.read_line(&mut line).expect("read handshake");
        let info: serde_json::Value = serde_json::from_str(line.trim()).expect("parse");
        let node_port = info["port"].as_u64().expect("port") as u16;

        let runtime = create_runtime();
        let socket = runtime.create_socket().await.expect("socket");
        socket
            .bind("127.0.0.1:0".parse::<SocketAddr>().expect("parse"))
            .await
            .expect("bind");
        let rust_port = socket.local_addr().await.expect("addr").port();

        let stream_configs: Vec<(u32, u32)> = vec![(100, 101), (200, 201), (300, 301)];

        let node_streams: Vec<serde_json::Value> = stream_configs
            .iter()
            .map(|(rust_local, rust_remote)| {
                serde_json::json!({
                    "localId": rust_remote,
                    "remoteId": rust_local
                })
            })
            .collect();

        let handshake = serde_json::json!({
            "port": rust_port,
            "streams": node_streams
        });
        writeln!(stdin, "{handshake}").expect("write handshake");
        drop(stdin);

        let mut ready_line = String::new();
        reader.read_line(&mut ready_line).expect("read ready");
        let ready: serde_json::Value = serde_json::from_str(ready_line.trim()).expect("parse ready");
        assert!(ready["ready"].as_bool().unwrap_or(false), "node not ready");

        let node_addr: SocketAddr = format!("127.0.0.1:{node_port}").parse().expect("parse addr");

        let mut streams = Vec::new();
        for (rust_local, rust_remote) in &stream_configs {
            let s = runtime.create_stream(*rust_local).await.expect("create stream");
            s.connect(&socket, *rust_remote, node_addr).await.expect("connect");
            streams.push(s);
        }

        for (i, stream) in streams.iter_mut().enumerate() {
            let payload = random_payload(512 * (i + 1));
            stream.write(&payload).await.expect("write");

            let mut received = Vec::new();
            while received.len() < payload.len() {
                let chunk = stream.read().await.expect("read").expect("EOF");
                received.extend_from_slice(&chunk);
            }
            verify_payload(&payload, &received);
        }

        for stream in &streams {
            stream.shutdown().await.expect("shutdown");
        }

        child.wait().expect("child exit");
    })
    .await;
}

#[tokio::test]
async fn wire_compat_message_ordering() {
    let _ = tracing_subscriber::fmt::try_init();

    with_timeout(Duration::from_secs(30), async {
        let (mut child, mut reader, stdin, port, local_id, remote_id) = spawn_echo_server();
        let runtime = create_runtime();
        let (mut stream, _socket) =
            connect_to_echo(&runtime, port, local_id, remote_id, stdin, &mut reader).await;

        let mut all_sent = Vec::new();
        for i in 0u32..50 {
            let mut msg = format!("msg_{i:04}").into_bytes();
            msg.resize(100, b'.');
            all_sent.extend_from_slice(&msg);
            stream.write(&msg).await.expect("write");
        }

        let mut all_received = Vec::new();
        while all_received.len() < all_sent.len() {
            let chunk = stream.read().await.expect("read").expect("EOF");
            all_received.extend_from_slice(&chunk);
        }

        assert_eq!(all_received.len(), all_sent.len(), "total byte count mismatch");
        assert_eq!(all_received, all_sent, "message ordering or content mismatch");

        stream.shutdown().await.expect("shutdown");
        let eof = stream.read().await.expect("read eof");
        assert!(eof.is_none(), "expected EOF");

        child.wait().expect("child exit");
    })
    .await;
}
