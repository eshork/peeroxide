use clap::Args;
use libudx::UdxRuntime;
use peeroxide::KeyPair;
use peeroxide_dht::hyperdht::{self, HyperDhtHandle};
use std::time::{Duration, Instant};
use tokio::signal;

use crate::config::ResolvedConfig;
use super::{build_dht_config, parse_topic, to_hex};

const PING_MAGIC: &[u8; 4] = b"PING";
const PONG_MAGIC: &[u8; 4] = b"PONG";
const ECHO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Args)]
pub struct PingArgs {
    /// Target: host:port, @pubkey, 64-char hex topic, or topic name
    target: String,

    /// Number of probes (0 = infinite)
    #[arg(long, default_value_t = 1)]
    count: u64,

    /// Delay between probes in seconds (decimal ok)
    #[arg(long, default_value_t = 1.0)]
    interval: f64,

    /// Attempt full Noise handshake connection
    #[arg(long)]
    connect: bool,

    /// Output as NDJSON
    #[arg(long)]
    json: bool,
}

enum Target {
    HostPort(String, u16),
    PubKey([u8; 32]),
    Topic([u8; 32], String),
}

fn parse_target(input: &str) -> Result<Target, String> {
    if let Some(hex_part) = input.strip_prefix('@') {
        if hex_part.len() != 64 {
            return Err(format!("public key must be 64 hex chars, got {}", hex_part.len()));
        }
        let bytes = hex::decode(hex_part).map_err(|e| format!("invalid hex: {e}"))?;
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&bytes);
        return Ok(Target::PubKey(pk));
    }

    if let Some(colon_pos) = input.rfind(':') {
        let port_str = &input[colon_pos + 1..];
        if port_str.chars().all(|c| c.is_ascii_digit()) && !port_str.is_empty() {
            if let Ok(port) = port_str.parse::<u16>() {
                let host = input[..colon_pos].to_string();
                return Ok(Target::HostPort(host, port));
            }
        }
    }

    if input.len() == 64 && hex::decode(input).is_ok() {
        let bytes = hex::decode(input).unwrap();
        let mut topic = [0u8; 32];
        topic.copy_from_slice(&bytes);
        return Ok(Target::Topic(topic, input.to_string()));
    }

    let topic = parse_topic(input);
    Ok(Target::Topic(topic, input.to_string()))
}

pub async fn run(args: PingArgs, cfg: &ResolvedConfig) -> i32 {
    let target = match parse_target(&args.target) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let dht_config = build_dht_config(cfg);

    let runtime = match UdxRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to create UDP runtime: {e}");
            return 1;
        }
    };

    let (task, handle, _rx) = match hyperdht::spawn(&runtime, dht_config).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: failed to start DHT: {e}");
            return 1;
        }
    };

    if let Err(e) = handle.bootstrapped().await {
        eprintln!("error: bootstrap failed: {e}");
        return 1;
    }

    let exit_code = match target {
        Target::HostPort(host, port) => {
            run_direct_ping(&handle, &args, &host, port).await
        }
        Target::PubKey(pk) => {
            run_pubkey_ping(&handle, &args, &pk, &runtime).await
        }
        Target::Topic(topic, name) => {
            run_topic_ping(&handle, &args, &topic, &name, &runtime).await
        }
    };

    let _ = handle.destroy().await;
    let _ = task.await;
    drop(runtime);

    // SIGINT produces 130 per spec; inner functions return SIGINT_EXIT when interrupted
    exit_code
}

const SIGINT_EXIT: i32 = 130;

