//! Batched serial publisher for chat-channel messages.
//!
//! Owns `FeedState`, the feed-refresh task, and the rotation tick. Drains
//! a bounded mpsc of message jobs, accumulates each into a short window
//! (`batch_wait_ms`) up to `batch_size`, and publishes the whole batch
//! with a single chained set of network operations:
//!
//!   1. join_all immutable_put(message bytes) for every message in batch
//!      (plus the summary block ciphertext, if eviction fired mid-batch)
//!   2. mutable_put(FeedRecord, final seq) with up to 3 retries
//!   3. announce on the next per-batch bucket
//!
//! This eliminates the per-message `tokio::spawn` race that allowed the
//! old code to advertise a FeedRecord whose referenced immutable_puts
//! had not yet propagated, which manifested at the receiver as `[late]`
//! gap-timeout releases when the immutable_get of a missing predecessor
//! could not be satisfied within the 5s window.

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Duration;

use futures::future::join_all;
use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::feed::{self, FeedState};
use crate::cmd::chat::post::{prepare_one, Prepared};
use crate::cmd::chat::probe;

/// Jobs the publisher accepts on its inbound queue.
pub enum PubJob {
    /// A single text message to publish.
    Message(String),
}

static BATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Retry schedule for the per-batch `mutable_put`. The publisher first
/// fires the FeedRecord update; on failure it waits each successive delay
/// and retries. If all attempts fail the batch's messages are still in
/// the DHT (immutables succeeded) and the next successful batch will
/// re-advertise them via the chain, so this is loss-tolerant.
const MUTABLE_PUT_RETRY_MS: [u64; 3] = [200, 500, 1000];

/// Rotation check interval — mirrors the cadence of the old in-line tick
/// that lived in `join.rs`.
const ROTATION_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Run the publisher worker to completion.
///
/// On entry, performs the initial `mutable_put` of the empty FeedRecord
/// and spawns the periodic feed-refresh task. The worker exits cleanly
/// when `rx` is closed (i.e. all senders dropped), at which point the
/// feed-refresh task is aborted and the function returns.
#[allow(clippy::too_many_arguments)]
pub async fn run_publisher(
    handle: HyperDhtHandle,
    mut feed_state: FeedState,
    id_keypair: KeyPair,
    message_key: [u8; 32],
    channel_key: [u8; 32],
    screen_name: String,
    mut rx: mpsc::Receiver<PubJob>,
    batch_size: usize,
    batch_wait_ms: u64,
) {
    // Sanitize to non-pathological values.
    let batch_size = batch_size.max(1);
    let batch_wait = Duration::from_millis(batch_wait_ms);

    // --- Initial publish ---
    let initial_data = feed_state.serialize_feed_record();
    if let Err(e) = handle
        .mutable_put(&feed_state.feed_keypair, &initial_data, feed_state.seq)
        .await
    {
        eprintln!("warning: initial feed publish failed: {e}");
    }

    let (refresh_tx, refresh_rx) =
        watch::channel((initial_data.clone(), feed_state.seq));
    let mut refresh_handle: JoinHandle<()> = {
        let h = handle.clone();
        let kp = feed_state.feed_keypair.clone();
        tokio::spawn(async move {
            feed::run_feed_refresh(h, kp, refresh_rx).await;
        })
    };
    let mut refresh_tx = refresh_tx;

    let mut rotation_check = tokio::time::interval(ROTATION_CHECK_INTERVAL);
    rotation_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Burn the immediate first tick.
    rotation_check.tick().await;

    loop {
        tokio::select! {
            biased;
            // Rotation only fires when no inbound jobs are queued, so a
            // rotation never splits the await chain of a batch.
            _ = rotation_check.tick() => {
                if feed_state.needs_rotation() {
                    rotate_feed(
                        &handle,
                        &mut feed_state,
                        &mut refresh_tx,
                        &mut refresh_handle,
                    )
                    .await;
                }
            }
            maybe_first = rx.recv() => {
                let Some(first) = maybe_first else {
                    // All senders dropped — stdin closed, caller shutting down.
                    refresh_handle.abort();
                    return;
                };
                let mut texts: Vec<String> = Vec::with_capacity(batch_size);
                push_text(&mut texts, first);

                // Accumulate up to batch_size or batch_wait timeout.
                let deadline = Instant::now() + batch_wait;
                while texts.len() < batch_size {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, rx.recv()).await {
                        Ok(Some(job)) => push_text(&mut texts, job),
                        Ok(None) => break, // senders dropped
                        Err(_) => break,   // timeout
                    }
                }

                publish_batch(
                    &handle,
                    &mut feed_state,
                    &id_keypair,
                    &message_key,
                    &channel_key,
                    &screen_name,
                    &refresh_tx,
                    texts,
                )
                .await;
            }
        }
    }
}

