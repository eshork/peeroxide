//! v3 sender: tree build + dependency-ordered publish + refresh + need-watch.

#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use libudx::UdxRuntime;
use peeroxide::KeyPair;
use peeroxide_dht::hyperdht::{self, HyperDhtHandle};
use rand::RngCore;
use tokio::signal;
use tokio::sync::{Mutex, Notify, Semaphore};

use crate::cmd::deaddrop::progress::reporter::ProgressReporter;
use crate::cmd::deaddrop::progress::state::{Phase, ProgressState};
use crate::cmd::sigterm_recv;
use crate::config::ResolvedConfig;

use super::super::{build_dht_config, to_hex, PutArgs};
use super::build::{build_tree, BuiltTree};
use super::keys::{ack_topic, need_topic};
use super::need::{decode_need_list, response_chunks_for_list};
use super::tree::data_chunk_count;
use super::wire::DATA_PAYLOAD_MAX;

/// Maximum tree depth the sender will produce by default. Override via
/// `--allow-deep` flag (TODO: add to PutArgs in a follow-up). Beyond this,
/// the sender refuses to build the tree.
pub const SOFT_DEPTH_CAP: u32 = 4;

/// How often the sender polls for need-list publishers from receivers.
const NEED_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// How often the sender re-announces its presence on the need topic
/// (this is on the receiver side; we keep the constant here for the
/// equivalent receiver-side use).
#[allow(dead_code)]
const NEED_REANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

/// AIMD controller: monitors put-result degradation and adjusts an effective
/// concurrency target.
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

/// Single shared concurrency state between the publish pipeline and the AIMD
/// controller. Permits are forgotten on shrink and added back on grow.
#[derive(Clone)]
struct ConcurrencyState {
    sem: Arc<Semaphore>,
    target: Arc<AtomicUsize>,
    forget_pending: Arc<AtomicUsize>,
    aimd: Arc<Mutex<AimdController>>,
}

impl ConcurrencyState {
    fn new(initial: usize, max_cap: Option<usize>) -> Self {
        Self {
            sem: Arc::new(Semaphore::new(initial)),
            target: Arc::new(AtomicUsize::new(initial)),
            forget_pending: Arc::new(AtomicUsize::new(0)),
            aimd: Arc::new(Mutex::new(AimdController::new(initial, max_cap))),
        }
    }

    /// Acquire a permit, honoring any pending shrink (forget).
    async fn acquire(&self) -> tokio::sync::OwnedSemaphorePermit {
        loop {
            let permit = self.sem.clone().acquire_owned().await.unwrap();
            let pending = self.forget_pending.load(Ordering::Relaxed);
            if pending > 0
                && self
                    .forget_pending
                    .fetch_sub(1, Ordering::Relaxed)
                    > 0
            {
                permit.forget();
            } else {
                return permit;
            }
        }
    }

    /// Record an outcome and rebalance permits if AIMD has changed the target.
    async fn record(&self, degraded: bool) {
        let new_target = {
            let mut ctrl = self.aimd.lock().await;
            ctrl.record(degraded)
        };
        if let Some(target) = new_target {
            let current_target = self.target.load(Ordering::Relaxed);
            match target.cmp(&current_target) {
                std::cmp::Ordering::Greater => {
                    let add = target - current_target;
                    self.sem.add_permits(add);
                    self.target.store(target, Ordering::Relaxed);
                }
                std::cmp::Ordering::Less => {
                    let remove = current_target - target;
                    self.forget_pending.fetch_add(remove, Ordering::Relaxed);
                    self.target.store(target, Ordering::Relaxed);
                }
                std::cmp::Ordering::Equal => {}
            }
        }
    }
}

/// Publish a single mutable record (signed by `kp` with the current Unix
/// timestamp as `seq`). Returns whether the put was degraded (commit
/// timeouts > 0).
async fn put_mutable(
    handle: &HyperDhtHandle,
    kp: &KeyPair,
    bytes: &[u8],
) -> Result<bool, String> {
    let seq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match handle.mutable_put(kp, bytes, seq).await {
        Ok(r) => Ok(r.commit_timeouts > 0),
        Err(e) => Err(format!("mutable_put failed: {e}")),
    }
}

/// One unit of work for the publish pipeline.
enum PublishUnit {
    /// An immutable data chunk (`immutable_put`).
    Data { encoded: Vec<u8> },
    /// A signed mutable index chunk (`mutable_put`).
    Index { keypair: KeyPair, encoded: Vec<u8> },
}

