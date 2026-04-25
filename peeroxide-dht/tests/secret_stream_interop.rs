use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use peeroxide_dht::noise;
use peeroxide_dht::secret_stream::SecretStream;

struct NodeServer {
    child: Child,
    port: u16,
}

impl NodeServer {
    fn spawn() -> Self {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let script = format!("{manifest}/../tests/node/secret-stream-server.js");

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
    let mut ss = SecretStream::new(true, tcp, keypair)
        .await
        .expect("SecretStream handshake failed");

    ss.write(b"ping").await.unwrap();
    let reply = ss.read().await.unwrap().expect("expected pong");
    assert_eq!(reply, b"pong");

    ss.write(b"hello from rust").await.unwrap();
    let reply = ss.read().await.unwrap().expect("expected hello");
    assert_eq!(reply, b"hello from node");

    for i in 0..5 {
        let msg = format!("multi {i}");
        ss.write(msg.as_bytes()).await.unwrap();
        let reply = ss.read().await.unwrap().expect("expected ack");
        let expected = format!("ack {i}");
        assert_eq!(reply, expected.as_bytes());
    }

    let eof = ss.read().await.unwrap();
    assert!(eof.is_none(), "expected EOF after all messages");
}
