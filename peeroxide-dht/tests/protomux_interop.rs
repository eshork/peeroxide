#![deny(clippy::all)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use peeroxide_dht::noise;
use peeroxide_dht::protomux::{ChannelEvent, Mux};
use peeroxide_dht::secret_stream::SecretStream;

struct NodeServer {
    child: Child,
    port: u16,
}

impl NodeServer {
    fn spawn() -> Self {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let script = format!("{manifest}/../tests/node/protomux-server.js");

        let mut child = Command::new("node")
            .arg(&script)
            .env("PORT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to spawn node: {e}. Is node installed?"));

        let stdout = child.stdout.take().expect("stdout piped");
        let reader = BufReader::new(stdout);

        let mut port = 0u16;
        for line in reader.lines() {
            let line = line.expect("read line");
            if let Some(p) = line.strip_prefix("LISTENING:") {
                port = p.parse().expect("valid port");
                break;
            }
        }
        assert_ne!(port, 0, "Node server did not report listening port");
        Self { child, port }
    }
}

impl Drop for NodeServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn rust_initiator_node_responder() {
    let server = NodeServer::spawn();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let tcp = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", server.port))
        .await
        .expect("TCP connect failed");

    let keypair = noise::generate_keypair();
    let ss = SecretStream::new(true, tcp, keypair)
        .await
        .expect("SecretStream handshake failed");

    let (mux, mux_run) = Mux::new(ss);
    tokio::spawn(mux_run);

    let mut channel = mux
        .create_channel("peeroxide-interop-test", None, None)
        .await
        .expect("create_channel failed");

    let result = tokio::time::timeout(Duration::from_secs(30), async {
        channel.wait_opened().await.expect("channel open failed");

        let event = channel.recv().await.expect("expected message");
        match event {
            ChannelEvent::Message { message_type: 0, data } => {
                assert_eq!(data, b"hello from node");
            }
            other => panic!("expected Message(0, hello from node), got {other:?}"),
        }

        channel.send(0, b"hello from rust").expect("send failed");

        let event = channel.recv().await.expect("expected echo message");
        match event {
            ChannelEvent::Message { message_type: 0, data } => {
                assert_eq!(data, b"echo: hello from rust");
            }
            other => panic!("expected Message(0, echo: hello from rust), got {other:?}"),
        }

        let event = channel.recv().await.expect("expected goodbye message");
        match event {
            ChannelEvent::Message { message_type: 0, data } => {
                assert_eq!(data, b"goodbye");
            }
            other => panic!("expected Message(0, goodbye), got {other:?}"),
        }

        let event = channel.recv().await.expect("expected close event");
        match event {
            ChannelEvent::Closed { .. } => {}
            other => panic!("expected Closed, got {other:?}"),
        }
    })
    .await;

    result.expect("test timed out after 30s");
}