async fn run_direct_ping(handle: &HyperDhtHandle, args: &PingArgs, host: &str, port: u16) -> i32 {
    if args.json {
        let obj = serde_json::json!({
            "type": "resolve",
            "method": "direct",
            "target": format!("{host}:{port}"),
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        eprintln!("PING {host}:{port} (direct)");
    }

    let interval = Duration::from_secs_f64(args.interval);
    let mut sent: u64 = 0;
    let mut responded: u64 = 0;
    let mut timed_out: u64 = 0;
    let mut rtts: Vec<f64> = Vec::new();
    let mut interrupted = false;

    let count = if args.count == 0 { u64::MAX } else { args.count };

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let cancel_handle = tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(());
    });

    for i in 1..=count {
        sent += 1;
        let dht = handle.dht();

        match dht.ping(host, port).await {
            Ok(resp) => {
                responded += 1;
                let rtt_ms = resp.rtt.as_secs_f64() * 1000.0;
                rtts.push(rtt_ms);

                if args.json {
                    let obj = serde_json::json!({
                        "type": "probe",
                        "seq": i,
                        "status": "ok",
                        "rtt_ms": rtt_ms,
                        "node_id": resp.id.map(|id| to_hex(&id)),
                    });
                    println!("{}", serde_json::to_string(&obj).unwrap());
                } else if i == 1 && args.count == 1 {
                    let node_str = resp.id.map(|id| format!("  node_id={}", to_hex(&id))).unwrap_or_default();
                    eprintln!("  [{i}] OK  {rtt_ms:.0}ms{node_str}");
                } else {
                    eprintln!("  [{i}] OK  {rtt_ms:.0}ms");
                }
            }
            Err(_) => {
                timed_out += 1;
                if args.json {
                    let obj = serde_json::json!({
                        "type": "probe",
                        "seq": i,
                        "status": "timeout",
                    });
                    println!("{}", serde_json::to_string(&obj).unwrap());
                } else {
                    eprintln!("  [{i}] TIMEOUT");
                }
            }
        }

        if i < count {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = &mut cancel_rx => { interrupted = true; break; }
            }
        }

        if cancel_rx.is_terminated() {
            interrupted = true;
            break;
        }
    }

    cancel_handle.abort();

    if sent > 1 || args.count == 0 {
        print_udp_summary(args, host, port, sent, responded, timed_out, &rtts);
    }

    if interrupted {
        SIGINT_EXIT
    } else if timed_out > 0 {
        1
    } else {
        0
    }
}

fn print_udp_summary(
    args: &PingArgs,
    host: &str,
    port: u16,
    sent: u64,
    responded: u64,
    timed_out: u64,
    rtts: &[f64],
) {
    if args.json {
        let summary = serde_json::json!({
            "type": "summary",
            "target": format!("{host}:{port}"),
            "probes_sent": sent,
            "probes_responded": responded,
            "probes_timed_out": timed_out,
            "rtt_min_ms": rtts.iter().copied().reduce(f64::min),
            "rtt_avg_ms": if rtts.is_empty() { None } else { Some(rtts.iter().sum::<f64>() / rtts.len() as f64) },
            "rtt_max_ms": rtts.iter().copied().reduce(f64::max),
        });
        println!("{}", serde_json::to_string(&summary).unwrap());
    } else {
        let loss_pct = if sent > 0 {
            (timed_out as f64 / sent as f64) * 100.0
        } else {
            0.0
        };
        eprintln!("--- {host}:{port} ping statistics ---");
        eprintln!("{sent} probes, {responded} responded, {timed_out} timed out ({loss_pct:.0}% probe loss)");
        if !rtts.is_empty() {
            let min = rtts.iter().copied().reduce(f64::min).unwrap();
            let max = rtts.iter().copied().reduce(f64::max).unwrap();
            let avg = rtts.iter().sum::<f64>() / rtts.len() as f64;
            eprintln!("rtt min/avg/max = {min:.1}/{avg:.1}/{max:.1} ms");
        }
    }
}

