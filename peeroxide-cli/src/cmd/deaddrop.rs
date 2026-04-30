use clap::{Args, Subcommand};
use libudx::UdxRuntime;
use peeroxide::KeyPair;
use peeroxide_dht::hyperdht::{self, HyperDhtHandle, MutablePutResult};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::signal;
use tokio::sync::{Mutex, Semaphore};

use crate::config::ResolvedConfig;
use super::{build_dht_config, to_hex};

const MAX_CHUNKS: usize = 65535;
const ROOT_HEADER_SIZE: usize = 39;
const NON_ROOT_HEADER_SIZE: usize = 33;
const MAX_PAYLOAD: usize = 1000;
const ROOT_PAYLOAD_MAX: usize = MAX_PAYLOAD - ROOT_HEADER_SIZE;
const NON_ROOT_PAYLOAD_MAX: usize = MAX_PAYLOAD - NON_ROOT_HEADER_SIZE;
const VERSION: u8 = 0x01;

#[derive(Subcommand)]
pub enum DeaddropCommands {
    /// Store data on DHT, print pickup key
    Leave(LeaveArgs),
    /// Retrieve data from DHT using pickup key
    Pickup(PickupArgs),
}

#[derive(Args)]
pub struct LeaveArgs {
    /// File path or - for stdin
    file: String,

    /// Hard cap on outbound byte rate (e.g. 100k, 1m)
    #[arg(long)]
    max_speed: Option<String>,

    /// Refresh interval in seconds (default: 600)
    #[arg(long, default_value_t = 600)]
    refresh_interval: u64,

    /// Stop refreshing after this duration
    #[arg(long)]
    ttl: Option<u64>,

    /// Exit after N pickups detected
    #[arg(long)]
    max_pickups: Option<u64>,

    /// Derive keypair from passphrase (provided on command line)
    #[arg(long, conflicts_with = "interactive_passphrase")]
    passphrase: Option<String>,

    /// Derive keypair from passphrase (prompted interactively, hidden input)
    #[arg(long, conflicts_with = "passphrase")]
    interactive_passphrase: bool,
}

#[derive(Args)]
pub struct PickupArgs {
    /// Pickup key (64-char hex or passphrase text)
    #[arg(required_unless_present_any = ["passphrase", "interactive_passphrase"])]
    key: Option<String>,

    /// Derive pickup key from passphrase (provided on command line)
    #[arg(long, conflicts_with = "interactive_passphrase")]
    passphrase: Option<String>,

    /// Derive pickup key from passphrase (prompted interactively, hidden input)
    #[arg(long, conflicts_with = "passphrase")]
    interactive_passphrase: bool,

    /// Write output to file (default: stdout)
    #[arg(long)]
    output: Option<String>,

    /// Give up on any single chunk after this duration (default: 1200s)
    #[arg(long, default_value_t = 1200)]
    timeout: u64,

    /// Don't announce pickup acknowledgement
    #[arg(long)]
    no_ack: bool,
}

pub async fn run(cmd: DeaddropCommands, cfg: &ResolvedConfig) -> i32 {
    match cmd {
        DeaddropCommands::Leave(args) => run_leave(args, cfg).await,
        DeaddropCommands::Pickup(args) => run_pickup(args, cfg).await,
    }
}

fn derive_chunk_keypair(root_seed: &[u8; 32], chunk_index: u16) -> KeyPair {
    let mut input = Vec::with_capacity(34);
    input.extend_from_slice(root_seed);
    input.extend_from_slice(&chunk_index.to_le_bytes());
    let hash = peeroxide::discovery_key(&input);
    KeyPair::from_seed(hash)
}

fn compute_crc32c(data: &[u8]) -> u32 {
    crc32c::crc32c(data)
}

fn encode_root_chunk(total_chunks: u16, crc: u32, next_pk: &[u8; 32], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ROOT_HEADER_SIZE + payload.len());
    buf.push(VERSION);
    buf.extend_from_slice(&total_chunks.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(next_pk);
    buf.extend_from_slice(payload);
    buf
}

