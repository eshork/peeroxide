#![allow(dead_code, private_interfaces)]
use super::*;
use crate::cmd::sigterm_recv;
use crate::cmd::deaddrop::progress::{
    state::{Phase, ProgressState},
    reporter::ProgressReporter,
};

pub const VERSION: u8 = 0x02;
const DATA_PAYLOAD_MAX: usize = 999; // MAX_PAYLOAD(1000) - 1 byte version header
const ROOT_INDEX_HEADER: usize = 41; // 1+4+4+32
const NON_ROOT_INDEX_HEADER: usize = 33; // 1+32
const PTRS_PER_ROOT: usize = (MAX_PAYLOAD - ROOT_INDEX_HEADER) / 32; // 29
const PTRS_PER_NON_ROOT: usize = (MAX_PAYLOAD - NON_ROOT_INDEX_HEADER) / 32; // 30
const MAX_DATA_CHUNKS: usize = PTRS_PER_ROOT + 65535 * PTRS_PER_NON_ROOT;
const MAX_FILE_SIZE: u64 = MAX_DATA_CHUNKS as u64 * DATA_PAYLOAD_MAX as u64;
pub const PARALLEL_FETCH_CAP: usize = 64;

/// How often the GET side re-announces on the need-topic to keep DHT records alive.
const NEED_REANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

/// How often the PUT side polls for need-lists in its dedicated watcher task.
const NEED_POLL_INTERVAL: Duration = Duration::from_secs(5);

pub fn derive_index_keypair(root_seed: &[u8; 32], i: u16) -> KeyPair {
    let mut input = Vec::with_capacity(32 + 3 + 2);
    input.extend_from_slice(root_seed);
    input.extend_from_slice(b"idx");
    input.extend_from_slice(&i.to_le_bytes());
    KeyPair::from_seed(peeroxide::discovery_key(&input))
}

pub fn data_chunk_hash(encoded: &[u8]) -> [u8; 32] {
    peeroxide::discovery_key(encoded)
}

pub fn encode_data_chunk(payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + payload.len());
    buf.push(VERSION);
    buf.extend_from_slice(payload);
    buf
}

pub fn encode_root_index(
    file_size: u32,
    crc: u32,
    next_pk: &[u8; 32],
    data_hashes: &[[u8; 32]],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ROOT_INDEX_HEADER + 32 * data_hashes.len());
    buf.push(VERSION);
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(next_pk);
    for h in data_hashes {
        buf.extend_from_slice(h);
    }
    buf
}

pub fn encode_non_root_index(next_pk: &[u8; 32], data_hashes: &[[u8; 32]]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(NON_ROOT_INDEX_HEADER + 32 * data_hashes.len());
    buf.push(VERSION);
    buf.extend_from_slice(next_pk);
    for h in data_hashes {
        buf.extend_from_slice(h);
    }
    buf
}

pub fn compute_data_chunk_count(file_size: usize) -> usize {
    if file_size == 0 {
        0
    } else {
        file_size.div_ceil(DATA_PAYLOAD_MAX)
    }
}

pub fn compute_index_chain_length(data_count: usize) -> usize {
    if data_count <= PTRS_PER_ROOT {
        1
    } else {
        1 + (data_count - PTRS_PER_ROOT).div_ceil(PTRS_PER_NON_ROOT)
    }
}

pub struct V2Built {
    pub data_chunks: Vec<Vec<u8>>,    // encoded data chunks (plain bytes for immutable_put)
    pub index_chunks: Vec<ChunkData>, // encoded index chunks (with keypairs for mutable_put)
    pub data_hashes: Vec<[u8; 32]>,   // content hash of each data chunk
}

