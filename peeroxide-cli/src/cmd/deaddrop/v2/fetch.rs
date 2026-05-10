//! v3 receiver: BFS fetch over the index tree with mmap output (`--output`)
//! or streaming stdout output.
//!
//! Spec: see *Fetch Protocol (Receiver)* in `DEADDROP_V3.md`.

#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use peeroxide::KeyPair;
use peeroxide_dht::hyperdht::HyperDhtHandle;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;

use crate::cmd::deaddrop::progress::reporter::ProgressReporter;
use crate::cmd::deaddrop::progress::state::ProgressState;

use super::super::GetArgs;
use super::keys::{ack_topic, need_topic};
use super::need::{coalesce_missing_ranges, encode_need_list};
use super::stream::StreamSink;
use super::tree::{compute_layout, data_chunk_count};
use super::wire::{
    decode_data_chunk, decode_non_root_index, decode_root_index, NON_ROOT_INDEX_SLOT_CAP,
};
use super::PARALLEL_FETCH_CAP;

/// Per-task fetch result variants.
enum TaskOutcome {
    Index {
        remaining_depth: u32,
        base: u64,
        end: u64,
        result: Result<Vec<u8>, String>,
    },
    Data {
        position: u64,
        result: Result<Vec<u8>, String>,
    },
}

/// Output destination strategy.
enum OutputSink {
    /// Memory-mapped output file (write-by-position).
    File {
        mmap: memmap2::MmapMut,
        temp_path: std::path::PathBuf,
        final_path: std::path::PathBuf,
    },
    /// Empty output file: no mmap, just create-and-rename at finalize.
    EmptyFile {
        temp_path: std::path::PathBuf,
        final_path: std::path::PathBuf,
    },
    /// Streaming stdout via reorder buffer.
    Stdout(StreamSink),
    /// Empty stdout: write nothing.
    EmptyStdout,
}

impl OutputSink {
    /// Accept a data chunk's payload at its file-order position.
    /// Returns Err if I/O fails.
    fn accept(&mut self, position: u64, payload: &[u8]) -> Result<(), String> {
        match self {
            OutputSink::File { mmap, .. } => {
                use super::wire::DATA_PAYLOAD_MAX;
                let offset = (position * DATA_PAYLOAD_MAX as u64) as usize;
                if offset + payload.len() > mmap.len() {
                    return Err(format!(
                        "chunk at position {position} extends past mmap end"
                    ));
                }
                mmap[offset..offset + payload.len()].copy_from_slice(payload);
                Ok(())
            }
            OutputSink::Stdout(sink) => {
                let to_emit = sink.accept(position, payload.to_vec());
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                for bytes in to_emit {
                    out.write_all(&bytes)
                        .map_err(|e| format!("stdout write failed: {e}"))?;
                }
                out.flush().map_err(|e| format!("stdout flush failed: {e}"))?;
                Ok(())
            }
            OutputSink::EmptyFile { .. } | OutputSink::EmptyStdout => {
                // Nothing to write — empty-file callers shouldn't pass any chunks
                // (N=0 means no data layer). Be permissive: just no-op.
                Ok(())
            }
        }
    }

    /// Finalize the output (flush mmap + atomic rename, or no-op for stdout).
    fn finalize(self) -> Result<(), String> {
        match self {
            OutputSink::File {
                mmap,
                temp_path,
                final_path,
            } => {
                mmap.flush().map_err(|e| format!("mmap flush failed: {e}"))?;
                drop(mmap);
                std::fs::rename(&temp_path, &final_path)
                    .map_err(|e| format!("rename to {final_path:?} failed: {e}"))?;
                Ok(())
            }
            OutputSink::EmptyFile {
                temp_path,
                final_path,
            } => {
                // Create an empty file at temp_path, then rename.
                std::fs::write(&temp_path, [])
                    .map_err(|e| format!("failed to write empty temp file: {e}"))?;
                std::fs::rename(&temp_path, &final_path)
                    .map_err(|e| format!("rename to {final_path:?} failed: {e}"))?;
                Ok(())
            }
            OutputSink::Stdout(sink) => {
                use std::io::Write;
                let _ = sink; // ensure consumed
                std::io::stdout()
                    .flush()
                    .map_err(|e| format!("stdout flush failed: {e}"))?;
                Ok(())
            }
            OutputSink::EmptyStdout => Ok(()),
        }
    }