/// Publish all non-root chunks (data + every index layer) interleaved through
/// the shared concurrency budget, then publish the root last.
///
/// The spec only requires that the root be published last — non-root chunks
/// can go in any order, since the root is the only discoverable entry point.
/// Interleaving lets the index counter make progress alongside the data
/// counter and avoids a "ramp twice" pattern (data then index) under AIMD.
async fn publish_tree_initial(
    handle: &HyperDhtHandle,
    tree: &BuiltTree,
    state: &ConcurrencyState,
    progress: Option<Arc<ProgressState>>,
) -> Result<(), String> {
    let mut units: Vec<PublishUnit> = Vec::with_capacity(tree.data_chunks.len() + tree.index_chunks.len());
    for chunk in &tree.data_chunks {
        units.push(PublishUnit::Data {
            encoded: chunk.encoded.clone(),
        });
    }
    for chunk in &tree.index_chunks {
        units.push(PublishUnit::Index {
            keypair: chunk.keypair.clone(),
            encoded: chunk.encoded.clone(),
        });
    }
    publish_units(handle, units, state, progress).await?;

    // Root last. This is the spec's only ordering requirement: until the root
    // is published, no one can derive any other pubkey in the drop, so a
    // partial publish is not discoverable.
    publish_units(
        handle,
        vec![PublishUnit::Index {
            keypair: tree.root_keypair.clone(),
            encoded: tree.root_encoded.clone(),
        }],
        state,
        None,
    )
    .await?;

    Ok(())
}

/// Re-publish the entire tree on a refresh tick.
async fn publish_tree_refresh(
    handle: &HyperDhtHandle,
    tree: &BuiltTree,
    state: &ConcurrencyState,
) -> Result<(), String> {
    let mut units: Vec<PublishUnit> = Vec::with_capacity(tree.data_chunks.len() + tree.index_chunks.len() + 1);
    for chunk in &tree.data_chunks {
        units.push(PublishUnit::Data {
            encoded: chunk.encoded.clone(),
        });
    }
    for chunk in &tree.index_chunks {
        units.push(PublishUnit::Index {
            keypair: chunk.keypair.clone(),
            encoded: chunk.encoded.clone(),
        });
    }
    units.push(PublishUnit::Index {
        keypair: tree.root_keypair.clone(),
        encoded: tree.root_encoded.clone(),
    });
    publish_units(handle, units, state, None).await
}

/// Fan out a batch of `PublishUnit`s through the shared concurrency budget.
async fn publish_units(
    handle: &HyperDhtHandle,
    units: Vec<PublishUnit>,
    state: &ConcurrencyState,
    progress: Option<Arc<ProgressState>>,
) -> Result<(), String> {
    let mut tasks = tokio::task::JoinSet::new();
    for unit in units {
        let permit = state.acquire().await;
        let h = handle.clone();
        let st = state.clone();
        let pg = progress.clone();
        match unit {
            PublishUnit::Data { encoded } => {
                let chunk_len = encoded.len() as u64;
                tasks.spawn(async move {
                    let result = h.immutable_put(&encoded).await;
                    let degraded = result.is_err();
                    st.record(degraded).await;
                    drop(permit);
                    if let Some(state) = pg {
                        state.inc_data(chunk_len);
                    }
                    result
                        .map(|_| ())
                        .map_err(|e| format!("immutable_put failed: {e}"))
                });
            }
            PublishUnit::Index { keypair, encoded } => {
                tasks.spawn(async move {
                    let res = put_mutable(&h, &keypair, &encoded).await;
                    let degraded = res.as_ref().map(|d| *d).unwrap_or(true);
                    st.record(degraded).await;
                    drop(permit);
                    if let Some(state) = pg {
                        state.inc_index();
                    }
                    res.map(|_| ())
                });
            }
        }
    }
    while let Some(joined) = tasks.join_next().await {
        joined.map_err(|e| format!("publish task panicked: {e}"))??;
    }
    Ok(())
}

/// Re-publish a specific subset of chunks (for need-list responses).
async fn publish_partial(
    handle: &HyperDhtHandle,
    tree: &BuiltTree,
    data_indices: &[usize],
    index_indices: &[usize],
    state: &ConcurrencyState,
) -> Result<(), String> {
    // Data chunks first.
    let mut tasks = tokio::task::JoinSet::new();
    for &i in data_indices {
        let permit = state.acquire().await;
        let h = handle.clone();
        let bytes = tree.data_chunks[i].encoded.clone();
        let st = state.clone();
        tasks.spawn(async move {
            let res = h.immutable_put(&bytes).await;
            let degraded = res.is_err();
            st.record(degraded).await;
            drop(permit);
            res.map(|_| ()).map_err(|e| format!("immutable_put failed: {e}"))
        });
    }
    while let Some(joined) = tasks.join_next().await {
        joined.map_err(|e| format!("partial-data task panicked: {e}"))??;
    }
    // Then index chunks.
    let mut tasks = tokio::task::JoinSet::new();
    for &i in index_indices {
        let chunk = &tree.index_chunks[i];
        let permit = state.acquire().await;
        let h = handle.clone();
        let kp = chunk.keypair.clone();
        let bytes = chunk.encoded.clone();
        let st = state.clone();
        tasks.spawn(async move {
            let res = put_mutable(&h, &kp, &bytes).await;
            let degraded = res.as_ref().map(|d| *d).unwrap_or(true);
            st.record(degraded).await;
            drop(permit);
            res.map(|_| ())
        });
    }
    while let Some(joined) = tasks.join_next().await {
        joined.map_err(|e| format!("partial-index task panicked: {e}"))??;
    }
    Ok(())
}

