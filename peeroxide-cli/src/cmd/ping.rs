use clap::Args;
use libudx::UdxRuntime;
use peeroxide::KeyPair;
use peeroxide_dht::hyperdht::{self, HyperDhtHandle};
use peeroxide_dht::messages::Ipv4Peer;
use std::time::{Duration, Instant};
use tokio::signal;

use crate::config::ResolvedConfig;
use super::{build_dht_config, parse_topic, to_hex};

const PING_MAGIC: &[u8; 4] = b"PING";
const PONG_MAGIC: &[u8; 4] = b"PONG";
const ECHO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Args)]
pub struct PingArgs {
    /// Target: host:port, @pubkey, 64-char hex topic, or topic name.
    /// If omitted, pings all configured bootstrap nodes.
    target: Option<String>,

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
    let dht_config = build_dht_config(cfg);
    let bootstrap_addrs = dht_config.dht.bootstrap.clone();

    let target = match &args.target {
        Some(t) => {
            match parse_target(t) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            }
        }
        None => {
            if bootstrap_addrs.is_empty() {
                eprintln!("error: no target specified and no bootstrap nodes configured.");
                eprintln!("       Use --public for default network, or specify bootstrap nodes in config.");
                return 1;
            }
            None
        }
    };

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
        None => {
            run_bootstrap_check(&handle, &args, &bootstrap_addrs).await
        }
        Some(Target::HostPort(host, port)) => {
            run_direct_ping(&handle, &args, &host, port).await
        }
        Some(Target::PubKey(pk)) => {
            run_pubkey_ping(&handle, &args, &pk, &runtime).await
        }
        Some(Target::Topic(topic, name)) => {
            run_topic_ping(&handle, &args, &topic, &name, &runtime).await
        }
    };

    let _ = handle.destroy().await;
    let _ = task.await;
    drop(runtime);

    exit_code
}

const SIGINT_EXIT: i32 = 130;

#[derive(Debug, Clone, Copy)]
enum NatType {
    Open,
    Consistent,
    Random,
    MultiHomed,
    Unknown,
}

impl NatType {
    fn classify(reflexive_addrs: &[Ipv4Peer], local_port: Option<u16>) -> (Self, Option<String>, Option<u16>) {
        if reflexive_addrs.is_empty() {
            return (Self::Unknown, None, None);
        }

        let hosts: Vec<&str> = reflexive_addrs.iter().map(|a| a.host.as_str()).collect();
        let ports: Vec<u16> = reflexive_addrs.iter().map(|a| a.port).collect();

        let all_same_host = hosts.windows(2).all(|w| w[0] == w[1]);
        let all_same_port = ports.windows(2).all(|w| w[0] == w[1]);

        if !all_same_host {
            if reflexive_addrs.len() >= 2 {
                return (Self::MultiHomed, None, None);
            }
            return (Self::Unknown, None, None);
        }

        let host = Some(hosts[0].to_string());

        if all_same_port {
            let port = Some(ports[0]);
            // Open requires: same port as local AND reflexive IP is a local interface address.
            // Port-preserving NATs pass the port check but fail the IP check.
            let nat_type = match local_port {
                Some(lp) if lp == ports[0] && is_local_address(hosts[0]) => Self::Open,
                _ => Self::Consistent,
            };
            (nat_type, host, port)
        } else {
            (Self::Random, host, None)
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Open => "open (directly reachable)",
            Self::Consistent => "consistent (hole-punchable)",
            Self::Random => "random (not directly hole-punchable)",
            Self::MultiHomed => "multi-homed (direct connections unreliable — relay required)",
            Self::Unknown => "unknown",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Consistent => "consistent",
            Self::Random => "random",
            Self::MultiHomed => "multi-homed",
            Self::Unknown => "unknown",
        }
    }
}

fn is_local_address(addr: &str) -> bool {
    use std::net::{IpAddr, SocketAddr, UdpSocket};
    let ip: IpAddr = match addr.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    let bind_addr = SocketAddr::new(ip, 0);
    UdpSocket::bind(bind_addr).is_ok()
}

fn parse_host_port(addr: &str) -> Option<(&str, u16)> {
    let colon = addr.rfind(':')?;
    let port_str = &addr[colon + 1..];
    let port: u16 = port_str.parse().ok()?;
    let host_part = &addr[..colon];
    // Handle "suggestedIP@hostname:port" format — extract IP before '@'
    let host = if let Some(at_pos) = host_part.find('@') {
        &host_part[..at_pos]
    } else {
        host_part
    };
    Some((host, port))
}