async fn run_pubkey_ping(handle: &HyperDhtHandle, args: &PingArgs, pk: &[u8; 32], runtime: &UdxRuntime) -> i32 {
    let pk_hex = to_hex(pk);

    if args.json {
        let obj = serde_json::json!({
            "type": "resolve",
            "method": "find_peer",
            "public_key": pk_hex,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        eprintln!("RESOLVE find_peer({pk_hex})");
    }

    let peer = match handle.find_peer(*pk).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            if args.json {
                let obj = serde_json::json!({
                    "type": "resolve",
                    "status": "not_found",
                    "error": "Peer not found on DHT",
                });
                println!("{}", serde_json::to_string(&obj).unwrap());
            }
            eprintln!("error: Peer not found on DHT — no nodes have a record for this public key.");
            return 1;
        }
        Err(e) => {
            eprintln!("error: find_peer failed: {e}");
            return 1;
        }
    };

    if peer.relay_addresses.is_empty() {
        eprintln!("error: Peer {pk_hex} has no advertised addresses — cannot ping.");
        if args.connect {
            return run_connect(handle, args, pk, &peer.relay_addresses, runtime).await;
        }
        return 1;
    }

    if args.json {
        let obj = serde_json::json!({
            "type": "resolve",
            "status": "found",
            "addresses": peer.relay_addresses.len(),
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        eprintln!("  found, {} relay addresses", peer.relay_addresses.len());
    }

    let mut any_success = false;
    let mut any_failure = false;
    let udp_count = if args.connect { 1 } else if args.count == 0 { u64::MAX } else { args.count };
    let interval = Duration::from_secs_f64(args.interval);
    let mut interrupted = false;

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let cancel_handle = tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(());
    });

    'outer: for addr in &peer.relay_addresses {
        let host = &addr.host;
        let port = addr.port;
        if !args.json {
            eprintln!("PING {host}:{port}");
        }

        for seq in 1..=udp_count {
            let dht = handle.dht();
            match dht.ping(host, port).await {
                Ok(resp) => {
                    any_success = true;
                    let rtt_ms = resp.rtt.as_secs_f64() * 1000.0;
                    if args.json {
                        let obj = serde_json::json!({
                            "type": "probe",
                            "address": format!("{host}:{port}"),
                            "seq": seq,
                            "status": "ok",
                            "rtt_ms": rtt_ms,
                        });
                        println!("{}", serde_json::to_string(&obj).unwrap());
                    } else {
                        eprintln!("  [{seq}] OK  {rtt_ms:.0}ms");
                    }
                }
                Err(_) => {
                    any_failure = true;
                    if args.json {
                        let obj = serde_json::json!({
                            "type": "probe",
                            "address": format!("{host}:{port}"),
                            "seq": seq,
                            "status": "timeout",
                        });
                        println!("{}", serde_json::to_string(&obj).unwrap());
                    } else {
                        eprintln!("  [{seq}] TIMEOUT");
                    }
                }
            }

            if seq < udp_count {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = &mut cancel_rx => { interrupted = true; break 'outer; }
                }
            }

            if cancel_rx.is_terminated() {
                interrupted = true;
                break 'outer;
            }
        }
    }

    cancel_handle.abort();

    if args.connect {
        return run_connect(handle, args, pk, &peer.relay_addresses, runtime).await;
    }

    if interrupted {
        SIGINT_EXIT
    } else if !any_success {
        eprintln!("All resolved addresses unreachable — target may be offline or firewalled.");
        1
    } else if any_failure {
        1
    } else {
        0
    }
}