/// Background task: poll the need topic and republish chunks as receivers
/// request them. Ends when `shutdown` fires.
async fn run_need_watcher(
    handle: HyperDhtHandle,
    tree: Arc<BuiltTree>,
    need_topic_key: [u8; 32],
    state: ConcurrencyState,
    shutdown: Arc<Notify>,
) {
    let mut seen_peers: HashSet<[u8; 32]> = HashSet::new();
    eprintln!(
        "  need-list watcher started (poll every {}s)",
        NEED_POLL_INTERVAL.as_secs()
    );
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = tokio::time::sleep(NEED_POLL_INTERVAL) => {
                let lookup = match handle.lookup(need_topic_key).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("  warning: need-topic lookup failed: {e}");
                        continue;
                    }
                };
                for result in &lookup {
                    for peer in &result.peers {
                        if seen_peers.insert(peer.public_key) {
                            eprintln!(
                                "  need-list peer discovered: {}",
                                &to_hex(&peer.public_key)[..8]
                            );
                        }
                        let value = match handle.mutable_get(&peer.public_key, 0).await {
                            Ok(Some(v)) => v.value,
                            Ok(None) => continue,
                            Err(e) => {
                                eprintln!(
                                    "  warning: need-list get from {} failed: {e}",
                                    &to_hex(&peer.public_key)[..8]
                                );
                                continue;
                            }
                        };
                        let entries = match decode_need_list(&value) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!(
                                    "  warning: malformed need-list from {}: {e}",
                                    &to_hex(&peer.public_key)[..8]
                                );
                                continue;
                            }
                        };
                        if entries.is_empty() {
                            continue;
                        }
                        let resp = response_chunks_for_list(&tree, &entries);
                        let n_data = resp.data_chunk_indices.len();
                        let n_index = resp.index_chunk_indices.len();
                        eprintln!(
                            "  need-list received from {}: {} data + {} index chunks to republish",
                            &to_hex(&peer.public_key)[..8],
                            n_data,
                            n_index
                        );
                        if let Err(e) = publish_partial(
                            &handle,
                            &tree,
                            &resp.data_chunk_indices,
                            &resp.index_chunk_indices,
                            &state,
                        )
                        .await
                        {
                            eprintln!("  warning: need-list republish failed: {e}");
                        } else {
                            eprintln!(
                                "  need-list republish complete: {n_data} data + {n_index} index"
                            );
                        }
                    }
                }
            }
        }
    }
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

/// Read input bytes for the put operation. Uses mmap when reading from a
/// regular file (low RAM footprint); falls back to in-memory buffering for
/// stdin (where mmap is not applicable).
fn read_input(path: &str) -> Result<Vec<u8>, String> {
    if path == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        Ok(buf)
    } else {
        // Open + mmap. We materialize into Vec<u8> here so build_tree's
        // chunk iterator can hold simple slices. The mmap is dropped at
        // function end. For the strict zero-RAM path, build_tree should
        // accept a borrowed slice (which is what `Vec` provides via deref);
        // future iteration could pass the Mmap's Deref directly through.
        let file = std::fs::File::open(path).map_err(|e| format!("failed to open {path}: {e}"))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("failed to stat {path}: {e}"))?;
        if metadata.len() == 0 {
            return Ok(Vec::new());
        }
        let mmap = unsafe {
            memmap2::Mmap::map(&file).map_err(|e| format!("mmap failed for {path}: {e}"))?
        };
        Ok(mmap.to_vec())
    }
}