async fn run_bootstrap_check(handle: &HyperDhtHandle, args: &PingArgs, bootstrap_addrs: &[String]) -> i32 {
    let node_count = bootstrap_addrs.len();

    if args.json {
        let obj = serde_json::json!({
            "type": "bootstrap_check",
            "nodes": node_count,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        eprintln!("BOOTSTRAP CHECK ({node_count} node{})", if node_count == 1 { "" } else { "s" });
    }

    let interval = Duration::from_secs_f64(args.interval);
    let count = if args.count == 0 { u64::MAX } else { args.count };
    let mut reachable: u64 = 0;
    let mut unreachable: u64 = 0;
    let mut reflexive_addrs: Vec<Ipv4Peer> = Vec::new();
    let mut all_closer_nodes: Vec<Ipv4Peer> = Vec::new();
    let mut interrupted = false;

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let cancel_handle = tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(());
    });

    'outer: for addr_str in bootstrap_addrs {
        let (host, port) = match parse_host_port(addr_str) {
            Some(hp) => hp,
            None => {
                if args.json {
                    let obj = serde_json::json!({
                        "type": "bootstrap_probe",
                        "node": addr_str,
                        "status": "invalid",
                        "error": "could not parse address",
                    });
                    println!("{}", serde_json::to_string(&obj).unwrap());
                } else {
                    eprintln!("  {addr_str}  INVALID (could not parse)");
                }
                unreachable += 1;
                continue;
            }
        };

        for seq in 1..=count {
            let dht = handle.dht();
            match dht.ping(host, port).await {
                Ok(resp) => {
                    reachable += 1;
                    let rtt_ms = resp.rtt.as_secs_f64() * 1000.0;
                    let nodes_offered = resp.closer_nodes.len();
                    all_closer_nodes.extend(resp.closer_nodes);

                    if let Some(ref reflexive) = resp.to {
                        reflexive_addrs.push(reflexive.clone());
                    }

                    if args.json {
                        let obj = serde_json::json!({
                            "type": "bootstrap_probe",
                            "node": addr_str,
                            "seq": seq,
                            "status": "ok",
                            "rtt_ms": rtt_ms,
                            "node_id": resp.id.map(|id| to_hex(&id)),
                            "reflexive_addr": resp.to.as_ref().map(|a| format!("{}:{}", a.host, a.port)),
                            "closer_nodes": nodes_offered,
                        });
                        println!("{}", serde_json::to_string(&obj).unwrap());
                    } else if count == 1 {
                        let node_str = resp.id.map(|id| format!("  node_id={}", to_hex(&id))).unwrap_or_default();
                        eprintln!("  {addr_str}  OK  {rtt_ms:.0}ms  ({nodes_offered} nodes){node_str}");
                    } else {
                        eprintln!("  {addr_str}  [{seq}] OK  {rtt_ms:.0}ms  ({nodes_offered} nodes)");
                    }
                }
                Err(_) => {
                    unreachable += 1;
                    if args.json {
                        let obj = serde_json::json!({
                            "type": "bootstrap_probe",
                            "node": addr_str,
                            "seq": seq,
                            "status": "timeout",
                        });
                        println!("{}", serde_json::to_string(&obj).unwrap());
                    } else if count == 1 {
                        eprintln!("  {addr_str}  TIMEOUT");
                    } else {
                        eprintln!("  {addr_str}  [{seq}] TIMEOUT");
                    }
                }
            }

            if seq < count && count > 1 {
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

    let local_port = handle.dht().local_port().await.ok();
    let (nat_type, public_host, public_port) = NatType::classify(&reflexive_addrs, local_port);

    all_closer_nodes.sort_by(|a, b| (&a.host, a.port).cmp(&(&b.host, b.port)));
    all_closer_nodes.dedup_by(|a, b| a.host == b.host && a.port == b.port);
    let total_unique_nodes = all_closer_nodes.len();

    if args.json {
        let mut summary = serde_json::json!({
            "type": "bootstrap_summary",
            "nodes": node_count,
            "reachable": reachable,
            "unreachable": unreachable,
            "nat_type": nat_type.short_label(),
            "closer_nodes_total": total_unique_nodes,
        });
        if let Some(ref host) = public_host {
            summary["public_host"] = serde_json::Value::String(host.clone());
        }
        if let Some(port) = public_port {
            summary["public_port"] = serde_json::Value::Number(port.into());
            summary["port_consistent"] = serde_json::Value::Bool(true);
        } else if public_host.is_some() {
            summary["port_consistent"] = serde_json::Value::Bool(false);
            let ports: Vec<u16> = reflexive_addrs.iter().map(|a| a.port).collect();
            summary["observed_ports"] = serde_json::json!(ports);
        }
        if matches!(nat_type, NatType::MultiHomed) {
            let mut unique_hosts: Vec<&str> = reflexive_addrs.iter().map(|a| a.host.as_str()).collect();
            unique_hosts.dedup();
            summary["observed_hosts"] = serde_json::json!(unique_hosts);
        }
        println!("{}", serde_json::to_string(&summary).unwrap());
    } else {
        eprintln!("--- bootstrap summary ---");
        eprintln!("{node_count} node{}, {reachable} reachable, {unreachable} unreachable",
            if node_count == 1 { "" } else { "s" });
        eprintln!("{total_unique_nodes} unique peer{} discovered via routing tables",
            if total_unique_nodes == 1 { "" } else { "s" });

        match nat_type {
            NatType::MultiHomed => {
                let mut unique_hosts: Vec<&str> = reflexive_addrs.iter().map(|a| a.host.as_str()).collect();
                unique_hosts.sort();
                unique_hosts.dedup();
                eprintln!("public address: multiple ({})", unique_hosts.join(", "));
            }
            _ => {
                if let Some(ref host) = public_host {
                    if let Some(port) = public_port {
                        let sample_count = reflexive_addrs.len();
                        eprintln!("public address: {host}:{port} (consistent across {sample_count} node{})",
                            if sample_count == 1 { "" } else { "s" });
                    } else {
                        let ports: Vec<String> = reflexive_addrs.iter().map(|a| a.port.to_string()).collect();
                        eprintln!("public address: {host} (port varies: {})", ports.join(", "));
                    }
                }
            }
        }

        if reachable >= 2 {
            eprintln!("NAT type: {}", nat_type.label());
        } else if reachable == 1 {
            eprintln!("NAT type: insufficient samples (need 2+ nodes)");
        }
    }

    if interrupted {
        SIGINT_EXIT
    } else if unreachable > 0 {
        1
    } else {
        0
    }
}

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

#[cfg(test)]
mod tests {
    use super::{NatType, Ipv4Peer};

    fn peer(host: &str, port: u16) -> Ipv4Peer {
        Ipv4Peer { host: host.to_string(), port }
    }

    #[test]
    fn classify_open() {
        let addrs = [
            peer("127.0.0.1", 5000),
            peer("127.0.0.1", 5000),
            peer("127.0.0.1", 5000),
        ];

        let (nat, host, port) = NatType::classify(&addrs, Some(5000));

        assert!(matches!(nat, NatType::Open));
        assert_eq!(host.as_deref(), Some("127.0.0.1"));
        assert_eq!(port, Some(5000));
    }

    #[test]
    fn classify_consistent_port_preserving() {
        let addrs = [
            peer("93.184.216.34", 5000),
            peer("93.184.216.34", 5000),
            peer("93.184.216.34", 5000),
        ];

        let (nat, host, port) = NatType::classify(&addrs, Some(5000));

        assert!(matches!(nat, NatType::Consistent));
        assert_eq!(host.as_deref(), Some("93.184.216.34"));
        assert_eq!(port, Some(5000));
    }

    #[test]
    fn classify_consistent_port_remapping() {
        let addrs = [peer("73.42.18.201", 51234), peer("73.42.18.201", 51234)];

        let (nat, host, port) = NatType::classify(&addrs, Some(12345));

        assert!(matches!(nat, NatType::Consistent));
        assert_eq!(host.as_deref(), Some("73.42.18.201"));
        assert_eq!(port, Some(51234));
    }

    #[test]
    fn classify_random() {
        let addrs = [
            peer("73.42.18.201", 51234),
            peer("73.42.18.201", 52001),
            peer("73.42.18.201", 49888),
        ];

        let (nat, host, port) = NatType::classify(&addrs, Some(12345));

        assert!(matches!(nat, NatType::Random));
        assert_eq!(host.as_deref(), Some("73.42.18.201"));
        assert_eq!(port, None);
    }

    #[test]
    fn classify_unknown_no_responses() {
        let addrs: [Ipv4Peer; 0] = [];

        let (nat, host, port) = NatType::classify(&addrs, Some(12345));

        assert!(matches!(nat, NatType::Unknown));
        assert_eq!(host, None);
        assert_eq!(port, None);
    }

    #[test]
    fn classify_multi_homed() {
        let addrs = [peer("1.2.3.4", 5000), peer("5.6.7.8", 5000)];

        let (nat, host, port) = NatType::classify(&addrs, Some(5000));

        assert!(matches!(nat, NatType::MultiHomed));
        assert_eq!(host, None);
        assert_eq!(port, None);
    }

    #[test]
    fn classify_unknown_single_sample_different_ip() {
        let addrs = [peer("1.2.3.4", 5000)];

        let (nat, host, port) = NatType::classify(&addrs, Some(9999));

        assert!(matches!(nat, NatType::Consistent));
        assert_eq!(host.as_deref(), Some("1.2.3.4"));
        assert_eq!(port, Some(5000));
    }
}