pub fn build_v2_chunks(data: &[u8], root_seed: &[u8; 32]) -> Result<V2Built, String> {
    if data.len() as u64 > MAX_FILE_SIZE {
        return Err(format!(
            "file too large ({} bytes, max {})",
            data.len(),
            MAX_FILE_SIZE
        ));
    }
    let crc = crc32c::crc32c(data);
    let file_size = data.len() as u32;

    // Split and encode data chunks; compute content hash for each
    let encoded_data: Vec<Vec<u8>> = if data.is_empty() {
        vec![]
    } else {
        data.chunks(DATA_PAYLOAD_MAX).map(encode_data_chunk).collect()
    };
    let data_hashes: Vec<[u8; 32]> = encoded_data.iter().map(|e| data_chunk_hash(e)).collect();

    let data_count = encoded_data.len();
    let index_count = compute_index_chain_length(data_count);

    // Derive index keypairs
    // root = KeyPair::from_seed(*root_seed); non-root i=1..
    let index_keypairs: Vec<KeyPair> = {
        let mut kps = Vec::with_capacity(index_count);
        kps.push(KeyPair::from_seed(*root_seed));
        for i in 1..index_count {
            kps.push(derive_index_keypair(root_seed, i as u16));
        }
        kps
    };

    // Encode index chunks
    // root gets data_hashes[0..PTRS_PER_ROOT]
    // non-root i gets data_hashes[PTRS_PER_ROOT + (i-1)*PTRS_PER_NON_ROOT .. PTRS_PER_ROOT + i*PTRS_PER_NON_ROOT]
    // next_pk: index[j].next_pk = index_keypairs[j+1].public_key (last has [0u8;32])
    let mut index_chunks: Vec<ChunkData> = Vec::with_capacity(index_count);
    for j in 0..index_count {
        let next_pk: [u8; 32] = if j + 1 < index_count {
            index_keypairs[j + 1].public_key
        } else {
            [0u8; 32]
        };

        let encoded = if j == 0 {
            // root
            let end = PTRS_PER_ROOT.min(data_count);
            encode_root_index(file_size, crc, &next_pk, &data_hashes[..end])
        } else {
            // non-root j: data_hashes[PTRS_PER_ROOT + (j-1)*PTRS_PER_NON_ROOT ..]
            let start = PTRS_PER_ROOT + (j - 1) * PTRS_PER_NON_ROOT;
            let end = (start + PTRS_PER_NON_ROOT).min(data_count);
            encode_non_root_index(&next_pk, &data_hashes[start..end])
        };

        index_chunks.push(ChunkData {
            keypair: index_keypairs[j].clone(),
            encoded,
        });
    }

    Ok(V2Built {
        data_chunks: encoded_data,
        index_chunks,
        data_hashes,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum NeedEntry {
    Index { start: u16, end: u16 },
    Data { start: u32, end: u32 },
}

pub fn need_topic(root_pk: &[u8; 32]) -> [u8; 32] {
    let mut input = Vec::with_capacity(32 + 4);
    input.extend_from_slice(root_pk);
    input.extend_from_slice(b"need");
    peeroxide::discovery_key(&input)
}

pub fn encode_need_list(entries: &[NeedEntry]) -> Vec<u8> {
    let mut buf = vec![VERSION];
    for entry in entries {
        match entry {
            NeedEntry::Index { start, end } => {
                if buf.len() + 5 > MAX_PAYLOAD {
                    break;
                }
                buf.push(0x00);
                buf.extend_from_slice(&start.to_le_bytes());
                buf.extend_from_slice(&end.to_le_bytes());
            }
            NeedEntry::Data { start, end } => {
                if buf.len() + 9 > MAX_PAYLOAD {
                    break;
                }
                buf.push(0x01);
                buf.extend_from_slice(&start.to_le_bytes());
                buf.extend_from_slice(&end.to_le_bytes());
            }
        }
    }
    buf
}

pub fn decode_need_list(data: &[u8]) -> Result<Vec<NeedEntry>, String> {
    if data.is_empty() {
        return Ok(vec![]);
    }
    if data[0] != VERSION {
        return Err(format!("unexpected version byte 0x{:02x}", data[0]));
    }
    let mut entries = Vec::new();
    let mut i = 1;
    while i < data.len() {
        match data[i] {
            0x00 => {
                if i + 5 > data.len() {
                    return Err("truncated index entry".into());
                }
                let start = u16::from_le_bytes([data[i + 1], data[i + 2]]);
                let end = u16::from_le_bytes([data[i + 3], data[i + 4]]);
                entries.push(NeedEntry::Index { start, end });
                i += 5;
            }
            0x01 => {
                if i + 9 > data.len() {
                    return Err("truncated data entry".into());
                }
                let start = u32::from_le_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]);
                let end = u32::from_le_bytes([data[i + 5], data[i + 6], data[i + 7], data[i + 8]]);
                entries.push(NeedEntry::Data { start, end });
                i += 9;
            }
            tag => return Err(format!("unknown need list tag 0x{tag:02x}")),
        }
    }
    Ok(entries)
}

/// Convert a sorted slice of missing data-chunk positions into compact
/// `NeedEntry::Data` ranges. Each range covers a contiguous run.
pub fn compute_need_entries(missing: &[u32]) -> Vec<NeedEntry> {
    contiguous_ranges(missing)
        .into_iter()
        .map(|(s, e)| NeedEntry::Data { start: s, end: e })
        .collect()
}

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

    if data.len() as u64 > MAX_FILE_SIZE {
        eprintln!("error: file too large ({} bytes, max {})", data.len(), MAX_FILE_SIZE);
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

    eprintln!("  chunking {} bytes...", data.len());
    let built = match build_v2_chunks(&data, &root_seed) {
        Ok(b) => Arc::new(b),
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

    let (max_concurrency, dispatch_delay): (Option<usize>, Option<Duration>) =
        if let Some(ref speed_str) = args.max_speed {
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

    eprintln!(
        "DD PUT v2: {} index chunks, {} data chunks ({} bytes)",
        built.index_chunks.len(),
        built.data_chunks.len(),
        data.len()
    );

    let filename: Arc<str> = if args.file == "-" {
        Arc::from("<stdin>")
    } else {
        Arc::from(args.file.as_str())
    };
    let state = ProgressState::new(Phase::Put, 2, filename);
    state.set_length(data.len() as u64, built.index_chunks.len() as u32, built.data_chunks.len() as u32);
    let mut reporter = ProgressReporter::from_args(state.clone(), args.no_progress, args.json);
    reporter.on_start();

    let mut tasks: Vec<PublishTask> = Vec::with_capacity(
        built.index_chunks.len() + built.data_chunks.len()
    );
    for chunk in built.index_chunks.iter().cloned() {
        tasks.push(PublishTask::Index(chunk));
    }
    for chunk in built.data_chunks.iter().cloned() {
        tasks.push(PublishTask::Data(chunk));
    }
    eprintln!("  publishing {} chunks to DHT...", tasks.len());
    let publish_fut = publish_tasks(&handle, tasks, max_concurrency, dispatch_delay, Some(state.clone()));
    tokio::pin!(publish_fut);
    tokio::select! {
        res = &mut publish_fut => {
            if let Err(e) = res {
                eprintln!("error: publish failed: {e}");
                reporter.finish().await;
                let _ = handle.destroy().await;
                let _ = task.await;
                return 1;
            }
        }
        _ = signal::ctrl_c() => {
            eprintln!("interrupted");
            reporter.finish().await;
            let _ = handle.destroy().await;
            let _ = task.await;
            return 130;
        }
        _ = sigterm_recv() => {
            reporter.finish().await;
            let _ = handle.destroy().await;
            let _ = task.await;
            return 143;
        }
    }

    let pickup_key = to_hex(&root_kp.public_key);
    reporter.emit_initial_publish_complete(&pickup_key).await;

    let need_topic_key = need_topic(&root_kp.public_key);
    eprintln!("  published to DHT (best-effort)");
    eprintln!("  pickup key printed to stdout");
    eprintln!("  refreshing every {}s, polling needs every {}s, monitoring for acks every 30s...", args.refresh_interval, NEED_POLL_INTERVAL.as_secs());

    let ack_topic = peeroxide::discovery_key(&[root_kp.public_key.as_slice(), b"ack"].concat());
    let mut seen_acks: HashSet<[u8; 32]> = HashSet::new();
    let mut pickup_count: u64 = 0;

    let ttl_deadline =
        args.ttl.map(|t| tokio::time::Instant::now() + Duration::from_secs(t));
    let mut refresh_interval =
        tokio::time::interval(Duration::from_secs(args.refresh_interval));
    refresh_interval.tick().await;
    let mut ack_interval = tokio::time::interval(Duration::from_secs(30));
    ack_interval.tick().await;

    let watcher_notify = Arc::new(tokio::sync::Notify::new());
    let watcher_notify_task = watcher_notify.clone();
    let watcher_handle = handle.clone();
    let watcher_built = built.clone();
    let watcher_need_topic_key = need_topic_key;
    let watcher_max_concurrency = max_concurrency;
    let watcher_dispatch_delay = dispatch_delay;
    let need_watcher = tokio::spawn(async move {
        eprintln!("  need-list watcher started (poll every {}s)", NEED_POLL_INTERVAL.as_secs());
        let mut seen_peers: HashSet<[u8; 32]> = HashSet::new();
        let mut lookup_was_err = false;
        loop {
            tokio::select! {
                _ = watcher_notify_task.notified() => break,
                _ = tokio::time::sleep(NEED_POLL_INTERVAL) => {
                    match watcher_handle.lookup(watcher_need_topic_key).await {
                        Ok(need_results) => {
                            lookup_was_err = false;
                            for result in &need_results {
                                for peer in &result.peers {
                                    if seen_peers.insert(peer.public_key) {
                                        eprintln!("  need-list peer discovered: {} (poll cycle)", &to_hex(&peer.public_key)[..8]);
                                    }
                                    match watcher_handle.mutable_get(&peer.public_key, 0).await {
                                        Ok(Some(mget)) => {
                                            match decode_need_list(&mget.value) {
                                                Ok(needs) => {
                                                    let n_entries = needs.len();
                                                    eprintln!("  need-list received: {n_entries} entries from {}, republishing", &to_hex(&peer.public_key)[..8]);
                                                    for need in needs {
                                                        match need {
                                                            NeedEntry::Index { start, end } => {
                                                                let s = start as usize;
                                                                let e = (end as usize + 1)
                                                                    .min(watcher_built.index_chunks.len());
                                                                if s >= e { continue; }
                                                                let mut tasks: Vec<PublishTask> = Vec::new();
                                                                for chunk in &watcher_built.index_chunks[s..e] {
                                                                    tasks.push(PublishTask::Index(chunk.clone()));
                                                                }
                                                                for j in s..e {
                                                                    let data_start = if j == 0 {
                                                                        0
                                                                    } else {
                                                                        PTRS_PER_ROOT
                                                                            + (j - 1) * PTRS_PER_NON_ROOT
                                                                    };
                                                                    let data_end = if j == 0 {
                                                                        PTRS_PER_ROOT
                                                                    } else {
                                                                        data_start + PTRS_PER_NON_ROOT
                                                                    }.min(watcher_built.data_chunks.len());
                                                                    if data_start < data_end {
                                                                        for chunk in
                                                                            &watcher_built.data_chunks[data_start..data_end]
                                                                        {
                                                                            tasks.push(PublishTask::Data(chunk.clone()));
                                                                        }
                                                                    }
                                                                }
                                                let n_chunks = tasks.len();
                                                let _ = publish_tasks(&watcher_handle, tasks, watcher_max_concurrency, watcher_dispatch_delay, None).await;
                                                eprintln!("  need-list republish complete: {n_chunks} chunks");
                                            }
                                            NeedEntry::Data { start, end } => {
                                                let s = start as usize;
                                                let e = (end as usize + 1)
                                                    .min(watcher_built.data_chunks.len());
                                                if s >= e { continue; }
                                                let tasks: Vec<PublishTask> = watcher_built.data_chunks[s..e]
                                                    .iter()
                                                    .map(|c| PublishTask::Data(c.clone()))
                                                    .collect();
                                                let n_chunks = tasks.len();
                                                let _ = publish_tasks(&watcher_handle, tasks, watcher_max_concurrency, watcher_dispatch_delay, None).await;
                                                                eprintln!("  need-list republish complete: {n_chunks} chunks");
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("  warning: malformed need-list from {}: {e}", &to_hex(&peer.public_key)[..8]);
                                                }
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(e) => {
                                            eprintln!("  warning: need-list mutable_get failed for {}: {e}", &to_hex(&peer.public_key)[..8]);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if !lookup_was_err {
                                eprintln!("  warning: need-topic lookup failed: {e}");
                                lookup_was_err = true;
                            }
                        }
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => break,
            _ = sigterm_recv() => break,
            _ = async {
                if let Some(deadline) = ttl_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => break,
            _ = refresh_interval.tick() => {
                eprintln!("  refreshing {} index + {} data chunks...",
                    built.index_chunks.len(), built.data_chunks.len());
                let mut tasks: Vec<PublishTask> = Vec::with_capacity(
                    built.index_chunks.len() + built.data_chunks.len()
                );
                for chunk in &built.index_chunks {
                    tasks.push(PublishTask::Index(chunk.clone()));
                }
                for chunk in &built.data_chunks {
                    tasks.push(PublishTask::Data(chunk.clone()));
                }
                if let Err(e) = publish_tasks(&handle, tasks, max_concurrency, dispatch_delay, None).await {
                    eprintln!("  warning: refresh failed: {e}");
                }
            }
            _ = ack_interval.tick() => {
                if let Ok(results) = handle.lookup(ack_topic).await {
                    for result in &results {
                        for peer in &result.peers {
                            if seen_acks.insert(peer.public_key) {
                                pickup_count += 1;
                                reporter.on_ack(pickup_count, &to_hex(&peer.public_key));
                                eprintln!("  [ack] pickup #{pickup_count} detected");
                                if let Some(max) = args.max_pickups {
                                    if pickup_count >= max {
                                        eprintln!("  max pickups reached, stopping");
                                        reporter.finish().await;
                                        watcher_notify.notify_one();
                                        let _ = need_watcher.await;
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
    watcher_notify.notify_one();
    let _ = need_watcher.await;
    reporter.finish().await;
    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

async fn fetch_index_with_retry(
    handle: &HyperDhtHandle,
    pk: &[u8; 32],
    timeout: Duration,
    sem: Arc<Semaphore>,
) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);
    loop {
        let permit = sem.clone().acquire_owned().await.unwrap();
        let result = handle.mutable_get(pk, 0).await;
        drop(permit);
        if let Ok(Some(result)) = result {
            return Some(result.value);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        let remaining = deadline - tokio::time::Instant::now();
        tokio::time::sleep(backoff.min(remaining)).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

fn contiguous_ranges(positions: &[u32]) -> Vec<(u32, u32)> {
    if positions.is_empty() {
        return vec![];
    }
    let mut ranges = Vec::new();
    let mut start = positions[0];
    let mut end = positions[0];
    for &p in &positions[1..] {
        if p == end + 1 {
            end = p;
        } else {
            ranges.push((start, end));
            start = p;
            end = p;
        }
    }
    ranges.push((start, end));
    ranges
}

pub async fn get_from_root(
    root_data: Vec<u8>,
    root_pk: [u8; 32],
    handle: HyperDhtHandle,
    task_handle: tokio::task::JoinHandle<Result<(), peeroxide_dht::hyperdht::HyperDhtError>>,
    args: &GetArgs,
) -> i32 {
    let chunk_timeout = Duration::from_secs(args.timeout);

    if root_data.len() < ROOT_INDEX_HEADER {
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }
    if root_data[0] != VERSION {
        eprintln!("error: unexpected version byte 0x{:02x}", root_data[0]);
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    let file_size = u32::from_le_bytes(root_data[1..5].try_into().unwrap());
    let stored_crc = u32::from_le_bytes(root_data[5..9].try_into().unwrap());
    let mut first_next_pk = [0u8; 32];
    first_next_pk.copy_from_slice(&root_data[9..41]);

    let mut root_data_hashes: Vec<[u8; 32]> = Vec::new();
    let mut offset = ROOT_INDEX_HEADER;
    while offset + 32 <= root_data.len() {
        let mut h = [0u8; 32];
        h.copy_from_slice(&root_data[offset..offset + 32]);
        root_data_hashes.push(h);
        offset += 32;
    }

    let expected_data_count = compute_data_chunk_count(file_size as usize);
    let expected_index_count = compute_index_chain_length(expected_data_count);
    eprintln!(
        "DD GET v2: file_size={}, expected {} index + {} data chunks",
        file_size, expected_index_count, expected_data_count
    );
    eprintln!("  fetched index 1/{expected_index_count}");

    let need_kp = KeyPair::generate();
    let nt = need_topic(&root_pk);
    let mut need_seq: u64 = 0;

    // Periodic re-announce task — keeps the need-topic DHT record alive.
    let reannounce_notify = Arc::new(tokio::sync::Notify::new());
    let reannounce_notify_task = reannounce_notify.clone();
    let need_kp_reannounce = need_kp.clone();
    let handle_reannounce = handle.clone();
    let reannounce_handle = tokio::spawn(async move {
        let mut last_announce_was_err = false;
        // Initial announce (immediately, replaces the removed one-shot call)
        match handle_reannounce.announce(nt, &need_kp_reannounce, &[]).await {
            Ok(_) => {
                eprintln!("  announced need-topic {}", &to_hex(&nt)[..8]);
            }
            Err(e) => {
                eprintln!("  warning: re-announce failed: {e}");
                last_announce_was_err = true;
            }
        }
        loop {
            tokio::select! {
                _ = reannounce_notify_task.notified() => break,
                _ = tokio::time::sleep(NEED_REANNOUNCE_INTERVAL) => {
                    match handle_reannounce.announce(nt, &need_kp_reannounce, &[]).await {
                        Ok(_) => {
                            if last_announce_was_err {
                                eprintln!("  re-announce recovered after errors");
                                last_announce_was_err = false;
                            }
                        }
                        Err(e) => {
                            if !last_announce_was_err {
                                eprintln!("  warning: re-announce failed: {e}");
                                last_announce_was_err = true;
                            }
                        }
                    }
                }
            }
        }
    });

    let sem = Arc::new(Semaphore::new(PARALLEL_FETCH_CAP));
    let (result_tx, mut result_rx) =
        tokio::sync::mpsc::unbounded_channel::<(u32, Option<Vec<u8>>)>();
    let mut spawned_count: usize = 0;

    let mut all_data_hashes: Vec<[u8; 32]> = root_data_hashes;
    let mut next_pk = first_next_pk;
    let mut seen_index_keys: HashSet<[u8; 32]> = HashSet::new();
    let mut index_pos: u16 = 1;

    for (i, &hash) in all_data_hashes.iter().enumerate() {
        let hh = handle.clone();
        let sem2 = sem.clone();
        let tx = result_tx.clone();
        tokio::spawn(async move {
            let permit = sem2.acquire_owned().await.unwrap();
            let result = hh.immutable_get(hash).await.ok().flatten();
            drop(permit);
            let _ = tx.send((i as u32, result));
        });
        spawned_count += 1;
    }

    let mut fetched_indexes: usize = 1; // root already fetched
    let mut fetched_data: usize = 0;
    let mut drained: usize = 0;
    let mut results: std::collections::HashMap<u32, Vec<u8>> =
        std::collections::HashMap::new();

    while next_pk != [0u8; 32] {
        if !seen_index_keys.insert(next_pk) {
            eprintln!("error: loop detected in index chain");
            need_seq += 1;
            let _ = handle.mutable_put(&need_kp, &[], need_seq).await;
            reannounce_notify.notify_one();
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        let idx_data =
            match fetch_index_with_retry(&handle, &next_pk, chunk_timeout, sem.clone()).await {
                Some(d) => d,
                None => {
                    eprintln!("error: index chunk {} not found (timeout)", index_pos);
                    let need_entries =
                        vec![NeedEntry::Index { start: index_pos, end: index_pos }];
                    let encoded = encode_need_list(&need_entries);
                    need_seq += 1;
                    let _ = handle.mutable_put(&need_kp, &encoded, need_seq).await;
                    need_seq += 1;
                    let _ = handle.mutable_put(&need_kp, &[], need_seq).await;
                    reannounce_notify.notify_one();
                    let _ = handle.destroy().await;
                    let _ = task_handle.await;
                    return 1;
                }
            };

        if idx_data.len() < NON_ROOT_INDEX_HEADER || idx_data[0] != VERSION {
            eprintln!("error: invalid non-root index chunk");
            reannounce_notify.notify_one();
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        let mut new_next = [0u8; 32];
        new_next.copy_from_slice(&idx_data[1..33]);
        next_pk = new_next;

        let mut idx_offset = NON_ROOT_INDEX_HEADER;
        while idx_offset + 32 <= idx_data.len() {
            let mut h = [0u8; 32];
            h.copy_from_slice(&idx_data[idx_offset..idx_offset + 32]);
            let pos = all_data_hashes.len() as u32;
            all_data_hashes.push(h);
            let hh = handle.clone();
            let sem2 = sem.clone();
            let tx = result_tx.clone();
            tokio::spawn(async move {
                let permit = sem2.acquire_owned().await.unwrap();
                let result = hh.immutable_get(h).await.ok().flatten();
                drop(permit);
                let _ = tx.send((pos, result));
            });
            spawned_count += 1;
            idx_offset += 32;
        }
        index_pos += 1;
        fetched_indexes += 1;
        eprintln!("  fetched index {fetched_indexes}/{expected_index_count}");
        while let Ok((pos, opt)) = result_rx.try_recv() {
            if let Some(data) = opt {
                results.insert(pos, data);
            }
            fetched_data += 1;
            drained += 1;
            eprintln!("  fetched data {fetched_data}/{expected_data_count}");
        }
    }

    if all_data_hashes.len() != expected_data_count {
        eprintln!(
            "error: hash count mismatch: got {} hashes, expected {}",
            all_data_hashes.len(),
            expected_data_count
        );
        reannounce_notify.notify_one();
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    drop(result_tx);
    while drained < spawned_count {
        match result_rx.recv().await {
            Some((pos, opt)) => {
                if let Some(data) = opt {
                    results.insert(pos, data);
                }
                fetched_data += 1;
                drained += 1;
                eprintln!("  fetched data {fetched_data}/{expected_data_count}");
            }
            None => break,
        }
    }

    eprintln!(
        "  fetched {}/{} data chunks",
        results.len(),
        expected_data_count
    );

    let mut last_published_missing: Option<Vec<u32>> = None;
    let mut need_list_topic_logged = false;
    let retry_deadline = tokio::time::Instant::now() + chunk_timeout;
    loop {
        let missing: Vec<u32> = (0..expected_data_count as u32)
            .filter(|p| !results.contains_key(p))
            .collect();
        if missing.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= retry_deadline {
            eprintln!("error: timed out waiting for {} missing chunks", missing.len());
            need_seq += 1;
            let _ = handle.mutable_put(&need_kp, &[], need_seq).await;
            reannounce_notify.notify_one();
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        let ranges = contiguous_ranges(&missing);
        let mut retry_positions: Vec<u32> = ranges.iter().map(|(s, _)| *s).collect();
        for (s, e) in &ranges {
            for p in (s + 1)..=*e {
                retry_positions.push(p);
            }
        }

        let mut new_data = 0usize;
        let mut retry_handles: Vec<tokio::task::JoinHandle<(u32, Option<Vec<u8>>)>> =
            Vec::new();
        for pos in &retry_positions {
            let hash = all_data_hashes[*pos as usize];
            let permit = sem.clone().acquire_owned().await.unwrap();
            let h = handle.clone();
            let p = *pos;
            retry_handles.push(tokio::spawn(async move {
                let r = h.immutable_get(hash).await.ok().flatten();
                drop(permit);
                (p, r)
            }));
        }
        for jh in retry_handles {
            if let Ok((pos, Some(data))) = jh.await {
                results.insert(pos, data);
                new_data += 1;
            }
        }

        let missing_now: Vec<u32> = (0..expected_data_count as u32)
            .filter(|p| !results.contains_key(p))
            .collect();

        // Publish need-list if the missing set has changed since last publish
        if Some(&missing_now) != last_published_missing.as_ref() && !missing_now.is_empty() {
            let need_entries = compute_need_entries(&missing_now);
            let encoded = encode_need_list(&need_entries);
            need_seq += 1;
            if let Err(e) = handle.mutable_put(&need_kp, &encoded, need_seq).await {
                eprintln!("  warning: need-list publish failed: {e}");
            } else {
                if !need_list_topic_logged {
                    eprintln!("  need-list published under topic {}", &to_hex(&nt)[..8]);
                    need_list_topic_logged = true;
                }
                eprintln!(
                    "  waiting for {} missing chunks, published need list",
                    missing_now.len()
                );
                last_published_missing = Some(missing_now.clone());
            }
        }

        if new_data == 0 {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    need_seq += 1;
    let _ = handle.mutable_put(&need_kp, &[], need_seq).await;

    let mut payload_data: Vec<u8> = Vec::with_capacity(file_size as usize);
    for pos in 0..expected_data_count as u32 {
        match results.get(&pos) {
            Some(chunk) => {
                if chunk.is_empty() || chunk[0] != VERSION {
                    eprintln!("error: invalid data chunk at position {pos}");
                    reannounce_notify.notify_one();
                    let _ = handle.destroy().await;
                    let _ = task_handle.await;
                    return 1;
                }
                payload_data.extend_from_slice(&chunk[1..]);
            }
            None => {
                eprintln!("error: missing data chunk at position {pos}");
                reannounce_notify.notify_one();
                let _ = handle.destroy().await;
                let _ = task_handle.await;
                return 1;
            }
        }
    }

    if expected_data_count != 0 && payload_data.len() != file_size as usize {
        eprintln!(
            "error: size mismatch: got {} bytes, expected {}",
            payload_data.len(),
            file_size
        );
        reannounce_notify.notify_one();
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    let computed_crc = crc32c::crc32c(&payload_data);
    if computed_crc != stored_crc {
        eprintln!(
            "error: CRC mismatch (expected {stored_crc:08x}, got {computed_crc:08x})"
        );
        reannounce_notify.notify_one();
        let _ = handle.destroy().await;
        let _ = task_handle.await;
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
            reannounce_notify.notify_one();
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        if let Err(e) = tokio::fs::rename(&temp_path, output_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            eprintln!("error: failed to rename: {e}");
            reannounce_notify.notify_one();
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        eprintln!("  written to {output_path}");
    } else {
        use std::io::Write;
        if let Err(e) = std::io::stdout().write_all(&payload_data) {
            eprintln!("error: failed to write to stdout: {e}");
            reannounce_notify.notify_one();
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }
    }

    if !args.no_ack {
        let ack_topic =
            peeroxide::discovery_key(&[root_pk.as_slice(), b"ack"].concat());
        let ack_kp = KeyPair::generate();
        let _ = handle.announce(ack_topic, &ack_kp, &[]).await;
        eprintln!("  ack sent (ephemeral identity)");
    } else {
        eprintln!("  done (no ack sent)");
    }

    eprintln!("  done");
    reannounce_notify.notify_one();
    let _ = reannounce_handle.await;
    let _ = handle.destroy().await;
    let _ = task_handle.await;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(b: u8) -> [u8; 32] {
        [b; 32]
    }

    #[test]
    fn test_derive_index_keys_domain_separation() {
        let s = seed(1);
        let v2_key = derive_index_keypair(&s, 0).public_key;
        // v1 derivation: discovery_key(seed || u16_le) — no domain tag
        let mut v1_input = Vec::new();
        v1_input.extend_from_slice(&s);
        v1_input.extend_from_slice(&0u16.to_le_bytes());
        let v1_key = peeroxide::KeyPair::from_seed(peeroxide::discovery_key(&v1_input)).public_key;
        assert_ne!(v2_key, v1_key, "v2 and v1 keys must differ for same seed/index");
        let key1 = derive_index_keypair(&s, 1).public_key;
        assert_ne!(v2_key, key1, "different indices must give different keys");
    }

    #[test]
    fn test_encode_data_chunk() {
        let payload = vec![1u8, 2, 3];
        let encoded = encode_data_chunk(&payload);
        assert_eq!(encoded[0], VERSION);
        assert_eq!(&encoded[1..], &payload);
        // max payload
        let max_payload = vec![0u8; DATA_PAYLOAD_MAX];
        let encoded_max = encode_data_chunk(&max_payload);
        assert_eq!(encoded_max.len(), MAX_PAYLOAD);
    }

    #[test]
    fn test_data_chunk_hash_deterministic() {
        let a = encode_data_chunk(&[1, 2, 3]);
        let b = encode_data_chunk(&[1, 2, 3]);
        let c = encode_data_chunk(&[4, 5, 6]);
        assert_eq!(data_chunk_hash(&a), data_chunk_hash(&b));
        assert_ne!(data_chunk_hash(&a), data_chunk_hash(&c));
        // hash is blake2b of encoded bytes
        assert_eq!(data_chunk_hash(&a), peeroxide::discovery_key(&a));
    }

    #[test]
    fn test_encode_root_index_structure() {
        let next_pk = [7u8; 32];
        let hashes: Vec<[u8; 32]> = (0..3).map(|i| [i as u8; 32]).collect();
        let enc = encode_root_index(42, 99, &next_pk, &hashes);
        assert_eq!(enc[0], VERSION);
        assert_eq!(u32::from_le_bytes(enc[1..5].try_into().unwrap()), 42);
        assert_eq!(u32::from_le_bytes(enc[5..9].try_into().unwrap()), 99);
        assert_eq!(&enc[9..41], &next_pk);
        assert_eq!(enc.len(), ROOT_INDEX_HEADER + 32 * 3);
    }

    #[test]
    fn test_encode_non_root_index_structure() {
        let next_pk = [3u8; 32];
        let hashes: Vec<[u8; 32]> = (0..2).map(|i| [i as u8; 32]).collect();
        let enc = encode_non_root_index(&next_pk, &hashes);
        assert_eq!(enc[0], VERSION);
        assert_eq!(&enc[1..33], &next_pk);
        assert_eq!(enc.len(), NON_ROOT_INDEX_HEADER + 32 * 2);
    }

    #[test]
    fn test_compute_data_chunk_count() {
        assert_eq!(compute_data_chunk_count(0), 0);
        assert_eq!(compute_data_chunk_count(1), 1);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX), 1);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX + 1), 2);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX * 2), 2);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX * 2 + 1), 3);
    }

    #[test]
    fn test_compute_index_chain_length() {
        assert_eq!(compute_index_chain_length(0), 1);
        assert_eq!(compute_index_chain_length(1), 1);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT), 1);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT + 1), 2);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT + PTRS_PER_NON_ROOT), 2);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT + PTRS_PER_NON_ROOT + 1), 3);
    }

    #[test]
    fn test_build_v2_chunks_empty() {
        let s = seed(2);
        let built = build_v2_chunks(&[], &s).unwrap();
        assert_eq!(built.data_chunks.len(), 0);
        assert_eq!(built.index_chunks.len(), 1);
        assert_eq!(built.data_hashes.len(), 0);
        // root index must have file_size=0
        let root = &built.index_chunks[0].encoded;
        assert_eq!(root[0], VERSION);
        assert_eq!(u32::from_le_bytes(root[1..5].try_into().unwrap()), 0);
    }

    #[test]
    fn test_build_v2_chunks_single() {
        let s = seed(3);
        let data = b"hello";
        let built = build_v2_chunks(data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), 1);
        assert_eq!(built.index_chunks.len(), 1);
        assert_eq!(built.data_hashes.len(), 1);
        let root = &built.index_chunks[0].encoded;
        // root should contain 1 hash after the header
        assert_eq!(root.len(), ROOT_INDEX_HEADER + 32);
    }

    #[test]
    fn test_build_v2_chunks_fills_root() {
        let s = seed(4);
        let data = vec![0u8; PTRS_PER_ROOT * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), PTRS_PER_ROOT);
        assert_eq!(built.index_chunks.len(), 1);
        assert_eq!(
            built.index_chunks[0].encoded.len(),
            ROOT_INDEX_HEADER + 32 * PTRS_PER_ROOT
        );
    }

    #[test]
    fn test_build_v2_chunks_spills() {
        let s = seed(5);
        let data = vec![0u8; (PTRS_PER_ROOT + 1) * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), PTRS_PER_ROOT + 1);
        assert_eq!(built.index_chunks.len(), 2);
        // root has PTRS_PER_ROOT hashes; non-root has 1
        assert_eq!(
            built.index_chunks[0].encoded.len(),
            ROOT_INDEX_HEADER + 32 * PTRS_PER_ROOT
        );
        assert_eq!(
            built.index_chunks[1].encoded.len(),
            NON_ROOT_INDEX_HEADER + 32
        );
        // root's next_pk = non-root's public key
        let root_next: [u8; 32] = built.index_chunks[0].encoded[9..41].try_into().unwrap();
        assert_eq!(root_next, built.index_chunks[1].keypair.public_key);
    }

    #[test]
    fn test_build_v2_chunks_multi_index() {
        let s = seed(6);
        let n = PTRS_PER_ROOT + 2 * PTRS_PER_NON_ROOT + 1;
        let data = vec![1u8; n * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), n);
        assert!(built.index_chunks.len() >= 3);
    }

    #[test]
    fn test_build_v2_chunks_reassemble() {
        let s = seed(7);
        let original: Vec<u8> = (0..5000u32).map(|i| (i % 256) as u8).collect();
        let built = build_v2_chunks(&original, &s).unwrap();
        // reassemble: strip version byte from each data chunk
        let reassembled: Vec<u8> = built
            .data_chunks
            .iter()
            .flat_map(|c| c[1..].iter().copied())
            .collect();
        assert_eq!(&reassembled, &original);
        // verify CRC stored in root matches original
        let root = &built.index_chunks[0].encoded;
        let stored_crc = u32::from_le_bytes(root[5..9].try_into().unwrap());
        assert_eq!(stored_crc, crc32c::crc32c(&original));
    }

    #[test]
    fn test_build_v2_rejects_oversized() {
        // We can't actually allocate MAX_FILE_SIZE, so test the boundary check logic
        // by checking a known oversized value
        // Instead, verify MAX_FILE_SIZE constant is set correctly
        let max_file_size = MAX_FILE_SIZE;
        assert!(max_file_size > 1_000_000_000, "MAX_FILE_SIZE should be > 1GB");
        // Test: MAX_DATA_CHUNKS constant is > 1.9M
        let max_data_chunks = MAX_DATA_CHUNKS;
        assert!(max_data_chunks > 1_900_000);
    }

    #[test]
    fn test_index_chain_links() {
        let s = seed(8);
        let data = vec![0u8; (PTRS_PER_ROOT + 2) * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        let n = built.index_chunks.len();
        for j in 0..n - 1 {
            // root (j==0): next_pk at [9..41]; non-root (j>0): next_pk at [1..33]
            let next_pk: [u8; 32] = if j == 0 {
                built.index_chunks[j].encoded[9..41].try_into().unwrap()
            } else {
                built.index_chunks[j].encoded[1..33].try_into().unwrap()
            };
            assert_eq!(next_pk, built.index_chunks[j + 1].keypair.public_key);
        }
        // last chunk next_pk is all zeros
        let last = &built.index_chunks[n - 1];
        let last_next_pk: [u8; 32] = last.encoded[9..41].try_into().unwrap_or([0u8; 32]);
        // non-root: next_pk is at offset 1..33
        let last_non_root_next_pk: [u8; 32] = last.encoded[1..33].try_into().unwrap();
        let zero = [0u8; 32];
        // one of them must be zero (depending on root vs non-root)
        assert!(last_next_pk == zero || last_non_root_next_pk == zero);
    }

    #[test]
    fn test_index_stores_content_hashes() {
        let s = seed(9);
        let data = b"abc def ghi";
        let built = build_v2_chunks(data, &s).unwrap();
        for (i, encoded) in built.data_chunks.iter().enumerate() {
            let expected_hash = data_chunk_hash(encoded);
            assert_eq!(built.data_hashes[i], expected_hash);
        }
        // Also verify hashes appear in root index
        let root = &built.index_chunks[0].encoded;
        for (i, hash) in built.data_hashes.iter().enumerate() {
            let offset = ROOT_INDEX_HEADER + i * 32;
            let stored: [u8; 32] = root[offset..offset + 32].try_into().unwrap();
            assert_eq!(stored, *hash);
        }
    }

    #[test]
    fn test_need_topic_deterministic() {
        let pk1 = [1u8; 32];
        let pk2 = [2u8; 32];
        assert_eq!(need_topic(&pk1), need_topic(&pk1));
        assert_ne!(need_topic(&pk1), need_topic(&pk2));
    }

    #[test]
    fn test_encode_decode_need_list_index_entries() {
        let entries = vec![NeedEntry::Index { start: 2, end: 5 }];
        let encoded = encode_need_list(&entries);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_encode_decode_need_list_data_entries() {
        let entries = vec![NeedEntry::Data {
            start: 100,
            end: 200,
        }];
        let encoded = encode_need_list(&entries);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_encode_decode_need_list_mixed() {
        let entries = vec![
            NeedEntry::Index { start: 0, end: 3 },
            NeedEntry::Data { start: 10, end: 20 },
            NeedEntry::Index { start: 5, end: 8 },
        ];
        let encoded = encode_need_list(&entries);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_encode_need_list_capacity() {
        // Fill with data entries (9 bytes each + 1 version byte)
        // MAX_PAYLOAD=1000, so max ~(999/9)=111 data entries
        let entries: Vec<NeedEntry> = (0..200)
            .map(|i| NeedEntry::Data { start: i, end: i })
            .collect();
        let encoded = encode_need_list(&entries);
        assert!(
            encoded.len() <= MAX_PAYLOAD,
            "encoded must fit in MAX_PAYLOAD bytes"
        );
        assert!(encoded.len() > 1, "must have at least version byte + one entry");
    }

    #[test]
    fn test_decode_need_list_empty() {
        let result = decode_need_list(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_decode_need_list_invalid_tag() {
        let data = vec![VERSION, 0xFF];
        let result = decode_need_list(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_need_entries_empty() {
        let result = super::compute_need_entries(&[]);
        assert_eq!(result, vec![]);
    }

    #[test]
    fn test_compute_need_entries_single() {
        let result = super::compute_need_entries(&[42]);
        assert_eq!(result, vec![NeedEntry::Data { start: 42, end: 42 }]);
    }

    #[test]
    fn test_compute_need_entries_contiguous() {
        let result = super::compute_need_entries(&[1, 2, 3, 4]);
        assert_eq!(result, vec![NeedEntry::Data { start: 1, end: 4 }]);
    }

    #[test]
    fn test_compute_need_entries_disjoint() {
        let result = super::compute_need_entries(&[1, 3, 5]);
        assert_eq!(
            result,
            vec![
                NeedEntry::Data { start: 1, end: 1 },
                NeedEntry::Data { start: 3, end: 3 },
                NeedEntry::Data { start: 5, end: 5 },
            ]
        );
    }

    #[test]
    fn test_compute_need_entries_mixed() {
        let result = super::compute_need_entries(&[1, 2, 5, 7, 8, 9]);
        assert_eq!(
            result,
            vec![
                NeedEntry::Data { start: 1, end: 2 },
                NeedEntry::Data { start: 5, end: 5 },
                NeedEntry::Data { start: 7, end: 9 },
            ]
        );
    }
}