async fn run_topic_ping(
    handle: &HyperDhtHandle,
    args: &PingArgs,
    topic: &[u8; 32],
    name: &str,
    runtime: &UdxRuntime,
) -> i32 {
    let topic_hex = to_hex(topic);
    let display = if name.len() == 64 && hex::decode(name).is_ok() {
        topic_hex.clone()
    } else {
        format!("blake2b(\"{name}\")")
    };

    if args.json {
        let obj = serde_json::json!({
            "type": "resolve",
            "method": "lookup",
            "topic": topic_hex,
            "display": display,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        eprintln!("RESOLVE lookup({display})");
    }

    let results = match handle.lookup(*topic).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: lookup failed: {e}");
            return 1;
        }
    };

    let mut peers: Vec<([u8; 32], Vec<peeroxide_dht::messages::Ipv4Peer>)> = Vec::new();
    let mut seen_keys: std::collections::HashMap<[u8; 32], usize> = std::collections::HashMap::new();

    for result in &results {
        for peer in &result.peers {
            if let Some(&idx) = seen_keys.get(&peer.public_key) {
                // Union relay addresses for duplicate pubkeys
                let existing = &mut peers[idx].1;
                for addr in &peer.relay_addresses {
                    if !existing.iter().any(|a| a.host == addr.host && a.port == addr.port) {
                        existing.push(addr.clone());
                    }
                }
            } else {
                seen_keys.insert(peer.public_key, peers.len());
                peers.push((peer.public_key, peer.relay_addresses.clone()));
            }
        }
    }

    if peers.is_empty() {
        if args.json {
            let obj = serde_json::json!({
                "type": "resolve",
                "status": "not_found",
                "error": "No peers announcing on this topic",
            });
            println!("{}", serde_json::to_string(&obj).unwrap());
        }
        eprintln!("error: No peers announcing on this topic.");
        return 1;
    }

    let display_count = peers.len().min(20);
    if args.json {
        let obj = serde_json::json!({
            "type": "resolve",
            "status": "found",
            "peers": peers.len(),
            "using": display_count,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else if peers.len() > 20 {
        eprintln!("  {} peers found (showing 20 of {})", display_count, peers.len());
    } else {
        eprintln!("  {} peer(s) found", peers.len());
    }

    let peers_to_ping = &peers[..display_count];

    let mut any_success = false;
    let mut any_failure = false;
    let udp_count = if args.connect { 1 } else if args.count == 0 { u64::MAX } else { args.count };
    let interval = Duration::from_secs_f64(args.interval);
    let mut interrupted = false;

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let cancel_handle = tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(());
    });

    'outer: for (pk, relay_addrs) in peers_to_ping {
        for addr in relay_addrs {
            let host = &addr.host;
            let port = addr.port;
            if !args.json {
                eprintln!("PING {host}:{port}");
            }

            for seq in 1..=udp_count {
                let dht = handle.dht();
                match dht.ping(host, port).await {
                    Ok(resp) => {
                        any_success = true;
                        let rtt_ms = resp.rtt.as_secs_f64() * 1000.0;
                        if args.json {
                            let obj = serde_json::json!({
                                "type": "probe",
                                "peer": to_hex(pk),
                                "address": format!("{host}:{port}"),
                                "seq": seq,
                                "status": "ok",
                                "rtt_ms": rtt_ms,
                            });
                            println!("{}", serde_json::to_string(&obj).unwrap());
                        } else {
                            eprintln!("  [{seq}] OK  {rtt_ms:.0}ms");
                        }
                    }
                    Err(_) => {
                        any_failure = true;
                        if args.json {
                            let obj = serde_json::json!({
                                "type": "probe",
                                "peer": to_hex(pk),
                                "address": format!("{host}:{port}"),
                                "seq": seq,
                                "status": "timeout",
                            });
                            println!("{}", serde_json::to_string(&obj).unwrap());
                        } else {
                            eprintln!("  [{seq}] TIMEOUT — direct UDP unreachable (firewalled)");
                        }
                    }
                }

                if seq < udp_count {
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {}
                        _ = &mut cancel_rx => { interrupted = true; break 'outer; }
                    }
                }

                if cancel_rx.is_terminated() {
                    interrupted = true;
                    break 'outer;
                }
            }
        }

        if args.connect {
            let code = run_connect(handle, args, pk, relay_addrs, runtime).await;
            if code == 0 {
                return 0;
            }
            continue;
        }
    }

    cancel_handle.abort();

    if args.connect {
        eprintln!("All peers failed connection.");
        return 1;
    }

    if interrupted {
        SIGINT_EXIT
    } else if !any_success {
        eprintln!("All resolved addresses unreachable — target may be offline or firewalled.");
        1
    } else if any_failure {
        1
    } else {
        0
    }
}