fn push_text(texts: &mut Vec<String>, job: PubJob) {
    match job {
        PubJob::Message(text) => texts.push(text),
    }
}

/// Build, sign, encrypt every message in `texts` (chain-linked) then run
/// the immutable_put → mutable_put → announce pipeline serially.
#[allow(clippy::too_many_arguments)]
async fn publish_batch(
    handle: &HyperDhtHandle,
    feed_state: &mut FeedState,
    id_keypair: &KeyPair,
    message_key: &[u8; 32],
    channel_key: &[u8; 32],
    screen_name: &str,
    refresh_tx: &watch::Sender<(Vec<u8>, u64)>,
    texts: Vec<String>,
) {
    let batch_n = BATCH_COUNTER.fetch_add(1, AtomicOrdering::Relaxed) + 1;

    // --- Phase 1: synchronous chain construction ---
    let mut encrypted_blobs: Vec<Vec<u8>> = Vec::with_capacity(texts.len());
    let mut summary_blobs: Vec<Vec<u8>> = Vec::new();
    for text in &texts {
        match prepare_one(feed_state, id_keypair, message_key, screen_name, text) {
            Ok(Prepared {
                encrypted,
                summary_data,
                ..
            }) => {
                encrypted_blobs.push(encrypted);
                if let Some(s) = summary_data {
                    summary_blobs.push(s);
                }
            }
            Err(e) => {
                eprintln!("error: failed to prepare message: {e}");
            }
        }
    }

    if encrypted_blobs.is_empty() {
        return;
    }

    feed_state.seq += 1;
    let feed_record_data = feed_state.serialize_feed_record();
    let seq = feed_state.seq;
    let msg_count = feed_state.msg_count;
    let feed_kp = feed_state.feed_keypair.clone();
    let epoch = crypto::current_epoch();
    let bucket = feed_state.next_bucket();
    let topic = crypto::announce_topic(channel_key, epoch, bucket);

    if probe::is_enabled() {
        eprintln!(
            "[probe] batch#{batch_n} messages={} summary_blocks={} seq={seq}",
            encrypted_blobs.len(),
            summary_blobs.len(),
        );
    }

    // --- Phase 2: all immutable_puts in parallel; await all ---
    let put_start = Instant::now();
    let mut put_futures = Vec::with_capacity(encrypted_blobs.len() + summary_blobs.len());
    for blob in encrypted_blobs.iter().chain(summary_blobs.iter()) {
        let h = handle.clone();
        let bytes = blob.clone();
        put_futures.push(tokio::spawn(async move { h.immutable_put(&bytes).await }));
    }

    let put_results = join_all(put_futures).await;
    let mut put_failed = 0usize;
    for r in &put_results {
        match r {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                eprintln!("warning: immutable_put failed: {e}");
                put_failed += 1;
            }
            Err(e) => {
                eprintln!("warning: immutable_put task panicked: {e}");
                put_failed += 1;
            }
        }
    }
    if probe::is_enabled() {
        eprintln!(
            "[probe] batch#{batch_n} immutable_put_done elapsed_ms={} failed={}",
            put_start.elapsed().as_millis(),
            put_failed,
        );
    }

    // --- Phase 3: mutable_put with retry; only advertise after immutables ---
    let mut mput_attempts = 0usize;
    let mput_start = Instant::now();
    let mput_ok = loop {
        mput_attempts += 1;
        match handle.mutable_put(&feed_kp, &feed_record_data, seq).await {
            Ok(_) => {
                debug::log_event(
                    "Feed record update",
                    "mutable_put",
                    &format!(
                        "feed_pubkey={}, seq={seq}, msg_count={msg_count}",
                        debug::short_key(&feed_kp.public_key),
                    ),
                );
                break true;
            }
            Err(e) => {
                if let Some(delay_ms) = MUTABLE_PUT_RETRY_MS.get(mput_attempts - 1) {
                    eprintln!(
                        "warning: mutable_put failed (attempt {mput_attempts}/{}): {e}; retrying in {delay_ms}ms",
                        MUTABLE_PUT_RETRY_MS.len() + 1,
                    );
                    tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
                } else {
                    eprintln!(
                        "warning: mutable_put failed after {mput_attempts} attempts: {e}; batch's FeedRecord left unadvertised, next batch will re-advertise via chain"
                    );
                    break false;
                }
            }
        }
    };
    if probe::is_enabled() {
        eprintln!(
            "[probe] batch#{batch_n} mutable_put_done elapsed_ms={} attempts={mput_attempts} ok={mput_ok}",
            mput_start.elapsed().as_millis(),
        );
    }

    // Tell the feed-refresh task the new (data, seq) pair regardless of put
    // success — refresh will retry on its own cadence and a future success
    // is what users care about.
    let _ = refresh_tx.send((feed_record_data, seq));

    // --- Phase 4: announce ---
    let ann_start = Instant::now();
    let _ = handle.announce(topic, &feed_kp, &[]).await;
    debug::log_event(
        "Channel announce",
        "announce",
        &format!(
            "feed_pubkey={}, epoch={epoch}, bucket={bucket}, topic={}",
            debug::short_key(&feed_kp.public_key),
            debug::short_key(&topic),
        ),
    );
    if probe::is_enabled() {
        eprintln!(
            "[probe] batch#{batch_n} announce_done elapsed_ms={}",
            ann_start.elapsed().as_millis(),
        );
    }
}

