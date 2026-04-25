//! Live interop test: Rust HyperDHT ↔ Node.js hyperdht.
//!
//! Verifies cross-language peer discovery:
//!   1. Node.js announces a topic → Rust looks it up and finds the peer.
//!   2. Rust announces a topic → Node.js looks it up and finds the peer.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use libudx::UdxRuntime;
use peeroxide_dht::crypto::hash;
use peeroxide_dht::hyperdht::{HyperDhtConfig, KeyPair, spawn};
use peeroxide_dht::rpc::DhtConfig;
use serde_json::Value;

fn node_peer_path() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    format!("{manifest}/../tests/node/hyperdht-peer.js")
}

struct JsPeer {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: Arc<Mutex<Vec<String>>>,
    _reader_thread: std::thread::JoinHandle<()>,
    next_id: u32,
}

impl JsPeer {
    fn spawn() -> Self {
        let mut child = Command::new("node")
            .arg(node_peer_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn node hyperdht-peer.js");

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

    fn read_ready_port(&self) -> u16 {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            {
                let lines = self.lines.lock().unwrap();
                if let Some(line) = lines.first() {
                    let v: Value = serde_json::from_str(line).expect("invalid JSON from JS peer");
                    assert!(v["ready"].as_bool().unwrap_or(false), "JS peer not ready");
                    return v["port"].as_u64().expect("missing port") as u16;
                }
            }
            assert!(std::time::Instant::now() < deadline, "timed out waiting for JS peer ready");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn send_cmd(&mut self, cmd: serde_json::Value) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = cmd;
        msg["id"] = serde_json::json!(id);
        let line = serde_json::to_string(&msg).unwrap() + "\n";
        self.stdin.write_all(line.as_bytes()).expect("write to JS stdin");
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
            assert!(std::time::Instant::now() < deadline, "timed out waiting for JS reply id={id}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn announce(&mut self, topic_hex: &str) -> Value {
        let id = self.send_cmd(serde_json::json!({ "cmd": "announce", "topic": topic_hex }));
        let reply = self.wait_reply(id);
        assert!(reply["ok"].as_bool().unwrap_or(false), "JS announce failed: {reply}");
        reply
    }

    fn lookup(&mut self, topic_hex: &str) -> Vec<String> {
        let id = self.send_cmd(serde_json::json!({ "cmd": "lookup", "topic": topic_hex }));
        let reply = self.wait_reply(id);
        assert!(reply["ok"].as_bool().unwrap_or(false), "JS lookup failed: {reply}");
        reply["peers"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    }
}

impl Drop for JsPeer {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(b"{\"cmd\":\"shutdown\",\"id\":0}\n");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[tokio::test]
async fn hyperdht_cross_language_announce_lookup() {
    let _ = tracing_subscriber::fmt::try_init();

    let result = tokio::time::timeout(Duration::from_secs(60), run_interop()).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("interop test failed: {e}"),
        Err(_) => panic!("interop test timed out after 60s"),
    }
}

async fn run_interop() -> Result<(), Box<dyn std::error::Error>> {
    let mut js = JsPeer::spawn();
    let bs_port = js.read_ready_port();
    tracing::info!(bs_port, "JS testnet ready");

    let runtime = UdxRuntime::new()?;

    let config = HyperDhtConfig {
        dht: DhtConfig {
            bootstrap: vec![format!("127.0.0.1:{bs_port}")],
            port: 0,
            host: "127.0.0.1".to_string(),
            firewalled: true,
            ..DhtConfig::default()
        },
        ..HyperDhtConfig::default()
    };

    let (task, handle, _server_rx) = spawn(&runtime, config).await?;

    handle.bootstrapped().await?;
    tracing::info!("Rust HyperDHT bootstrapped");

    // ── Test 1: Node.js announces, Rust looks up ──────────────────────────
    let topic1_raw = hash(b"interop-test-topic-js-announces");
    let topic1_hex = to_hex(&topic1_raw);

    tracing::info!("JS announcing topic {topic1_hex}");
    js.announce(&topic1_hex);
    tracing::info!("JS announce complete");

    tokio::time::sleep(Duration::from_millis(200)).await;

    tracing::info!("Rust looking up topic {topic1_hex}");
    let results = handle.lookup(topic1_raw).await?;
    tracing::info!("Rust lookup returned {} node replies", results.len());

    let found_any = results.iter().any(|r| !r.peers.is_empty());
    assert!(found_any, "Rust lookup found no peers after JS announce");
    tracing::info!("Test 1 passed: Rust found JS-announced peer");

    // ── Test 2: Rust announces, Node.js looks up ──────────────────────────
    let topic2_raw = hash(b"interop-test-topic-rust-announces");
    let topic2_hex = to_hex(&topic2_raw);

    let kp = KeyPair::generate();
    tracing::info!("Rust announcing topic {topic2_hex}");
    handle.announce(topic2_raw, &kp, &[]).await?;
    tracing::info!("Rust announce complete");

    tokio::time::sleep(Duration::from_millis(200)).await;

    tracing::info!("JS looking up topic {topic2_hex}");
    let js_peers = js.lookup(&topic2_hex);
    tracing::info!("JS lookup returned {} peers", js_peers.len());

    assert!(!js_peers.is_empty(), "JS lookup found no peers after Rust announce");
    tracing::info!("Test 2 passed: JS found Rust-announced peer");

    // ── Cleanup ───────────────────────────────────────────────────────────
    handle.destroy().await?;
    let _ = task.await;
    drop(js);
    drop(runtime);

    Ok(())
}
