pub mod progress;
pub mod v1;
pub mod v2;

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
use crate::cmd::deaddrop::progress::reporter::ProgressReporter;
use crate::cmd::deaddrop::progress::state::{Phase, ProgressState};

const MAX_PAYLOAD: usize = 1000;

#[derive(Subcommand)]
pub enum DdCommands {
    /// Store data at a dead drop location on the DHT
    Put(PutArgs),
    /// Retrieve data from a dead drop location on the DHT
    Get(GetArgs),
}

#[derive(Args)]
pub struct PutArgs {
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

    /// Disable progress output
    #[arg(long)]
    pub no_progress: bool,

    /// Emit JSON progress/output
    #[arg(long)]
    pub json: bool,

    /// Use legacy v1 protocol (default: v2)
    #[arg(long)]
    pub v1: bool,
}

#[derive(Args)]
pub struct GetArgs {
    /// Pickup key (64-char hex or passphrase text)
    #[arg(required_unless_present_any = ["passphrase", "interactive_passphrase"])]
    key: Option<String>,

    /// Derive pickup key from passphrase (provided on command line)
    #[arg(long, conflicts_with = "interactive_passphrase")]
    passphrase: Option<String>,

    /// Derive pickup key from passphrase (prompted interactively, hidden input)
    #[arg(long, conflicts_with = "passphrase")]
    interactive_passphrase: bool,

    /// Disable progress output
    #[arg(long)]
    pub no_progress: bool,

    /// Write output to file (default: stdout)
    #[arg(long)]
    output: Option<String>,

    /// Emit JSON progress/output
    #[arg(long, requires = "output")]
    pub json: bool,

    /// Give up on any single chunk after this duration (default: 1200s)
    #[arg(long, default_value_t = 1200)]
    timeout: u64,

    /// Don't announce pickup acknowledgement
    #[arg(long)]
    no_ack: bool,
}

pub async fn run(cmd: DdCommands, cfg: &ResolvedConfig) -> i32 {
    match cmd {
        DdCommands::Put(args) => {
            if args.v1 {
                v1::run_put(&args, cfg).await
            } else {
                v2::run_put(&args, cfg).await
            }
        }
        DdCommands::Get(args) => run_get(args, cfg).await,
    }
}

async fn run_get(args: GetArgs, cfg: &ResolvedConfig) -> i32 {
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
    eprintln!("DD GET @{}...", &pk_hex[..8]);

    let dht_config = build_dht_config(cfg);
    let runtime = match UdxRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to create UDP runtime: {e}");
            return 1;
        }
    };

    let (task_handle, handle, _rx) = match hyperdht::spawn(&runtime, dht_config).await {
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
            let _ = task_handle.await;
            return 1;
        }
    };

    if root_data.is_empty() {
        eprintln!("error: root chunk is empty");
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    match root_data[0] {
        0x01 => {
            let get_filename: Arc<str> = match args.output.as_deref() {
                None => Arc::from("<stdout>"),
                Some(p) => {
                    let base = std::path::Path::new(p)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(p);
                    Arc::from(base)
                }
            };
            let state = ProgressState::new(Phase::Get, 0x01, get_filename);
            let reporter =
                ProgressReporter::from_args(state.clone(), args.no_progress, args.json);
            reporter.on_start();
            v1::get_from_root(root_data, root_public_key, handle, task_handle, &args, state, reporter).await
        }
        0x02 => {
            let get_filename: Arc<str> = match args.output.as_deref() {
                None => Arc::from("<stdout>"),
                Some(p) => {
                    let base = std::path::Path::new(p)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(p);
                    Arc::from(base)
                }
            };
            let state = ProgressState::new(Phase::Get, 0x02, get_filename);
            let reporter =
                ProgressReporter::from_args(state.clone(), args.no_progress, args.json);
            reporter.on_start();
            v2::get_from_root(root_data, root_public_key, handle, task_handle, &args, state, reporter).await
        }
        v => {
            eprintln!("error: unknown dead drop version 0x{v:02x}");
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            1
        }
    }
}

fn compute_crc32c(data: &[u8]) -> u32 {
    crc32c::crc32c(data)
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

#[derive(Clone)]
pub(crate) struct ChunkData {
    keypair: KeyPair,
    encoded: Vec<u8>,
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
    progress: Option<Arc<ProgressState>>,
) -> Result<(), String> {
    let initial_concurrency = 4usize;
    let sem = Arc::new(Semaphore::new(initial_concurrency));
    let active_target = Arc::new(AtomicUsize::new(initial_concurrency));
    let permits_to_forget = Arc::new(AtomicUsize::new(0));
    let controller = Arc::new(Mutex::new(AimdController::new(initial_concurrency, max_concurrency)));

    let mut handles: Vec<tokio::task::JoinHandle<Result<MutablePutResult, String>>> = Vec::new();
    let mut chunk_byte_sizes: Vec<usize> = Vec::new();

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
        let chunk_size = data.len();

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
        chunk_byte_sizes.push(chunk_size);

        if let Some(delay) = dispatch_delay {
            tokio::time::sleep(delay).await;
        }

        let mut i = 0;
        while i < handles.len() {
            if handles[i].is_finished() {
                let chunk_bytes = chunk_byte_sizes.swap_remove(i);
                let h = handles.swap_remove(i);
                match h.await {
                    Ok(Ok(_)) => {
                        if let Some(ref state) = progress {
                            state.inc_data(chunk_bytes as u64);
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

    for (h, chunk_bytes) in handles.into_iter().zip(chunk_byte_sizes.into_iter()) {
        match h.await {
            Ok(Ok(_)) => {
                if let Some(ref state) = progress {
                    state.inc_data(chunk_bytes as u64);
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(format!("task panicked: {e}")),
        }
    }

    Ok(())
}