    /// Discard the output without committing (used on error before finalize).
    fn discard(self) {
        match self {
            OutputSink::File {
                mmap, temp_path, ..
            } => {
                drop(mmap);
                let _ = std::fs::remove_file(&temp_path);
            }
            OutputSink::EmptyFile { temp_path, .. } => {
                let _ = std::fs::remove_file(&temp_path);
            }
            OutputSink::Stdout(_) | OutputSink::EmptyStdout => {}
        }
    }
}

/// Build the appropriate `OutputSink` for the user's request.
fn open_output_sink(args: &GetArgs, file_size: u64) -> Result<OutputSink, String> {
    use super::wire::DATA_PAYLOAD_MAX;
    if let Some(path) = args.output.as_ref() {
        let path = std::path::PathBuf::from(path);
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf();
        let temp_name = format!(".peeroxide-pickup-{}", std::process::id());
        let temp_path = dir.join(temp_name);

        if file_size == 0 {
            return Ok(OutputSink::EmptyFile {
                temp_path,
                final_path: path,
            });
        }

        // Allocate output file. We size it to N * DATA_PAYLOAD_MAX so that
        // each chunk writes to its position * 998 byte offset; the last
        // chunk may overshoot file_size by up to 998 bytes. We truncate
        // to file_size before rename.
        let n = data_chunk_count(file_size);
        let alloc_size = (n.saturating_mul(DATA_PAYLOAD_MAX as u64)).max(file_size);

        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|e| format!("failed to open temp file {temp_path:?}: {e}"))?;
        file.set_len(alloc_size)
            .map_err(|e| format!("failed to size temp file: {e}"))?;
        let mmap = unsafe {
            memmap2::MmapMut::map_mut(&file).map_err(|e| format!("mmap failed: {e}"))?
        };
        drop(file); // mmap holds the underlying mapping
        Ok(OutputSink::File {
            mmap,
            temp_path,
            final_path: path,
        })
    } else if file_size == 0 {
        Ok(OutputSink::EmptyStdout)
    } else {
        let n = data_chunk_count(file_size);
        Ok(OutputSink::Stdout(StreamSink::new(n)))
    }
}