/// Rotate the feed keypair, publishing the new feed first and then
/// updating the old feed with `next_feed_pubkey` so readers can follow.
async fn rotate_feed(
    handle: &HyperDhtHandle,
    feed_state: &mut FeedState,
    refresh_tx: &mut watch::Sender<(Vec<u8>, u64)>,
    refresh_handle: &mut JoinHandle<()>,
) {
    let mut new_fs = feed_state.rotate();

    let new_data = new_fs.serialize_feed_record();
    let new_kp = new_fs.feed_keypair.clone();
    let new_seq = new_fs.seq;

    if let Err(e) = handle.mutable_put(&new_kp, &new_data, new_seq).await {
        eprintln!("warning: feed rotation failed (new feed publish), will retry: {e}");
        // Roll back the pointer set during rotate() so we retry cleanly next tick.
        feed_state.next_feed_pubkey = [0u8; 32];
        return;
    }
    debug::log_event(
        "Feed rotation (new)",
        "mutable_put",
        &format!(
            "new_feed_pubkey={}, old_feed_pubkey={}",
            debug::short_key(&new_kp.public_key),
            debug::short_key(&feed_state.feed_keypair.public_key),
        ),
    );

    // Publish the old feed one last time so readers can discover the pointer.
    let old_record = feed_state.serialize_feed_record();
    feed_state.seq += 1;
    let old_seq = feed_state.seq;
    let old_kp = feed_state.feed_keypair.clone();
    if let Err(e) = handle.mutable_put(&old_kp, &old_record, old_seq).await {
        tracing::warn!("rotation: old feed update failed (non-fatal): {e}");
    } else {
        debug::log_event(
            "Feed rotation (old ptr)",
            "mutable_put",
            &format!(
                "old_feed_pubkey={}, seq={old_seq}, next_feed={}",
                debug::short_key(&old_kp.public_key),
                debug::short_key(&new_kp.public_key),
            ),
        );
    }

    // Spawn the overlap refresh so the old feed stays alive long enough
    // for in-flight readers to follow the pointer.
    let overlap_h = handle.clone();
    let overlap_kp = old_kp.clone();
    let overlap_data = old_record.clone();
    let overlap_seq = old_seq;
    tokio::spawn(async move {
        feed::run_rotation_overlap_refresh(overlap_h, overlap_kp, overlap_data, overlap_seq).await;
    });

    // Tear down the old refresh task and start a new one for the new feed.
    refresh_handle.abort();
    let (new_tx, new_rx) = watch::channel((new_data.clone(), new_seq));
    *refresh_tx = new_tx;
    *refresh_handle = {
        let h = handle.clone();
        let kp = new_kp.clone();
        tokio::spawn(async move {
            feed::run_feed_refresh(h, kp, new_rx).await;
        })
    };

    // Swap in the new state.
    std::mem::swap(feed_state, &mut new_fs);
    eprintln!("*** feed keypair rotated");
}