fn encode_non_root_chunk(next_pk: &[u8; 32], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(NON_ROOT_HEADER_SIZE + payload.len());
    buf.push(VERSION);
    buf.extend_from_slice(next_pk);
    buf.extend_from_slice(payload);
    buf
}

fn parse_max_speed(s: &str) -> Result<u64, String> {
    let s = s.trim().to_lowercase();
    if let Some(num) = s.strip_suffix('m') {
        num.parse::<u64>()
            .map(|n| n * 1_000_000)
            .map_err(|e| format!("invalid --max-speed: {e}"))
    } else if let Some(num) = s.strip_suffix('k') {
        num.parse::<u64>()
            .map(|n| n * 1_000)
            .map_err(|e| format!("invalid --max-speed: {e}"))
    } else {
        s.parse::<u64>()
            .map_err(|e| format!("invalid --max-speed: {e}"))
    }
}

async fn run_leave(args: LeaveArgs, cfg: &ResolvedConfig) -> i32 {
    if args.refresh_interval == 0 {
        eprintln!("error: --refresh-interval must be greater than 0");
        return 1;
    }
    if args.ttl == Some(0) {
        eprintln!("error: --ttl must be greater than 0");
        return 1;
    }
    if args.max_pickups == Some(0) {
        eprintln!("error: --max-pickups must be greater than 0");
        return 1;
    }

    let data = if args.file == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
            eprintln!("error: failed to read stdin: {e}");
            return 1;
        }
        buf
    } else {
        match std::fs::read(&args.file) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: failed to read file: {e}");
                return 1;
            }
        }
    };

    let total_chunks = compute_chunk_count(data.len());
    if total_chunks > MAX_CHUNKS {
        eprintln!("error: file too large ({} chunks exceeds max {})", total_chunks, MAX_CHUNKS);
        return 1;
    }

    let root_seed: [u8; 32] = if let Some(ref phrase) = args.passphrase {
        if phrase.is_empty() {
            eprintln!("error: passphrase cannot be empty");
            return 1;
        }
        peeroxide::discovery_key(phrase.as_bytes())
    } else if args.interactive_passphrase {
        eprintln!("Enter passphrase: ");
        let passphrase = rpassword_read();
        if passphrase.is_empty() {
            eprintln!("error: passphrase cannot be empty");
            return 1;
        }
        peeroxide::discovery_key(passphrase.as_bytes())
    } else {
        let mut seed = [0u8; 32];
        use rand::RngCore;
        rand::rng().fill_bytes(&mut seed);
        seed
    };

    let root_kp = KeyPair::from_seed(root_seed);
    let crc = compute_crc32c(&data);

    let chunks = split_into_chunks(&data, total_chunks as u16, crc, &root_seed);

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

    let (max_concurrency, dispatch_delay): (Option<usize>, Option<Duration>) = if let Some(ref speed_str) = args.max_speed {
        match parse_max_speed(speed_str) {
            Ok(speed) => {
                let cap = ((speed / 22000) as usize).max(1);
                let delay = Duration::from_secs_f64(22000.0 / speed as f64);
                (Some(cap), Some(delay))
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    } else {
        (None, None)
    };

    eprintln!("DEADDROP LEAVE {} chunks ({} bytes)", total_chunks, data.len());

    if let Err(e) = publish_chunks(&handle, &chunks, max_concurrency, dispatch_delay, true).await {
        eprintln!("error: publish failed: {e}");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    let pickup_key = to_hex(&root_kp.public_key);
    println!("{pickup_key}");

    eprintln!("  published to DHT (best-effort)");
    eprintln!("  pickup key printed to stdout");
    eprintln!("  refreshing every {}s, monitoring for acks...", args.refresh_interval);

    let ack_topic = peeroxide::discovery_key(&[root_kp.public_key.as_slice(), b"ack"].concat());
    let mut seen_acks: HashSet<[u8; 32]> = HashSet::new();
    let mut pickup_count: u64 = 0;

    let ttl_deadline = args.ttl.map(|t| tokio::time::Instant::now() + Duration::from_secs(t));
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(args.refresh_interval));
    refresh_interval.tick().await;
    let mut ack_interval = tokio::time::interval(Duration::from_secs(30));
    ack_interval.tick().await;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => break,
            _ = super::sigterm_recv() => break,
            _ = async {
                if let Some(deadline) = ttl_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => break,
            _ = refresh_interval.tick() => {
                eprintln!("  refreshing {} chunks...", chunks.len());
                if let Err(e) = publish_chunks(&handle, &chunks, max_concurrency, dispatch_delay, true).await {
                    eprintln!("  warning: refresh failed: {e}");
                }
            }
            _ = ack_interval.tick() => {
                if let Ok(results) = handle.lookup(ack_topic).await {
                    for result in &results {
                        for peer in &result.peers {
                            if seen_acks.insert(peer.public_key) {
                                pickup_count += 1;
                                eprintln!("  [ack] pickup #{pickup_count} detected");
                                if let Some(max) = args.max_pickups {
                                    if pickup_count >= max {
                                        eprintln!("  max pickups reached, stopping");
                                        let _ = handle.destroy().await;
                                        let _ = task.await;
                                        return 0;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    eprintln!("  stopped refreshing; records expire in ~20m");
    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

fn rpassword_read() -> String {
    use std::io::{BufRead, BufReader};
    let tty = match std::fs::File::open("/dev/tty") {
        Ok(f) => f,
        Err(_) => {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap_or(0);
            return line.trim_end_matches('\n').trim_end_matches('\r').to_string();
        }
    };
    let mut reader = BufReader::new(tty);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap_or(0);
    line.trim_end_matches('\n').trim_end_matches('\r').to_string()
}

fn compute_chunk_count(data_len: usize) -> usize {
    if data_len <= ROOT_PAYLOAD_MAX {
        1
    } else {
        let remaining = data_len - ROOT_PAYLOAD_MAX;
        1 + remaining.div_ceil(NON_ROOT_PAYLOAD_MAX)
    }
}

struct ChunkData {
    keypair: KeyPair,
    encoded: Vec<u8>,
}

fn split_into_chunks(data: &[u8], total: u16, crc: u32, root_seed: &[u8; 32]) -> Vec<ChunkData> {
    let mut chunks = Vec::new();
    let root_kp = KeyPair::from_seed(*root_seed);

    let root_payload_len = data.len().min(ROOT_PAYLOAD_MAX);
    let root_payload = &data[..root_payload_len];
    let mut offset = root_payload_len;

    let mut keypairs: Vec<KeyPair> = Vec::with_capacity(total as usize);
    keypairs.push(root_kp.clone());
    for i in 1..total {
        keypairs.push(derive_chunk_keypair(root_seed, i));
    }

    let next_pk = if total > 1 {
        keypairs[1].public_key
    } else {
        [0u8; 32]
    };

    chunks.push(ChunkData {
        keypair: root_kp,
        encoded: encode_root_chunk(total, crc, &next_pk, root_payload),
    });

    for i in 1..total as usize {
        let payload_len = (data.len() - offset).min(NON_ROOT_PAYLOAD_MAX);
        let payload = &data[offset..offset + payload_len];
        offset += payload_len;

        let next_pk = if i + 1 < total as usize {
            keypairs[i + 1].public_key
        } else {
            [0u8; 32]
        };

        chunks.push(ChunkData {
            keypair: keypairs[i].clone(),
            encoded: encode_non_root_chunk(&next_pk, payload),
        });
    }

    chunks
}

struct AimdController {
    current: usize,
    max_cap: Option<usize>,
    window_size: usize,
    degraded_in_window: u32,
    total_in_window: u32,
}

impl AimdController {
    fn new(initial: usize, max_cap: Option<usize>) -> Self {
        Self {
            current: initial,
            max_cap,
            window_size: 10,
            degraded_in_window: 0,
            total_in_window: 0,
        }
    }

    fn record(&mut self, degraded: bool) -> Option<usize> {
        if degraded {
            self.degraded_in_window += 1;
        }
        self.total_in_window += 1;

        if self.total_in_window >= self.window_size as u32 {
            let ratio = self.degraded_in_window as f64 / self.total_in_window as f64;
            self.degraded_in_window = 0;
            self.total_in_window = 0;

            if ratio > 0.3 {
                self.current = (self.current / 2).max(1);
            } else if ratio == 0.0 {
                let next = self.current + 1;
                self.current = match self.max_cap {
                    Some(cap) => next.min(cap),
                    None => next,
                };
            }
            Some(self.current)
        } else {
            None
        }
    }
}

async fn publish_chunks(
    handle: &HyperDhtHandle,
    chunks: &[ChunkData],
    max_concurrency: Option<usize>,
    dispatch_delay: Option<Duration>,
    show_progress: bool,
) -> Result<(), String> {
    let initial_concurrency = 4usize;
    let sem = Arc::new(Semaphore::new(initial_concurrency));
    let active_target = Arc::new(AtomicUsize::new(initial_concurrency));
    let permits_to_forget = Arc::new(AtomicUsize::new(0));
    let controller = Arc::new(Mutex::new(AimdController::new(initial_concurrency, max_concurrency)));

    let total = chunks.len();
    let mut completed = 0usize;

    let mut handles: Vec<tokio::task::JoinHandle<Result<MutablePutResult, String>>> = Vec::new();
    for chunk in chunks {
        let permit = loop {
            let p = sem.clone().acquire_owned().await.unwrap();
            let forget_pending = permits_to_forget.load(Ordering::Relaxed);
            if forget_pending > 0 && permits_to_forget.fetch_sub(1, Ordering::Relaxed) > 0 {
                p.forget();
            } else {
                break p;
            }
        };

        let h = handle.clone();
        let kp = chunk.keypair.clone();
        let data = chunk.encoded.clone();

        let seq = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let sem_inner = sem.clone();
        let active_target_inner = active_target.clone();
        let permits_to_forget_inner = permits_to_forget.clone();
        let controller_inner = controller.clone();

        handles.push(tokio::spawn(async move {
            let result = h.mutable_put(&kp, &data, seq).await;
            let put_result = match result {
                Ok(r) => r,
                Err(e) => {
                    drop(permit);
                    return Err(format!("mutable_put failed: {e}"));
                }
            };

            let degraded = put_result.commit_timeouts > 0;
            let new_target = {
                let mut ctrl = controller_inner.lock().await;
                ctrl.record(degraded)
            };

            if let Some(target) = new_target {
                let current_target = active_target_inner.load(Ordering::Relaxed);
                if target > current_target {
                    let add = target - current_target;
                    sem_inner.add_permits(add);
                    active_target_inner.store(target, Ordering::Relaxed);
                } else if target < current_target {
                    let remove = current_target - target;
                    permits_to_forget_inner.fetch_add(remove, Ordering::Relaxed);
                    active_target_inner.store(target, Ordering::Relaxed);
                }
            }

            drop(permit);
            Ok(put_result)
        }));

        if let Some(delay) = dispatch_delay {
            tokio::time::sleep(delay).await;
        }

        let mut i = 0;
        while i < handles.len() {
            if handles[i].is_finished() {
                let h = handles.swap_remove(i);
                match h.await {
                    Ok(Ok(_)) => {
                        completed += 1;
                        if show_progress {
                            eprintln!("  published chunk {completed}/{total}");
                        }
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(e) => return Err(format!("task panicked: {e}")),
                }
            } else {
                i += 1;
            }
        }
    }

    for h in handles {
        match h.await {
            Ok(Ok(_)) => {
                completed += 1;
                if show_progress {
                    eprintln!("  published chunk {completed}/{total}");
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(format!("task panicked: {e}")),
        }
    }

    Ok(())
}

async fn run_pickup(args: PickupArgs, cfg: &ResolvedConfig) -> i32 {
    if args.timeout == 0 {
        eprintln!("error: --timeout must be greater than 0");
        return 1;
    }

    let root_public_key = if let Some(ref phrase) = args.passphrase {
        if phrase.is_empty() {
            eprintln!("error: passphrase cannot be empty");
            return 1;
        }
        derive_pk_from_passphrase(phrase)
    } else if args.interactive_passphrase {
        eprintln!("Enter passphrase: ");
        let passphrase = rpassword_read();
        if passphrase.is_empty() {
            eprintln!("error: passphrase cannot be empty");
            return 1;
        }
        derive_pk_from_passphrase(&passphrase)
    } else {
        let key = args.key.as_ref().unwrap();
        if key.len() == 64 {
            match hex::decode(key) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut pk = [0u8; 32];
                    pk.copy_from_slice(&bytes);
                    pk
                }
                _ => derive_pk_from_passphrase(key),
            }
        } else {
            derive_pk_from_passphrase(key)
        }
    };

    let pk_hex = to_hex(&root_public_key);
    eprintln!("DEADDROP PICKUP @{}...", &pk_hex[..8]);

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

    let chunk_timeout = Duration::from_secs(args.timeout);

    let root_data = match fetch_with_retry(&handle, &root_public_key, chunk_timeout).await {
        Some(d) => d,
        None => {
            eprintln!("error: root chunk not found (timeout after {}s)", args.timeout);
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }
    };

    if root_data.is_empty() || root_data[0] != VERSION {
        eprintln!("error: invalid root chunk (bad version)");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    if root_data.len() < ROOT_HEADER_SIZE {
        eprintln!("error: root chunk too small");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    let total_chunks = u16::from_le_bytes([root_data[1], root_data[2]]) as usize;
    let stored_crc = u32::from_le_bytes([root_data[3], root_data[4], root_data[5], root_data[6]]);
    let mut next_pk = [0u8; 32];
    next_pk.copy_from_slice(&root_data[7..39]);
    let root_payload = &root_data[39..];

    if total_chunks == 0 || total_chunks > MAX_CHUNKS {
        eprintln!("error: invalid chunk count: {total_chunks}");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    eprintln!("  fetching chunk 1/{total_chunks}...");

    let mut payload_data = Vec::new();
    payload_data.extend_from_slice(root_payload);

    let mut seen_keys: HashSet<[u8; 32]> = HashSet::new();
    seen_keys.insert(root_public_key);

    for i in 1..total_chunks {
        eprintln!("  fetching chunk {}/{}...", i + 1, total_chunks);

        if next_pk == [0u8; 32] {
            if i == total_chunks - 1 {
                break;
            }
            eprintln!("error: chain ended prematurely at chunk {i}");
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }

        if !seen_keys.insert(next_pk) {
            eprintln!("error: loop detected in chunk chain");
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }

        let chunk_data = match fetch_with_retry(&handle, &next_pk, chunk_timeout).await {
            Some(d) => d,
            None => {
                eprintln!("error: chunk {} not found (timeout)", i + 1);
                let _ = handle.destroy().await;
                let _ = task.await;
                return 1;
            }
        };

        if chunk_data.is_empty() || chunk_data[0] != VERSION {
            eprintln!("error: invalid chunk {} (bad version)", i + 1);
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }

        if chunk_data.len() < NON_ROOT_HEADER_SIZE {
            eprintln!("error: chunk {} too small", i + 1);
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }

        next_pk.copy_from_slice(&chunk_data[1..33]);
        let chunk_payload = &chunk_data[33..];
        payload_data.extend_from_slice(chunk_payload);
    }

    if total_chunks > 1 && next_pk != [0u8; 32] {
        eprintln!("error: final chunk does not terminate chain (next != zeros)");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    let computed_crc = compute_crc32c(&payload_data);
    if computed_crc != stored_crc {
        eprintln!("error: CRC mismatch (expected {stored_crc:08x}, got {computed_crc:08x})");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    eprintln!("  reassembled {} bytes", payload_data.len());

    if let Some(ref output_path) = args.output {
        let dir = std::path::Path::new(output_path)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let temp_path = dir.join(format!(".peeroxide-pickup-{}", std::process::id()));

        if let Err(e) = tokio::fs::write(&temp_path, &payload_data).await {
            eprintln!("error: failed to write temp file: {e}");
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }

        if let Err(e) = tokio::fs::rename(&temp_path, output_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            eprintln!("error: failed to rename: {e}");
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }

        eprintln!("  written to {output_path}");
    } else {
        use std::io::Write;
        if let Err(e) = std::io::stdout().write_all(&payload_data) {
            eprintln!("error: failed to write to stdout: {e}");
            let _ = handle.destroy().await;
            let _ = task.await;
            return 1;
        }
    }

    if !args.no_ack {
        let ack_topic =
            peeroxide::discovery_key(&[root_public_key.as_slice(), b"ack"].concat());
        let ack_kp = KeyPair::generate();
        let _ = handle.announce(ack_topic, &ack_kp, &[]).await;
        eprintln!("  ack sent (ephemeral identity)");
    } else {
        eprintln!("  done (no ack sent)");
    }

    eprintln!("  done");
    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

fn derive_pk_from_passphrase(passphrase: &str) -> [u8; 32] {
    let seed = peeroxide::discovery_key(passphrase.as_bytes());
    let kp = KeyPair::from_seed(seed);
    kp.public_key
}

async fn fetch_with_retry(
    handle: &HyperDhtHandle,
    public_key: &[u8; 32],
    timeout: Duration,
) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        match handle.mutable_get(public_key, 0).await {
            Ok(Some(result)) => return Some(result.value),
            Ok(None) => {}
            Err(_) => {}
        }

        if tokio::time::Instant::now() >= deadline {
            return None;
        }

        tokio::time::sleep(backoff.min(deadline - tokio::time::Instant::now())).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_chunk_count_single() {
        assert_eq!(compute_chunk_count(0), 1);
        assert_eq!(compute_chunk_count(1), 1);
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX), 1);
    }

    #[test]
    fn compute_chunk_count_two() {
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX + 1), 2);
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX + NON_ROOT_PAYLOAD_MAX), 2);
    }

    #[test]
    fn compute_chunk_count_three() {
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX + NON_ROOT_PAYLOAD_MAX + 1), 3);
    }

    #[test]
    fn encode_root_chunk_structure() {
        let next_pk = [0xAA; 32];
        let payload = b"hello";
        let encoded = encode_root_chunk(5, 0x12345678, &next_pk, payload);

        assert_eq!(encoded[0], VERSION);
        assert_eq!(u16::from_le_bytes([encoded[1], encoded[2]]), 5);
        assert_eq!(
            u32::from_le_bytes([encoded[3], encoded[4], encoded[5], encoded[6]]),
            0x12345678
        );
        assert_eq!(&encoded[7..39], &[0xAA; 32]);
        assert_eq!(&encoded[39..], b"hello");
        assert_eq!(encoded.len(), ROOT_HEADER_SIZE + 5);
    }

    #[test]
    fn encode_non_root_chunk_structure() {
        let next_pk = [0xBB; 32];
        let payload = b"world";
        let encoded = encode_non_root_chunk(&next_pk, payload);

        assert_eq!(encoded[0], VERSION);
        assert_eq!(&encoded[1..33], &[0xBB; 32]);
        assert_eq!(&encoded[33..], b"world");
        assert_eq!(encoded.len(), NON_ROOT_HEADER_SIZE + 5);
    }

    #[test]
    fn split_and_reassemble_single_chunk() {
        let data = b"short message";
        let seed = [0x42; 32];
        let crc = compute_crc32c(data);
        let chunks = split_into_chunks(data, 1, crc, &seed);

        assert_eq!(chunks.len(), 1);
        let encoded = &chunks[0].encoded;
        assert_eq!(encoded[0], VERSION);
        assert_eq!(u16::from_le_bytes([encoded[1], encoded[2]]), 1);
        let stored_crc = u32::from_le_bytes([encoded[3], encoded[4], encoded[5], encoded[6]]);
        assert_eq!(stored_crc, crc);
        assert_eq!(&encoded[7..39], &[0u8; 32]);
        assert_eq!(&encoded[39..], data.as_slice());
    }

    #[test]
    fn split_and_reassemble_multi_chunk() {
        let data = vec![0x42u8; ROOT_PAYLOAD_MAX + NON_ROOT_PAYLOAD_MAX + 100];
        let crc = compute_crc32c(&data);
        let total = compute_chunk_count(data.len()) as u16;
        assert_eq!(total, 3);

        let seed = [0x01; 32];
        let chunks = split_into_chunks(&data, total, crc, &seed);
        assert_eq!(chunks.len(), 3);

        let root = &chunks[0].encoded;
        let root_total = u16::from_le_bytes([root[1], root[2]]);
        assert_eq!(root_total, 3);
        let root_payload = &root[39..];
        assert_eq!(root_payload.len(), ROOT_PAYLOAD_MAX);

        let c1 = &chunks[1].encoded;
        let c1_payload = &c1[33..];
        assert_eq!(c1_payload.len(), NON_ROOT_PAYLOAD_MAX);

        let c2 = &chunks[2].encoded;
        assert_eq!(&c2[1..33], &[0u8; 32]);
        let c2_payload = &c2[33..];
        assert_eq!(c2_payload.len(), 100);

        let mut reassembled = Vec::new();
        reassembled.extend_from_slice(root_payload);
        reassembled.extend_from_slice(c1_payload);
        reassembled.extend_from_slice(c2_payload);
        assert_eq!(reassembled, data);
        assert_eq!(compute_crc32c(&reassembled), crc);
    }

    #[test]
    fn derive_chunk_keypair_deterministic() {
        let seed = [0xAB; 32];
        let kp1 = derive_chunk_keypair(&seed, 1);
        let kp2 = derive_chunk_keypair(&seed, 1);
        assert_eq!(kp1.public_key, kp2.public_key);

        let kp3 = derive_chunk_keypair(&seed, 2);
        assert_ne!(kp1.public_key, kp3.public_key);
    }

    #[test]
    fn crc32c_basic() {
        let data = b"hello world";
        let crc = compute_crc32c(data);
        assert_eq!(crc, crc32c::crc32c(data));
        assert_ne!(crc, 0);
    }

    #[test]
    fn parse_max_speed_units() {
        assert_eq!(parse_max_speed("100k").unwrap(), 100_000);
        assert_eq!(parse_max_speed("1m").unwrap(), 1_000_000);
        assert_eq!(parse_max_speed("5000").unwrap(), 5000);
        assert_eq!(parse_max_speed(" 2M ").unwrap(), 2_000_000);
        assert!(parse_max_speed("abc").is_err());
    }

    #[test]
    fn derive_pk_from_passphrase_deterministic() {
        let pk1 = derive_pk_from_passphrase("test-phrase");
        let pk2 = derive_pk_from_passphrase("test-phrase");
        assert_eq!(pk1, pk2);

        let pk3 = derive_pk_from_passphrase("different-phrase");
        assert_ne!(pk1, pk3);
    }

    #[test]
    fn chunk_chain_links_correctly() {
        let data = vec![0xFFu8; ROOT_PAYLOAD_MAX + 10];
        let crc = compute_crc32c(&data);
        let total = compute_chunk_count(data.len()) as u16;
        let seed = [0x99; 32];
        let chunks = split_into_chunks(&data, total, crc, &seed);

        let root = &chunks[0].encoded;
        let next_in_root = &root[7..39];
        assert_eq!(next_in_root, chunks[1].keypair.public_key.as_slice());

        let c1 = &chunks[1].encoded;
        let next_in_c1 = &c1[1..33];
        assert_eq!(next_in_c1, &[0u8; 32]);
    }
}