/// Top-level PUT entry point.
pub async fn run_put(args: &PutArgs, cfg: &ResolvedConfig) -> i32 {
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

    // 1. Read input.
    let data = match read_input(&args.file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    // 2. Tree-shape soft cap check.
    let n = data_chunk_count(data.len() as u64);
    let depth = super::tree::canonical_depth(n);
    if depth > SOFT_DEPTH_CAP {
        eprintln!(
            "error: file requires tree depth {depth} (soft cap is {SOFT_DEPTH_CAP}); pass --allow-deep to override"
        );
        return 1;
    }

    // 3. Resolve root_seed.
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
        rand::rng().fill_bytes(&mut seed);
        seed
    };

    // 4. Build the tree.
    let tree = match build_tree(
        &root_seed,
        data.len() as u64,
        crc32c::crc32c(&data),
        data.chunks(DATA_PAYLOAD_MAX),
    ) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    // 5. Spawn DHT.
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
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    // 6. Concurrency / rate-limit setup.
    let (max_concurrency, _dispatch_delay): (Option<usize>, Option<Duration>) =
        if let Some(ref speed_str) = args.max_speed {
            match parse_max_speed(speed_str) {
                Ok(speed) => {
                    let cap = ((speed / 22000) as usize).max(1);
                    let delay = Duration::from_secs_f64(22000.0 / speed as f64);
                    (Some(cap), Some(delay))
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    let _ = handle.destroy().await;
                    let _ = task.await;
                    return 1;
                }
            }
        } else {
            (None, None)
        };
    // Initial concurrency. AIMD will adjust based on observed degradation;
    // starting higher gives better throughput on healthy networks while still
    // allowing the controller to shrink if puts start timing out.
    let initial_concurrency = 16usize;
    let conc = ConcurrencyState::new(initial_concurrency, max_concurrency);

    // 7. Progress reporter.
    let filename: Arc<str> = if args.file == "-" {
        Arc::from("<stdin>")
    } else {
        let base = std::path::Path::new(&args.file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| args.file.clone());
        Arc::from(base.as_str())
    };
    let state = ProgressState::new(Phase::Put, 2, filename);
    state.set_length(
        data.len() as u64,
        (tree.index_chunks.len() + 1) as u32, // include root
        tree.data_chunks.len() as u32,
    );
    let mut reporter = ProgressReporter::from_args(state.clone(), args.no_progress, args.json);
    reporter.on_start();

    // 8. Initial publish.
    if let Err(e) = publish_tree_initial(&handle, &tree, &conc, Some(state.clone())).await {
        eprintln!("error: publish failed: {e}");
        reporter.finish().await;
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    // 9. Print pickup key.
    let pickup_key = to_hex(&tree.root_keypair.public_key);
    reporter.emit_initial_publish_complete(&pickup_key).await;

    eprintln!("  published to DHT (best-effort)");
    eprintln!("  pickup key printed to stdout");
    eprintln!(
        "  refreshing every {}s, polling needs every {}s, monitoring for acks every 30s...",
        args.refresh_interval,
        NEED_POLL_INTERVAL.as_secs()
    );

    // 10. Spawn need-watcher.
    let tree_arc = Arc::new(tree);
    let need_topic_key = need_topic(&tree_arc.root_keypair.public_key);
    let watcher_shutdown = Arc::new(Notify::new());
    let watcher_handle = tokio::spawn(run_need_watcher(
        handle.clone(),
        tree_arc.clone(),
        need_topic_key,
        conc.clone(),
        watcher_shutdown.clone(),
    ));

    // 11. Refresh + ack loop.
    let ack_topic_key = ack_topic(&tree_arc.root_keypair.public_key);
    let mut seen_acks: HashSet<[u8; 32]> = HashSet::new();
    let mut pickup_count: u64 = 0;
    let ttl_deadline = args
        .ttl
        .map(|t| tokio::time::Instant::now() + Duration::from_secs(t));
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(args.refresh_interval));
    refresh_interval.tick().await;
    let mut ack_interval = tokio::time::interval(Duration::from_secs(30));
    ack_interval.tick().await;

    let exit_code: i32 = loop {
        tokio::select! {
            _ = signal::ctrl_c() => break 0,
            _ = sigterm_recv() => break 0,
            _ = async {
                if let Some(deadline) = ttl_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => break 0,
            _ = refresh_interval.tick() => {
                eprintln!(
                    "  refreshing tree ({} index + {} data chunks)...",
                    tree_arc.index_chunks.len() + 1,
                    tree_arc.data_chunks.len()
                );
                if let Err(e) = publish_tree_refresh(&handle, &tree_arc, &conc).await {
                    eprintln!("  warning: refresh failed: {e}");
                }
            }
            _ = ack_interval.tick() => {
                let mut max_reached = false;
                if let Ok(results) = handle.lookup(ack_topic_key).await {
                    'outer: for result in &results {
                        for peer in &result.peers {
                            if seen_acks.insert(peer.public_key) {
                                pickup_count += 1;
                                reporter.on_ack(pickup_count, &to_hex(&peer.public_key));
                                eprintln!("  [ack] pickup #{pickup_count} detected");
                                if let Some(max) = args.max_pickups {
                                    if pickup_count >= max {
                                        eprintln!("  max pickups reached, stopping");
                                        max_reached = true;
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                }
                if max_reached {
                    break 0;
                }
            }
        }
    };

    // 12. Cleanup.
    watcher_shutdown.notify_one();
    let _ = watcher_handle.await;
    eprintln!("  stopped refreshing; records expire in ~20m");
    reporter.finish().await;
    let _ = handle.destroy().await;
    let _ = task.await;
    exit_code
}
