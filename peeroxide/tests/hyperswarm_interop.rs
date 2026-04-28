//! M8 interop: Rust peeroxide (Hyperswarm) ↔ Node.js hyperswarm.
//!
//! 1. Node.js creates a local testnet + Hyperswarm, joins a topic as server.
//! 2. Rust peeroxide joins the same topic as client, discovers the peer,
//!    and receives a SwarmConnection through the channel.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use peeroxide::{discovery_key, spawn, JoinOpts, SwarmConfig};
use peeroxide_dht::hyperdht::HyperDhtConfig;
use peeroxide_dht::rpc::DhtConfig;
use serde_json::Value;

fn server_script_path() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    format!("{manifest}/../tests/node/hyperswarm-server.js")
}

struct JsSwarm {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: Arc<Mutex<Vec<String>>>,
    _reader_thread: std::thread::JoinHandle<()>,
    next_id: u32,
}

impl JsSwarm {
    fn start() -> Self {
        let mut child = Command::new("node")
            .arg(server_script_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn node hyperswarm-server.js");

        let stdout = child.stdout.take().expect("no stdout");
        let stdin = child.stdin.take().expect("no stdin");
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();

        let reader_thread = std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                lines_clone.lock().unwrap().push(line);
            }
        });

        Self {
            child,
            stdin,
            lines,
            _reader_thread: reader_thread,
            next_id: 1,
        }
    }

    fn read_ready(&self) -> (u16, String) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            {
                let lines = self.lines.lock().unwrap();
                if let Some(line) = lines.first() {
                    let v: Value = serde_json::from_str(line).expect("invalid JSON from JS");
                    assert!(v["ready"].as_bool().unwrap_or(false), "JS not ready");
                    let port = v["port"].as_u64().expect("missing port") as u16;
                    let pk = v["publicKey"].as_str().expect("missing publicKey").to_string();
                    return (port, pk);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for JS ready"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn send_cmd(&mut self, cmd: Value) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = cmd;
        msg["id"] = serde_json::json!(id);
        let line = serde_json::to_string(&msg).unwrap() + "\n";
        self.stdin
            .write_all(line.as_bytes())
            .expect("write to JS stdin");
        self.stdin.flush().expect("flush JS stdin");
        id
    }

    fn wait_reply(&self, id: u32) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            {
                let lines = self.lines.lock().unwrap();
                for line in lines.iter() {
                    if let Ok(v) = serde_json::from_str::<Value>(line) {
                        if v["id"].as_u64() == Some(id as u64) {
                            return v;
                        }
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for JS reply id={id}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_event(&self, event_name: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            {
                let lines = self.lines.lock().unwrap();
                for line in lines.iter() {
                    if let Ok(v) = serde_json::from_str::<Value>(line) {
                        if v["event"].as_str() == Some(event_name) {
                            return v;
                        }
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for JS event '{event_name}'"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn join_server(&mut self, topic_hex: &str) {
        let id = self.send_cmd(serde_json::json!({ "cmd": "join", "topic": topic_hex }));
        let reply = self.wait_reply(id);
        assert!(
            reply["ok"].as_bool().unwrap_or(false),
            "JS join failed: {reply}"
        );
    }
}

impl Drop for JsSwarm {
    fn drop(&mut self) {
        let _ = self
            .stdin
            .write_all(b"{\"cmd\":\"shutdown\",\"id\":0}\n");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn hyperswarm_cross_language_connect() {
    let _ = tracing_subscriber::fmt::try_init();

    let result = tokio::time::timeout(Duration::from_secs(60), run_interop()).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("hyperswarm interop test failed: {e}"),
        Err(_) => panic!("hyperswarm interop test timed out after 60s"),
    }
}

async fn run_interop() -> Result<(), Box<dyn std::error::Error>> {
    let mut js = JsSwarm::start();
    let (bs_port, _server_pk) = js.read_ready();
    tracing::info!(bs_port, "JS testnet ready");

    let topic = discovery_key(b"peeroxide-m8-interop-test");
    let topic_hex: String = topic.iter().map(|b| format!("{b:02x}")).collect();

    js.join_server(&topic_hex);
    tracing::info!("JS joined topic as server");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut dht_config = DhtConfig::default();
    dht_config.bootstrap = vec![format!("127.0.0.1:{bs_port}")];
    dht_config.port = 0;
    dht_config.host = "127.0.0.1".to_string();
    dht_config.firewalled = true;

    let mut hyper_config = HyperDhtConfig::default();
    hyper_config.dht = dht_config;

    let mut config = SwarmConfig::default();
    config.dht = hyper_config;

    let (_task, handle, mut conn_rx) = spawn(config).await?;
    tracing::info!("Rust swarm started");

    handle
        .join(
            topic,
            {
                let mut opts = JoinOpts::default();
                opts.server = false;
                opts
            },
        )
        .await?;
    tracing::info!("Rust joined topic as client");

    let mut conn = tokio::time::timeout(Duration::from_secs(30), conn_rx.recv())
        .await
        .map_err(|_| "timed out waiting for SwarmConnection")?
        .ok_or("conn_rx closed without delivering a connection")?;

    tracing::info!(
        remote_pk = hex::encode(conn.remote_public_key()),
        is_initiator = conn.is_initiator,
        "Rust received SwarmConnection"
    );

    assert!(conn.is_initiator, "Rust should be the initiator (client)");

    let connected_event = js.wait_event("connected");
    tracing::info!("JS saw connection: {connected_event}");

    let data = tokio::time::timeout(Duration::from_secs(10), conn.peer.stream.read())
        .await
        .map_err(|_| "timed out reading from encrypted stream")?
        .map_err(|e| format!("read error: {e}"))?
        .ok_or("stream closed without data")?;

    let msg = String::from_utf8_lossy(&data);
    tracing::info!(msg = %msg, "Rust read message from JS");
    assert_eq!(msg, "hello from node", "unexpected message from JS");

    handle.destroy().await?;
    drop(js);

    Ok(())
}
