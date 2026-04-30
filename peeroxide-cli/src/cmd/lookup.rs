use clap::Args;
use futures::stream::{self, StreamExt};
use indexmap::IndexMap;
use libudx::UdxRuntime;
use peeroxide_dht::hyperdht;
use tokio::signal;

use crate::config::ResolvedConfig;
use super::{build_dht_config, parse_topic, to_hex};

#[derive(Args)]
pub struct LookupArgs {
    /// Target topic (64-char hex = raw hash, plaintext = BLAKE2b hashed)
    topic: String,

    /// Also fetch metadata for each discovered peer
    #[arg(long)]
    with_data: bool,

    /// Output as NDJSON
    #[arg(long)]
    json: bool,
}

pub async fn run(args: LookupArgs, cfg: &ResolvedConfig) -> i32 {
    let topic = parse_topic(&args.topic);
    let topic_hex = to_hex(&topic);

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

    let results = tokio::select! {
        r = handle.lookup(topic) => {
            match r {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: lookup failed: {e}");
                    let _ = handle.destroy().await;
                    let _ = task.await;
                    return 1;
                }
            }
        }
        _ = signal::ctrl_c() => {
            let _ = handle.destroy().await;
            let _ = task.await;
            return 130;
        }
    };

    let mut peers: IndexMap<[u8; 32], Vec<String>> = IndexMap::new();
    for result in &results {
        for peer in &result.peers {
            let relay_strs: Vec<String> = peer
                .relay_addresses
                .iter()
                .map(|r| format!("{}:{}", r.host, r.port))
                .collect();

            let entry = peers.entry(peer.public_key).or_default();
            for addr in relay_strs {
                if !entry.contains(&addr) {
                    entry.push(addr);
                }
            }
        }
    }

    if args.with_data {
        let keys: Vec<[u8; 32]> = peers.keys().copied().collect();
        let data_results: Vec<_> = stream::iter(keys.iter().map(|pk| {
            let h = handle.clone();
            let pk = *pk;
            async move {
                let result = h.mutable_get(&pk, 0).await;
                (pk, result)
            }
        }))
        .buffer_unordered(16)
        .collect()
        .await;

        let mut data_map: std::collections::HashMap<[u8; 32], DataResult> =
            std::collections::HashMap::new();
        for (pk, result) in data_results {
            let dr = match result {
                Ok(Some(r)) => DataResult::Ok {
                    value: r.value,
                    seq: r.seq,
                },
                Ok(None) => DataResult::None,
                Err(e) => DataResult::Error(format!("{e}")),
            };
            data_map.insert(pk, dr);
        }

        if args.json {
            print_json_with_data(&peers, &data_map, &topic_hex);
        } else {
            print_human_with_data(&peers, &data_map, &args.topic, &topic_hex);
        }
    } else if args.json {
        print_json(&peers, &topic_hex);
    } else {
        print_human(&peers, &args.topic, &topic_hex);
    }

    let _ = handle.destroy().await;
    let _ = task.await;

    0
}

enum DataResult {
    Ok { value: Vec<u8>, seq: u64 },
    None,
    Error(String),
}

fn format_data_display(value: &[u8]) -> (String, &'static str) {
    match std::str::from_utf8(value) {
        Ok(s) => {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\t', "\\t")
                .replace('\r', "\\r");
            (format!("\"{escaped}\""), "utf8")
        }
        Err(_) => (format!("0x{}", hex::encode(value)), "hex"),
    }
}

fn print_human(peers: &IndexMap<[u8; 32], Vec<String>>, topic_input: &str, topic_hex: &str) {
    if topic_input.len() == 64 && hex::decode(topic_input).is_ok() {
        eprintln!("LOOKUP {topic_hex}");
    } else {
        eprintln!("LOOKUP blake2b(\"{topic_input}\")");
    }
    eprintln!("  found {} peers", peers.len());

    for (pk, relays) in peers {
        eprintln!();
        eprintln!("  @{}", to_hex(pk));
        if relays.is_empty() {
            eprintln!("    relays: (direct only)");
        } else {
            eprintln!("    relays: {}", relays.join(", "));
        }
    }
}

fn print_human_with_data(
    peers: &IndexMap<[u8; 32], Vec<String>>,
    data_map: &std::collections::HashMap<[u8; 32], DataResult>,
    topic_input: &str,
    topic_hex: &str,
) {
    if topic_input.len() == 64 && hex::decode(topic_input).is_ok() {
        eprintln!("LOOKUP {topic_hex}");
    } else {
        eprintln!("LOOKUP blake2b(\"{topic_input}\")");
    }
    eprintln!("  found {} peers", peers.len());

    for (pk, relays) in peers {
        eprintln!();
        eprintln!("  @{}", to_hex(pk));
        if relays.is_empty() {
            eprintln!("    relays: (direct only)");
        } else {
            eprintln!("    relays: {}", relays.join(", "));
        }
        match data_map.get(pk) {
            Some(DataResult::Ok { value, seq }) => {
                let (display, _) = format_data_display(value);
                eprintln!("    data: {display} (seq={seq})");
            }
            Some(DataResult::None) => {
                eprintln!("    data: (not stored)");
            }
            Some(DataResult::Error(e)) => {
                eprintln!("    data: (error: {e})");
            }
            None => {}
        }
    }
}

fn print_json(peers: &IndexMap<[u8; 32], Vec<String>>, topic_hex: &str) {
    for (pk, relays) in peers {
        let obj = serde_json::json!({
            "type": "peer",
            "public_key": to_hex(pk),
            "relay_addresses": relays,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    }
    let summary = serde_json::json!({
        "type": "summary",
        "topic": topic_hex,
        "peers_found": peers.len(),
    });
    println!("{}", serde_json::to_string(&summary).unwrap());
}

fn print_json_with_data(
    peers: &IndexMap<[u8; 32], Vec<String>>,
    data_map: &std::collections::HashMap<[u8; 32], DataResult>,
    topic_hex: &str,
) {
    for (pk, relays) in peers {
        let mut obj = serde_json::json!({
            "type": "peer",
            "public_key": to_hex(pk),
            "relay_addresses": relays,
        });
        if let Some(dr) = data_map.get(pk) {
            match dr {
                DataResult::Ok { value, seq } => {
                    let (display_val, encoding) = match std::str::from_utf8(value) {
                        Ok(s) => (s.to_string(), "utf8"),
                        Err(_) => (hex::encode(value), "hex"),
                    };
                    obj["data_status"] = serde_json::json!("ok");
                    obj["data"] = serde_json::json!(display_val);
                    obj["data_encoding"] = serde_json::json!(encoding);
                    obj["seq"] = serde_json::json!(seq);
                }
                DataResult::None => {
                    obj["data_status"] = serde_json::json!("none");
                    obj["data"] = serde_json::Value::Null;
                    obj["seq"] = serde_json::Value::Null;
                }
                DataResult::Error(e) => {
                    obj["data_status"] = serde_json::json!("error");
                    obj["data"] = serde_json::Value::Null;
                    obj["seq"] = serde_json::Value::Null;
                    obj["error"] = serde_json::json!(e);
                }
            }
        }
        println!("{}", serde_json::to_string(&obj).unwrap());
    }
    let summary = serde_json::json!({
        "type": "summary",
        "topic": topic_hex,
        "peers_found": peers.len(),
    });
    println!("{}", serde_json::to_string(&summary).unwrap());
}
