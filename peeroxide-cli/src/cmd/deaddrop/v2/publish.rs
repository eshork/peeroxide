//! v3 sender: tree build + dependency-ordered publish + refresh + need-watch.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use libudx::UdxRuntime;
use peeroxide::KeyPair;
use peeroxide_dht::hyperdht::{self, HyperDhtHandle};
use rand::RngCore;
use tokio::signal;
use tokio::sync::{Mutex, Notify, Semaphore};

use crate::cmd::deaddrop::progress::reporter::{OperationFactory, ProgressReporter};
use crate::cmd::deaddrop::progress::state::{Phase, ProgressState};
use crate::cmd::sigterm_recv;
use crate::config::ResolvedConfig;

use super::super::{build_dht_config, to_hex, PutArgs};
use super::build::{build_tree, BuiltTree};
use super::keys::{ack_topic, need_topic};
use super::need::{decode_need_list, response_chunks_for_list};
use super::queue::{ChunkId, Lane, Operation, WorkQueue};
use super::tree::data_chunk_count;
use super::wire::DATA_PAYLOAD_MAX;

/// Maximum tree depth the sender will produce by default. Override via
/// `--allow-deep` flag (TODO: add to PutArgs in a follow-up). Beyond this,
/// the sender refuses to build the tree.
pub const SOFT_DEPTH_CAP: u32 = 4;

/// How often the sender polls for need-list publishers from receivers.
const NEED_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Hard wall-clock cap on a single DHT put. The DHT layer has no terminal
/// timeout on `query` — a degenerate convergence can keep iterating
/// indefinitely while holding the publish-pipeline permit. Healthy puts
/// finish in 4–6s; 30s is ~5–7× that, well outside healthy variance.
/// On timeout the future is dropped (freeing the permit) and the outcome
/// is reported as degraded so AIMD reacts.
const PUT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the sender re-announces its presence on the need topic
/// (this is on the receiver side; we keep the constant here for the
/// equivalent receiver-side use).
#[allow(dead_code)]
const NEED_REANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

/// AIMD controller: monitors put-result degradation and adjusts an effective
/// concurrency target.
///
/// Reacts continuously via an EWMA of the degraded-put rate, with two
/// decision paths:
///
/// 1. **Normal**: every `decision_interval` samples, consult the EWMA and
///    shrink (>30%), grow (<5%), or hold (the dead band in between).
/// 2. **Fast trip**: if `fast_trip_threshold` degraded puts accumulate within
///    a single decision interval, shrink immediately without waiting for the
///    boundary. This catches sudden cliffs (e.g. a DHT region going dark)
///    that the EWMA alone would smear over.
///
/// A one-shot `shrink_cooldown` damps back-to-back shrinks so in-flight puts
/// from the larger target have a chance to drain before the next contraction.
struct AimdController {
    current: usize,
    /// Original target chosen at startup. Used by the stall watchdog as the
    /// reference for its recovery floor.
    initial: usize,
    max_cap: Option<usize>,
    /// EWMA of degradation in [0.0, 1.0]. Updated on every sample.
    ewma: f64,
    /// EWMA smoothing factor (per-sample weight). Smaller = smoother / slower
    /// to react; larger = more reactive but jumpier.
    alpha: f64,
    /// Samples observed since the last decision (gates the normal path).
    samples_since_decision: u32,
    /// Make a normal decision every `decision_interval` samples.
    decision_interval: u32,
    /// Degraded samples observed since the last decision (gates fast-trip).
    degraded_since_decision: u32,
    /// If degraded count reaches this *within* a decision interval, shrink
    /// immediately rather than waiting for the boundary.
    fast_trip_threshold: u32,
    /// If true, the previous decision shrank; suppress the next shrink so
    /// the system can drain before contracting again.
    shrink_cooldown: bool,
}

impl AimdController {
    fn new(initial: usize, max_cap: Option<usize>) -> Self {
        Self {
            current: initial,
            initial,
            max_cap,
            ewma: 0.0,
            // alpha = 0.1 → ~7-sample half-life; comparable reactivity to the
            // old 10-sample tumbling window but smooth and never blind.
            alpha: 0.1,
            samples_since_decision: 0,
            decision_interval: 20,
            degraded_since_decision: 0,
            // 50% degraded inside one decision interval → emergency shrink.
            fast_trip_threshold: 10,
            shrink_cooldown: false,
        }
    }