/// Fetch a single mutable record with exponential backoff, bounded by `deadline`.
async fn fetch_mutable_with_retry(
    handle: &HyperDhtHandle,
    pk: &[u8; 32],
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>, String> {
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(15);
    loop {
        match handle.mutable_get(pk, 0).await {
            Ok(Some(r)) => return Ok(r.value),
            Ok(None) => {}
            Err(e) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(format!("mutable_get failed: {e}"));
                }
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("timeout".to_string());
        }
        let sleep = backoff.min(deadline.saturating_duration_since(now));
        tokio::time::sleep(sleep).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Fetch a single immutable record (data chunk) with exponential backoff.
async fn fetch_immutable_with_retry(
    handle: &HyperDhtHandle,
    address: &[u8; 32],
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>, String> {
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(15);
    loop {
        match handle.immutable_get(*address).await {
            Ok(Some(bytes)) => return Ok(bytes),
            Ok(None) => {}
            Err(e) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(format!("immutable_get failed: {e}"));
                }
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("timeout".to_string());
        }
        let sleep = backoff.min(deadline.saturating_duration_since(now));
        tokio::time::sleep(sleep).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Receiver-side need-list keepalive: announces the receiver's ephemeral
/// keypair on the need topic on a refresh cycle while the get is in
/// progress.
async fn run_need_announcer(
    handle: HyperDhtHandle,
    need_topic_key: [u8; 32],
    need_kp: KeyPair,
    shutdown: Arc<tokio::sync::Notify>,
) {
    let interval = Duration::from_secs(60);
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = async {
                if let Err(e) = handle.announce(need_topic_key, &need_kp, &[]).await {
                    eprintln!("  warning: need-topic announce failed: {e}");
                }
                tokio::time::sleep(interval).await;
            } => {}
        }
    }
}

/// Top-level GET entry point. Already given the fetched root chunk bytes
/// from `mod.rs::run_get` (which had to read the version byte to dispatch).
#[allow(clippy::too_many_arguments)]
pub async fn get_from_root(
    root_data: Vec<u8>,
    root_pk: [u8; 32],
    handle: HyperDhtHandle,
    task_handle: tokio::task::JoinHandle<
        Result<(), peeroxide_dht::hyperdht::HyperDhtError>,
    >,
    args: &GetArgs,
    progress: Arc<ProgressState>,
    reporter: ProgressReporter,
) -> i32 {
    if args.timeout == 0 {
        eprintln!("error: --timeout must be greater than 0");
        return cleanup(handle, task_handle, reporter, None, 1).await;
    }

    // 1. Decode the root index chunk.
    let root = match decode_root_index(&root_data) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: invalid root index chunk: {e}");
            return cleanup(handle, task_handle, reporter, None, 1).await;
        }
    };
    let layout = compute_layout(root.file_size);
    let n = layout.data_chunk_count;
    let tree_depth = layout.depth;

    // Sanity: root.slots should match the canonical layer 0 (data direct) or
    // top-non-root layer (root's children) shape.
    let expected_root_slots: u64 = if tree_depth == 0 {
        n
    } else {
        *layout.layer_counts.last().unwrap()
    };
    if root.slots.len() as u64 != expected_root_slots {
        eprintln!(
            "error: root slot count mismatch: got {}, expected {} (file_size={}, depth={})",
            root.slots.len(),
            expected_root_slots,
            root.file_size,
            tree_depth
        );
        return cleanup(handle, task_handle, reporter, None, 1).await;
    }

    // 2. Update progress state with totals.
    let total_index_chunks = super::tree::total_non_root_index_chunks(root.file_size) + 1;
    progress.set_length(root.file_size, total_index_chunks as u32, n as u32);
    progress.inc_index(); // root accounted for

    // 3. Open output sink.
    let mut output = match open_output_sink(args, root.file_size) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            return cleanup(handle, task_handle, reporter, None, 1).await;
        }
    };

    // 4. BFS fetch.
    let chunk_timeout = Duration::from_secs(args.timeout);
    let deadline = tokio::time::Instant::now() + chunk_timeout;
    let sem = Arc::new(Semaphore::new(PARALLEL_FETCH_CAP));
    let mut tasks: JoinSet<TaskOutcome> = JoinSet::new();
    let seen_index = Arc::new(Mutex::new(HashSet::<[u8; 32]>::new()));
    seen_index.lock().await.insert(root_pk);

    // Schedule all of root's children first (or root data slots if depth 0).
    schedule_children_from_index(
        &handle,
        &mut tasks,
        sem.clone(),
        root.slots.clone(),
        tree_depth,
        0,
        n,
        deadline,
    )
    .await;

    // 5. Setup need-list keepalive.
    let need_kp = KeyPair::generate();
    let need_topic_key = need_topic(&root_pk);
    let need_shutdown = Arc::new(tokio::sync::Notify::new());
    let need_announce_handle = tokio::spawn(run_need_announcer(
        handle.clone(),
        need_topic_key,
        need_kp.clone(),
        need_shutdown.clone(),
    ));
    let mut need_seq: u64 = 0;
    let mut received_data: HashSet<u64> = HashSet::new();
    let mut last_need_publish = tokio::time::Instant::now();
    let need_publish_interval = Duration::from_secs(20);

    // 6. Drain results.
    let mut had_error = false;
    while !tasks.is_empty() {
        let outcome = match tokio::time::timeout(Duration::from_secs(1), tasks.join_next()).await {
            Ok(Some(joined)) => match joined {
                Ok(o) => Some(o),
                Err(e) => {
                    eprintln!("  warning: fetch task panicked: {e}");
                    None
                }
            },
            Ok(None) => break,
            Err(_) => None,
        };

        if let Some(outcome) = outcome {
            match outcome {
                TaskOutcome::Index {
                    remaining_depth,
                    base,
                    end,
                    result,
                } => match result {
                    Ok(bytes) => {
                        match decode_non_root_index(&bytes) {
                            Ok(slots) => {
                                progress.inc_index();
                                let mut seen = seen_index.lock().await;
                                // No-op for loop detection; we already
                                // de-duplicate at schedule time below.
                                let _ = &mut *seen;
                                drop(seen);
                                schedule_children_from_index(
                                    &handle,
                                    &mut tasks,
                                    sem.clone(),
                                    slots,
                                    remaining_depth,
                                    base,
                                    end,
                                    deadline,
                                )
                                .await;
                            }
                            Err(e) => {
                                eprintln!(
                                    "error: invalid non-root index at base={base}: {e}"
                                );
                                had_error = true;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("error: failed to fetch index chunk: {e}");
                        had_error = true;
                        break;
                    }
                },
                TaskOutcome::Data { position, result } => match result {
                    Ok(bytes) => match decode_data_chunk(&bytes) {
                        Ok(payload) => {
                            // Trim payload for the last chunk if necessary.
                            let trim_len = if (position + 1) * super::wire::DATA_PAYLOAD_MAX as u64
                                > root.file_size
                            {
                                let already = position * super::wire::DATA_PAYLOAD_MAX as u64;
                                (root.file_size - already) as usize
                            } else {
                                payload.len()
                            };
                            let trimmed = &payload[..trim_len.min(payload.len())];
                            if let Err(e) = output.accept(position, trimmed) {
                                eprintln!("error: {e}");
                                had_error = true;
                                break;
                            }
                            progress.inc_data(trimmed.len() as u64);
                            received_data.insert(position);
                        }
                        Err(e) => {
                            eprintln!("error: invalid data chunk at position {position}: {e}");
                            had_error = true;
                            break;
                        }
                    },
                    Err(e) => {
                        eprintln!(
                            "  warning: failed to fetch data chunk at position {position}: {e}"
                        );
                        // Continue — we may republish via need-list and retry.
                    }
                },
            }
        }

        // Periodically publish need-list for missing data positions.
        if tokio::time::Instant::now() - last_need_publish >= need_publish_interval {
            let mut missing: Vec<u32> = (0..n as u32)
                .filter(|p| !received_data.contains(&(*p as u64)))
                .collect();
            missing.sort_unstable();
            if !missing.is_empty() && missing.len() < n as usize {
                let entries = coalesce_missing_ranges(&missing);
                let encoded = encode_need_list(&entries);
                need_seq += 1;
                let _ = handle.mutable_put(&need_kp, &encoded, need_seq).await;
            }
            last_need_publish = tokio::time::Instant::now();
        }

        // Timeout check.
        if tokio::time::Instant::now() >= deadline {
            eprintln!("error: timeout waiting for chunks");
            had_error = true;
            break;
        }
    }

    // 7. Finalize.
    need_shutdown.notify_one();
    let _ = need_announce_handle.await;

    if had_error {
        output.discard();
        return cleanup(handle, task_handle, reporter, Some(need_kp), 1).await;
    }

    // Verify all data positions arrived.
    if (received_data.len() as u64) != n {
        eprintln!(
            "error: only {} of {} data chunks received",
            received_data.len(),
            n
        );
        output.discard();
        return cleanup(handle, task_handle, reporter, Some(need_kp), 1).await;
    }

    // CRC verification: read back from output (only meaningful for File mode;
    // streaming stdout has emitted bytes already).
    if let Err(e) = verify_crc(&output, root.file_size, root.crc32c) {
        eprintln!("error: {e}");
        output.discard();
        return cleanup(handle, task_handle, reporter, Some(need_kp), 1).await;
    }

    if let OutputSink::File { temp_path, .. } = &output {
        // Truncate the temp file to file_size before rename.
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(temp_path) {
            let _ = file.set_len(root.file_size);
        }
    }

    if let Err(e) = output.finalize() {
        eprintln!("error: {e}");
        return cleanup(handle, task_handle, reporter, Some(need_kp), 1).await;
    }

    // Send empty need-list as the done sentinel, plus an ack.
    need_seq += 1;
    let _ = handle.mutable_put(&need_kp, &[], need_seq).await;
    if !args.no_ack {
        let ack = ack_topic(&root_pk);
        let ack_kp = KeyPair::generate();
        let _ = handle.announce(ack, &ack_kp, &[]).await;
    }

    cleanup(handle, task_handle, reporter, Some(need_kp), 0).await
}

#[allow(clippy::too_many_arguments)]
async fn schedule_children_from_index(
    handle: &HyperDhtHandle,
    tasks: &mut JoinSet<TaskOutcome>,
    sem: Arc<Semaphore>,
    slots: Vec<[u8; 32]>,
    remaining_depth: u32,
    base: u64,
    end: u64,
    deadline: tokio::time::Instant,
) {
    if remaining_depth == 0 {
        // Slots are data hashes. Position[i] = base + i.
        for (i, address) in slots.into_iter().enumerate() {
            let pos = base + i as u64;
            if pos >= end {
                break;
            }
            let h = handle.clone();
            let permit_sem = sem.clone();
            tasks.spawn(async move {
                let _permit = permit_sem.acquire_owned().await.unwrap();
                let result = fetch_immutable_with_retry(&h, &address, deadline).await;
                TaskOutcome::Data {
                    position: pos,
                    result,
                }
            });
        }
        return;
    }

    // Slots are child index pubkeys. Each child covers a subtree.
    // Subtree size at remaining_depth r = NON_ROOT_INDEX_SLOT_CAP^r.
    let child_remaining = remaining_depth - 1;
    let mut subtree_size: u64 = 1;
    for _ in 0..=child_remaining {
        subtree_size = subtree_size.saturating_mul(NON_ROOT_INDEX_SLOT_CAP as u64);
    }

    let mut child_base = base;
    for (i, child_pk) in slots.into_iter().enumerate() {
        if child_base >= end {
            break;
        }
        // Last child of a parent may have a smaller range (due to N being
        // less than the full canonical capacity at this layer). Compute
        // the child's end as min(child_base + subtree_size, end).
        let child_end = (child_base + subtree_size).min(end);
        let h = handle.clone();
        let permit_sem = sem.clone();
        tasks.spawn(async move {
            let _permit = permit_sem.acquire_owned().await.unwrap();
            let result = fetch_mutable_with_retry(&h, &child_pk, deadline).await;
            TaskOutcome::Index {
                remaining_depth: child_remaining,
                base: child_base,
                end: child_end,
                result,
            }
        });
        child_base = child_end;
        let _ = i; // suppress unused
    }
}

/// CRC-verify the reassembled output. For File mode, reads the mmap; for
/// Stdout mode, this is a no-op (bytes are downstream already). For empty
/// outputs, verifies that `expected_crc` matches `crc32c(&[])`.
fn verify_crc(output: &OutputSink, file_size: u64, expected_crc: u32) -> Result<(), String> {
    match output {
        OutputSink::File { mmap, .. } => {
            let bytes = &mmap[..file_size as usize];
            let computed = crc32c::crc32c(bytes);
            if computed != expected_crc {
                return Err(format!(
                    "CRC mismatch: expected {expected_crc:08x}, got {computed:08x}"
                ));
            }
        }
        OutputSink::EmptyFile { .. } | OutputSink::EmptyStdout => {
            let computed = crc32c::crc32c(&[]);
            if computed != expected_crc {
                return Err(format!(
                    "CRC mismatch on empty file: expected {expected_crc:08x}, got {computed:08x}"
                ));
            }
        }
        OutputSink::Stdout(_) => {
            // Streaming has already emitted; CRC mismatch is best-effort.
            // We don't recompute (would require buffering the entire file).
        }
    }
    Ok(())
}

/// Cleanup helper: drains DHT handle, awaits the runtime task, finishes the
/// reporter, and returns the exit code.
async fn cleanup(
    handle: HyperDhtHandle,
    task_handle: tokio::task::JoinHandle<
        Result<(), peeroxide_dht::hyperdht::HyperDhtError>,
    >,
    reporter: ProgressReporter,
    _need_kp: Option<KeyPair>,
    code: i32,
) -> i32 {
    reporter.finish().await;
    let _ = handle.destroy().await;
    let _ = task_handle.await;
    code
}

#[cfg(test)]
mod tests {
    // Most fetch.rs logic requires a running DHT; integration tests cover
    // the end-to-end roundtrip in `peeroxide-cli/tests/local_commands.rs`.
}