async fn run_connect(
    handle: &HyperDhtHandle,
    args: &PingArgs,
    remote_pk: &[u8; 32],
    relay_addresses: &[peeroxide_dht::messages::Ipv4Peer],
    runtime: &UdxRuntime,
) -> i32 {
    let pk_hex = to_hex(remote_pk);
    let short_pk = &pk_hex[..8];

    if !args.json {
        eprintln!("CONNECT {short_pk}...");
    }

    let local_kp = KeyPair::generate();
    let start = Instant::now();

    let mut peer_conn = match handle.connect_with_nodes(&local_kp, *remote_pk, relay_addresses, runtime).await {
        Ok(conn) => conn,
        Err(e) => {
            let err_msg = format!("{e}");
            if args.json {
                let obj = serde_json::json!({
                    "type": "connect",
                    "status": "error",
                    "error": err_msg,
                });
                println!("{}", serde_json::to_string(&obj).unwrap());
            } else {
                eprintln!("  FAILED: {err_msg}");
            }
            return 1;
        }
    };

    let connect_time = start.elapsed();
    if args.json {
        let obj = serde_json::json!({
            "type": "connect",
            "status": "ok",
            "time_ms": connect_time.as_secs_f64() * 1000.0,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        eprintln!("  OK ({:.0}ms)", connect_time.as_secs_f64() * 1000.0);
    }

    if !args.json {
        eprintln!("ECHO {short_pk}...");
    }

    if peer_conn.stream.write(PING_MAGIC).await.is_err() {
        eprintln!("  error: failed to send PING magic");
        return 1;
    }

    let pong = tokio::time::timeout(ECHO_TIMEOUT, peer_conn.stream.read()).await;
    match pong {
        Ok(Ok(Some(msg))) if msg == PONG_MAGIC => {}
        _ => {
            eprintln!("  Remote peer does not support echo protocol (not running announce --ping?)");
            return 1;
        }
    }

    let count = if args.count == 0 { u64::MAX } else { args.count };
    let interval = Duration::from_secs_f64(args.interval);
    let mut sent: u64 = 0;
    let mut responded: u64 = 0;
    let mut latencies: Vec<f64> = Vec::new();
    let mut interrupted = false;
    let echo_start = Instant::now();

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let cancel_handle = tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(());
    });

    for i in 1..=count {
        sent += 1;
        let seq_bytes = i.to_le_bytes();
        let send_time = Instant::now();
        let ts_nanos = send_time.duration_since(echo_start).as_nanos() as u64;
        let mut probe = [0u8; 16];
        probe[..8].copy_from_slice(&seq_bytes);
        probe[8..].copy_from_slice(&ts_nanos.to_le_bytes());

        if peer_conn.stream.write(&probe).await.is_err() {
            break;
        }

        let reply = tokio::time::timeout(ECHO_TIMEOUT, peer_conn.stream.read()).await;
        match reply {
            Ok(Ok(Some(data))) if data.len() == 16 => {
                let latency_ms = send_time.elapsed().as_secs_f64() * 1000.0;
                responded += 1;
                latencies.push(latency_ms);

                if args.json {
                    let obj = serde_json::json!({
                        "type": "probe",
                        "seq": i,
                        "status": "ok",
                        "latency_ms": latency_ms,
                    });
                    println!("{}", serde_json::to_string(&obj).unwrap());
                } else {
                    eprintln!("  [{i}] OK  {latency_ms:.0}ms (e2e encrypted)");
                }
            }
            _ => {
                if args.json {
                    let obj = serde_json::json!({
                        "type": "probe",
                        "seq": i,
                        "status": "timeout",
                    });
                    println!("{}", serde_json::to_string(&obj).unwrap());
                }
                break;
            }
        }

        if i < count {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = &mut cancel_rx => { interrupted = true; break; }
            }
        }

        if cancel_rx.is_terminated() {
            interrupted = true;
            break;
        }
    }

    cancel_handle.abort();

    if sent > 1 || args.count == 0 {
        if args.json {
            let summary = serde_json::json!({
                "type": "summary",
                "probes_sent": sent,
                "probes_responded": responded,
                "latency_min_ms": latencies.iter().copied().reduce(f64::min),
                "latency_avg_ms": if latencies.is_empty() { None } else { Some(latencies.iter().sum::<f64>() / latencies.len() as f64) },
                "latency_max_ms": latencies.iter().copied().reduce(f64::max),
            });
            println!("{}", serde_json::to_string(&summary).unwrap());
        } else {
            eprintln!("--- {short_pk}... echo statistics ---");
            eprintln!("{sent} probes sent, {responded} responded, {} timed out", sent - responded);
            if !latencies.is_empty() {
                let min = latencies.iter().copied().reduce(f64::min).unwrap();
                let max = latencies.iter().copied().reduce(f64::max).unwrap();
                let avg = latencies.iter().sum::<f64>() / latencies.len() as f64;
                eprintln!("latency min/avg/max = {min:.1}/{avg:.1}/{max:.1} ms");
            }
        }
    }

    if interrupted {
        SIGINT_EXIT
    } else if responded < sent {
        1
    } else {
        0
    }
}