    /// Watchdog escape hatch: forcibly lift `current` to a recovery floor
    /// (half of initial) and clear adaptive state so the next real samples
    /// drive the decision afresh. Only returns Some when it actually raises
    /// current — if we're already at/above the floor, the stall is not an
    /// AIMD-wedge problem and we leave things alone.
    fn kick_stall(&mut self) -> Option<usize> {
        let floor = (self.initial / 2).max(1);
        if self.current >= floor {
            return None;
        }
        self.current = floor;
        self.ewma = 0.0;
        self.shrink_cooldown = false;
        self.samples_since_decision = 0;
        self.degraded_since_decision = 0;
        Some(self.current)
    }

    fn shrink_step(&mut self) -> usize {
        self.current = ((self.current as f64 * 0.75) as usize).max(1);
        self.shrink_cooldown = true;
        self.current
    }

    fn grow_step(&mut self) -> usize {
        let next = self.current + 2;
        self.current = match self.max_cap {
            Some(cap) => next.min(cap),
            None => next,
        };
        self.shrink_cooldown = false;
        self.current
    }

    fn reset_decision_window(&mut self) {
        self.samples_since_decision = 0;
        self.degraded_since_decision = 0;
    }

    fn record(&mut self, degraded: bool) -> Option<usize> {
        // Continuous EWMA update — never blind between decisions.
        let sample = if degraded { 1.0 } else { 0.0 };
        self.ewma = self.alpha * sample + (1.0 - self.alpha) * self.ewma;
        self.samples_since_decision += 1;
        if degraded {
            self.degraded_since_decision += 1;
        }

        // Fast-trip path: a burst of degradation mid-interval triggers an
        // immediate shrink (still honoring back-to-back cooldown).
        if self.degraded_since_decision >= self.fast_trip_threshold {
            self.reset_decision_window();
            if self.shrink_cooldown {
                self.shrink_cooldown = false;
                return None;
            }
            return Some(self.shrink_step());
        }

        // Normal decision boundary.
        if self.samples_since_decision >= self.decision_interval {
            let ewma = self.ewma;
            self.reset_decision_window();
            if ewma > 0.3 {
                if self.shrink_cooldown {
                    self.shrink_cooldown = false;
                    return None;
                }
                return Some(self.shrink_step());
            } else if ewma < 0.05 {
                return Some(self.grow_step());
            } else {
                // Dead band: hold the line, but clear cooldown so a real
                // spike afterwards can react without delay.
                self.shrink_cooldown = false;
                return None;
            }
        }

        None
    }
}

/// Single shared concurrency state between the publish pipeline and the AIMD
/// controller. Permits are forgotten on shrink and added back on grow.
#[derive(Clone)]
pub(super) struct ConcurrencyState {
    sem: Arc<Semaphore>,
    target: Arc<AtomicUsize>,
    forget_pending: Arc<AtomicUsize>,
    aimd: Arc<Mutex<AimdController>>,
    /// Unix-ms timestamp of the most recent `record()`. Drives the stall
    /// watchdog: if this stops moving, no put is resolving (success or
    /// failure), which usually means AIMD has wedged itself low.
    last_record_ms: Arc<AtomicU64>,
    /// Unix-ms timestamp of the most recent watchdog kick. Used to
    /// rate-limit kicks so a genuinely overloaded link can settle.
    last_kick_ms: Arc<AtomicU64>,
}

impl ConcurrencyState {
    fn new(initial: usize, max_cap: Option<usize>) -> Self {
        Self {
            sem: Arc::new(Semaphore::new(initial)),
            target: Arc::new(AtomicUsize::new(initial)),
            forget_pending: Arc::new(AtomicUsize::new(0)),
            aimd: Arc::new(Mutex::new(AimdController::new(initial, max_cap))),
            last_record_ms: Arc::new(AtomicU64::new(now_ms())),
            last_kick_ms: Arc::new(AtomicU64::new(0)),
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
        self.last_record_ms.store(now_ms(), Ordering::Relaxed);
        let new_target = {
            let mut ctrl = self.aimd.lock().await;
            ctrl.record(degraded)
        };
        if let Some(target) = new_target {
            self.apply_target(target);
        }
    }

    /// Watchdog entry. If no put has resolved in `stall_threshold` and we
    /// haven't kicked recently, ask AIMD to lift off the floor and rebalance
    /// permits. Returns the new target on a successful kick (for logging).
    async fn kick_if_stalled(
        &self,
        stall_threshold: Duration,
        min_kick_interval: Duration,
    ) -> Option<usize> {
        let now = now_ms();
        let since_record = now.saturating_sub(self.last_record_ms.load(Ordering::Relaxed));
        if since_record < stall_threshold.as_millis() as u64 {
            return None;
        }
        let since_kick = now.saturating_sub(self.last_kick_ms.load(Ordering::Relaxed));
        if since_kick < min_kick_interval.as_millis() as u64 {
            return None;
        }
        let new_target = {
            let mut ctrl = self.aimd.lock().await;
            ctrl.kick_stall()
        };
        if let Some(target) = new_target {
            self.last_kick_ms.store(now, Ordering::Relaxed);
            // Refresh the record clock so we don't immediately re-kick while
            // the new permits work their way through the system.
            self.last_record_ms.store(now, Ordering::Relaxed);
            self.apply_target(target);
        }
        new_target
    }

    fn apply_target(&self, target: usize) {
        let current_target = self.target.load(Ordering::Relaxed);
        match target.cmp(&current_target) {
            std::cmp::Ordering::Greater => {
                self.sem.add_permits(target - current_target);
                self.target.store(target, Ordering::Relaxed);
            }
            std::cmp::Ordering::Less => {
                self.forget_pending
                    .fetch_add(current_target - target, Ordering::Relaxed);
                self.target.store(target, Ordering::Relaxed);
            }
            std::cmp::Ordering::Equal => {}
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
pub(super) enum PublishUnit {
    /// An immutable data chunk (`immutable_put`).
    Data { encoded: Vec<u8> },
    /// A signed mutable index chunk (`mutable_put`).
    Index { keypair: KeyPair, encoded: Vec<u8> },
}

/// Long-lived dispatcher: pull from the queue, acquire a permit, spawn the
/// put. Exits cleanly on `shutdown`.
async fn dispatcher(
    handle: HyperDhtHandle,
    queue: Arc<WorkQueue>,
    state: ConcurrencyState,
    shutdown: Arc<Notify>,
) {
    loop {
        let pop_fut = queue.pop();
        tokio::pin!(pop_fut);
        let (id, unit, _subs) = tokio::select! {
            _ = shutdown.notified() => break,
            r = &mut pop_fut => r,
        };
        let permit = state.acquire().await;
        let h = handle.clone();
        let st = state.clone();
        let q = queue.clone();
        tokio::spawn(async move {
            let (degraded, bytes, is_data) = match unit {
                PublishUnit::Data { encoded } => {
                    let len = encoded.len() as u64;
                    let r = tokio::time::timeout(PUT_TIMEOUT, h.immutable_put(&encoded)).await;
                    let degraded = match r {
                        Ok(Ok(_)) => false,
                        Ok(Err(_)) | Err(_) => true,
                    };
                    (degraded, len, true)
                }
                PublishUnit::Index { keypair, encoded } => {
                    let r = tokio::time::timeout(PUT_TIMEOUT, put_mutable(&h, &keypair, &encoded))
                        .await;
                    let degraded = match r {
                        Ok(Ok(d)) => d,
                        Ok(Err(_)) | Err(_) => true,
                    };
                    (degraded, 0, false)
                }
            };
            st.record(degraded).await;
            q.mark_done(id, bytes, is_data).await;
            drop(permit);
        });
    }
}

/// Enqueue every non-root chunk of `tree` against `op` on the given lane.
async fn enqueue_tree_non_root(
    queue: &WorkQueue,
    tree: &BuiltTree,
    lane: Lane,
    op: &Operation,
) {
    for (i, c) in tree.data_chunks.iter().enumerate() {
        queue
            .enqueue(
                ChunkId::Data(i),
                PublishUnit::Data {
                    encoded: c.encoded.clone(),
                },
                lane,
                op.subscriber(),
            )
            .await;
    }
    for (i, c) in tree.index_chunks.iter().enumerate() {
        queue
            .enqueue(
                ChunkId::Index(i),
                PublishUnit::Index {
                    keypair: c.keypair.clone(),
                    encoded: c.encoded.clone(),
                },
                lane,
                op.subscriber(),
            )
            .await;
    }
}

/// Enqueue only the chunks listed in `data_idx` / `index_idx` (need-list
/// response). Always uses the High lane.
async fn enqueue_partial(
    queue: &WorkQueue,
    tree: &BuiltTree,
    data_idx: &[usize],
    index_idx: &[usize],
    op: &Operation,
) {
    for &i in data_idx {
        let c = &tree.data_chunks[i];
        queue
            .enqueue(
                ChunkId::Data(i),
                PublishUnit::Data {
                    encoded: c.encoded.clone(),
                },
                Lane::High,
                op.subscriber(),
            )
            .await;
    }
    for &i in index_idx {
        let c = &tree.index_chunks[i];
        queue
            .enqueue(
                ChunkId::Index(i),
                PublishUnit::Index {
                    keypair: c.keypair.clone(),
                    encoded: c.encoded.clone(),
                },
                Lane::High,
                op.subscriber(),
            )
            .await;
    }
}

/// Enqueue the root index chunk.
async fn enqueue_root(queue: &WorkQueue, tree: &BuiltTree, lane: Lane, op: &Operation) {
    queue
        .enqueue(
            ChunkId::Root,
            PublishUnit::Index {
                keypair: tree.root_keypair.clone(),
                encoded: tree.root_encoded.clone(),
            },
            lane,
            op.subscriber(),
        )
        .await;
}

/// Per-peer state tracked by the need-watcher. Dedups by value-hash so a
/// receiver's 10-minute keepalive republish (identical content, bumped
/// seq) doesn't trigger a duplicate service.
#[derive(Default)]
struct PeerState {
    /// Hash of the last value bytes we serviced (or empty-marker for done).
    last_value_hash: Option<[u8; 32]>,
    /// True once we've observed the empty-need-list "done" sentinel; we
    /// stop fetching from this peer thereafter.
    completed: bool,
    /// Last seq we observed — informational, only used for log clarity.
    last_seq: Option<u64>,
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

/// Background task: poll the need topic and enqueue chunks as receivers
/// request them. Dedups by per-peer value hash so identical keepalive
/// republishes from the receiver cost only a single `mutable_get`.
async fn run_need_watcher(
    handle: HyperDhtHandle,
    tree: Arc<BuiltTree>,
    queue: Arc<WorkQueue>,
    need_topic_key: [u8; 32],
    op_factory: OperationFactory,
    shutdown: Arc<Notify>,
) {
    let mut peers: HashMap<[u8; 32], PeerState> = HashMap::new();
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
                        let pk_short = to_hex(&peer.public_key);
                        let entry = peers.entry(peer.public_key).or_insert_with(|| {
                            eprintln!(
                                "  need-list peer discovered: {}",
                                &pk_short[..8]
                            );
                            PeerState::default()
                        });
                        if entry.completed {
                            continue;
                        }
                        let mv = match handle.mutable_get(&peer.public_key, 0).await {
                            Ok(Some(v)) => v,
                            Ok(None) => continue,
                            Err(e) => {
                                eprintln!(
                                    "  warning: need-list get from {} failed: {e}",
                                    &pk_short[..8]
                                );
                                continue;
                            }
                        };
                        let value_hash = hash_bytes(&mv.value);
                        if entry.last_value_hash == Some(value_hash) {
                            // Same content as last time we serviced — keepalive
                            // republish from the receiver. Skip.
                            entry.last_seq = Some(mv.seq);
                            continue;
                        }
                        let entries = match decode_need_list(&mv.value) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!(
                                    "  warning: malformed need-list from {}: {e}",
                                    &pk_short[..8]
                                );
                                continue;
                            }
                        };
                        if entries.is_empty() {
                            entry.completed = true;
                            entry.last_value_hash = Some(value_hash);
                            entry.last_seq = Some(mv.seq);
                            eprintln!(
                                "  need-list peer {} signaled done",
                                &pk_short[..8]
                            );
                            continue;
                        }
                        let resp = response_chunks_for_list(&tree, &entries);
                        let n_data = resp.data_chunk_indices.len();
                        let n_index = resp.index_chunk_indices.len();
                        eprintln!(
                            "  need-list received from {} (seq {}): {} data + {} index chunks to republish",
                            &pk_short[..8],
                            mv.seq,
                            n_data,
                            n_index
                        );
                        let bytes_total: u64 = resp
                            .data_chunk_indices
                            .iter()
                            .map(|&i| tree.data_chunks[i].encoded.len() as u64)
                            .sum();
                        let handle_op = op_factory.begin_operation(
                            bytes_total,
                            n_index as u32,
                            n_data as u32,
                        );
                        let op = Operation::new(handle_op.state(), n_data + n_index);
                        enqueue_partial(
                            &queue,
                            &tree,
                            &resp.data_chunk_indices,
                            &resp.index_chunk_indices,
                            &op,
                        )
                        .await;
                        // Mark as serviced on enqueue (not completion) — a
                        // failed put causes AIMD shrink, the receiver
                        // times out and publishes a fresh seq with the
                        // still-missing set, which we'll see as a new
                        // value hash and service again.
                        entry.last_value_hash = Some(value_hash);
                        entry.last_seq = Some(mv.seq);
                        op.await_done().await;
                        handle_op.finish().await;
                        eprintln!(
                            "  need-list republish complete: {n_data} data + {n_index} index"
                        );
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
    let initial_concurrency = 128usize;
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
    let state = ProgressState::new_with_wire(Phase::Put, 2, filename, handle.wire_counters());
    state.set_length(
        data.len() as u64,
        (tree.index_chunks.len() + 1) as u32, // include root
        tree.data_chunks.len() as u32,
    );
    let mut reporter = ProgressReporter::from_args(state.clone(), args.no_progress, args.json);
    reporter.on_start();

    // 8. Spawn the dispatcher and run the initial publish through the queue.
    let queue = WorkQueue::new();
    let dispatcher_shutdown = Arc::new(Notify::new());
    let dispatcher_handle = tokio::spawn(dispatcher(
        handle.clone(),
        queue.clone(),
        conc.clone(),
        dispatcher_shutdown.clone(),
    ));

    // Stall watchdog: if no put has resolved in 30s, kick AIMD off the floor.
    // Rate-limited to once per 2 min so a genuinely overloaded link can settle
    // at its true ceiling rather than oscillating around the kick target.
    let watchdog_shutdown = Arc::new(Notify::new());
    let watchdog_shutdown_inner = watchdog_shutdown.clone();
    let watchdog_conc = conc.clone();
    let watchdog_handle = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.tick().await;
        loop {
            tokio::select! {
                _ = watchdog_shutdown_inner.notified() => break,
                _ = tick.tick() => {
                    if let Some(t) = watchdog_conc
                        .kick_if_stalled(Duration::from_secs(30), Duration::from_secs(120))
                        .await
                    {
                        eprintln!("  stall watchdog: AIMD kicked → target {t}");
                    }
                }
            }
        }
    });

    // Initial publish: non-root chunks first (data + index layers), then
    // the root last. The "root last" rule is the only ordering constraint
    // in v3: until the root is published, no other pubkey is derivable.
    let non_root_count = tree.data_chunks.len() + tree.index_chunks.len();
    let initial_op = Operation::new(state.clone(), non_root_count);
    enqueue_tree_non_root(&queue, &tree, Lane::Normal, &initial_op).await;
    initial_op.await_done().await;

    let root_op = Operation::new(state.clone(), 1);
    enqueue_root(&queue, &tree, Lane::Normal, &root_op).await;
    root_op.await_done().await;

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
    let op_factory = reporter.operation_factory();
    let watcher_handle = tokio::spawn(run_need_watcher(
        handle.clone(),
        tree_arc.clone(),
        queue.clone(),
        need_topic_key,
        op_factory.clone(),
        watcher_shutdown.clone(),
    ));

    // 11. Refresh + ack loop.
    let ack_topic_key = ack_topic(&tree_arc.root_keypair.public_key);
    let mut seen_acks: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
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
                let bytes_total: u64 = tree_arc
                    .data_chunks
                    .iter()
                    .map(|c| c.encoded.len() as u64)
                    .sum();
                let idx_total = (tree_arc.index_chunks.len() + 1) as u32;
                let data_total = tree_arc.data_chunks.len() as u32;
                let total_chunks = tree_arc.index_chunks.len() + tree_arc.data_chunks.len() + 1;
                let handle_op = op_factory.begin_operation(bytes_total, idx_total, data_total);
                let op = Operation::new(handle_op.state(), total_chunks);
                // Concurrent refresh ticks coalesce naturally: chunks still
                // queued or in flight from a prior tick attach this op as a
                // subscriber rather than producing a duplicate put.
                enqueue_tree_non_root(&queue, &tree_arc, Lane::Normal, &op).await;
                enqueue_root(&queue, &tree_arc, Lane::Normal, &op).await;
                op.await_done().await;
                handle_op.finish().await;
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
    dispatcher_shutdown.notify_one();
    let _ = dispatcher_handle.await;
    watchdog_shutdown.notify_one();
    let _ = watchdog_handle.await;
    eprintln!("  stopped refreshing; records expire in ~20m");
    reporter.finish().await;
    let _ = handle.destroy().await;
    let _ = task.await;
    exit_code
}
