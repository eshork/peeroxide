use std::collections::{HashMap, HashSet};

use futures::future::join_all;
use peeroxide_dht::crypto::hash;
use peeroxide_dht::hyperdht::HyperDhtHandle;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::display::DisplayMessage;
use crate::cmd::chat::known_users;
use crate::cmd::chat::ordering::{chain_sort, ChainGate, DedupRing, PendingMessage, SubmitOutcome};
use crate::cmd::chat::probe;
use crate::cmd::chat::wire::{self, FeedRecord, MessageEnvelope, SummaryBlock};

struct KnownFeed {
    id_pubkey: [u8; 32],
    last_seq: u64,
    last_msg_hash: [u8; 32],
    last_summary_hash_seen: [u8; 32],
    last_active: Instant,
    last_message_time: Instant,
    next_poll: Instant,
}

impl KnownFeed {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            id_pubkey: [0u8; 32],
            last_seq: 0,
            last_msg_hash: [0u8; 32],
            last_summary_hash_seen: [0u8; 32],
            last_active: now,
            last_message_time: now,
            next_poll: now,
        }
    }

    fn poll_interval(&self) -> Duration {
        let since_msg = self.last_message_time.elapsed().as_secs();
        match since_msg {
            0..=59 => Duration::from_secs(1),
            60..=119 => Duration::from_secs(2),
            120..=179 => Duration::from_secs(3),
            180..=300 => Duration::from_secs(5),
            _ => Duration::from_secs(10),
        }
    }

    fn schedule_next_poll(&mut self) {
        self.next_poll = Instant::now() + self.poll_interval();
    }
}

const MAX_SUMMARY_DEPTH: usize = 100;
const FEED_EXPIRY_SECS: u64 = 20 * 60;
const DISCOVERY_INTERVAL_SECS: u64 = 8;
const GAP_TIMEOUT: Duration = Duration::from_secs(5);
const REFETCH_SCHEDULE_MS: [u64; 4] = [0, 500, 1500, 3000];

struct RefetchResult {
    hash: [u8; 32],
    owner: [u8; 32],
    feed_pubkey: [u8; 32],
    data: Option<Vec<u8>>,
}

fn spawn_refetch(
    handle: HyperDhtHandle,
    hash: [u8; 32],
    owner: [u8; 32],
    feed_pubkey: [u8; 32],
    tx: mpsc::UnboundedSender<RefetchResult>,
) {
    tokio::spawn(async move {
        for delay_ms in REFETCH_SCHEDULE_MS {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            if let Ok(Some(data)) = handle.immutable_get(hash).await {
                let _ = tx.send(RefetchResult {
                    hash,
                    owner,
                    feed_pubkey,
                    data: Some(data),
                });
                return;
            }
        }
        let _ = tx.send(RefetchResult {
            hash,
            owner,
            feed_pubkey,
            data: None,
        });
    });
}

fn decode_envelope(
    message_key: &[u8; 32],
    data: &[u8],
    owner_pubkey: &[u8; 32],
) -> Option<MessageEnvelope> {
    let plaintext = wire::decrypt_message(message_key, data).ok()?;
    let env = MessageEnvelope::deserialize(&plaintext).ok()?;
    if !env.verify() || env.id_pubkey != *owner_pubkey {
        return None;
    }
    Some(env)
}

fn envelope_to_pending(
    env: MessageEnvelope,
    msg_hash: [u8; 32],
    feed_pubkey: [u8; 32],
    self_id_pubkey: &[u8; 32],
) -> PendingMessage {
    let prev_msg_hash = env.prev_msg_hash;
    let id_pubkey = env.id_pubkey;
    let is_self = id_pubkey == *self_id_pubkey;
    PendingMessage {
        display: DisplayMessage {
            id_pubkey,
            screen_name: env.screen_name,
            content: env.content,
            timestamp: env.timestamp,
            is_self,
            late: false,
        },
        msg_hash,
        prev_msg_hash,
        feed_pubkey,
    }
}

fn submit_to_gate(
    gate: &mut ChainGate,
    msg: PendingMessage,
    dedup: &mut DedupRing,
    msg_tx: &mpsc::UnboundedSender<DisplayMessage>,
    pending_refetches: &mut HashSet<[u8; 32]>,
    refetch_tx: &mpsc::UnboundedSender<RefetchResult>,
    handle: &HyperDhtHandle,
) {
    let id = msg.display.id_pubkey;
    let feed_pubkey = msg.feed_pubkey;
    if let SubmitOutcome::Buffered {
        missing_predecessor,
    } = gate.submit(msg, dedup, msg_tx)
    {
        if pending_refetches.insert(missing_predecessor) {
            spawn_refetch(
                handle.clone(),
                missing_predecessor,
                id,
                feed_pubkey,
                refetch_tx.clone(),
            );
        }
    }
}

pub async fn run_reader(
    handle: HyperDhtHandle,
    channel_key: [u8; 32],
    message_key: [u8; 32],
    msg_tx: mpsc::UnboundedSender<DisplayMessage>,
    profile_name: String,
    self_feed_pubkey: Option<[u8; 32]>,
    self_id_pubkey: [u8; 32],
) {
    let mut known_feeds: HashMap<[u8; 32], KnownFeed> = HashMap::new();
    let mut dedup = DedupRing::with_default_capacity();
    let mut backlog: Vec<PendingMessage> = Vec::new();
    let mut gate = ChainGate::new();
    let mut pending_refetches: HashSet<[u8; 32]> = HashSet::new();
    let (refetch_tx, mut refetch_rx) = mpsc::unbounded_channel::<RefetchResult>();

    if let Some(pk) = self_feed_pubkey {
        known_feeds.insert(pk, KnownFeed::new());
    }

    // --- Cold-start: concurrent discovery across all epochs/buckets ---
    let current_epoch = crypto::current_epoch();
    let scan_start = current_epoch.saturating_sub(19);

    let lookup_futures: Vec<_> = (scan_start..=current_epoch)
        .flat_map(|epoch| (0..4u8).map(move |bucket| (epoch, bucket)))
        .map(|(epoch, bucket)| {
            let h = handle.clone();
            let topic = crypto::announce_topic(&channel_key, epoch, bucket);
            async move { (epoch, bucket, h.lookup(topic).await) }
        })
        .collect();

    for (epoch, bucket, result) in join_all(lookup_futures).await {
        if let Ok(results) = result {
            let peer_count: usize = results.iter().map(|r| r.peers.len()).sum();
            if debug::is_enabled() && peer_count > 0 {
                debug::log_event(
                    "Channel scan",
                    "lookup",
                    &format!("epoch={epoch}, bucket={bucket}, results={peer_count}"),
                );
            }
            for result in &results {
                for peer in &result.peers {
                    known_feeds.entry(peer.public_key).or_insert_with(KnownFeed::new);
                }
            }
        }
    }

    // --- Cold-start: fetch all feed records concurrently ---
    let feed_pks: Vec<[u8; 32]> = known_feeds.keys().copied().collect();
    let mget_futures: Vec<_> = feed_pks
        .iter()
        .map(|pk| {
            let h = handle.clone();
            let pk = *pk;
            async move { (pk, h.mutable_get(&pk, 0).await) }
        })
        .collect();

    for (feed_pk, result) in join_all(mget_futures).await {
        if let Ok(Some(mget)) = result {
            if let Ok(record) = FeedRecord::deserialize(&mget.value) {
                if !crypto::verify_ownership_proof(
                    &record.id_pubkey,
                    &feed_pk,
                    &channel_key,
                    &record.ownership_proof,
                ) {
                    continue;
                }

                debug::log_event(
                    "Feed record discovered",
                    "mutable_get",
                    &format!(
                        "feed_pubkey={}, id_pubkey={}, msg_count={}, next_feed={}",
                        debug::short_key(&feed_pk),
                        debug::short_key(&record.id_pubkey),
                        record.msg_count,
                        debug::short_key(&record.next_feed_pubkey),
                    ),
                );

                if let Some(feed_info) = known_feeds.get_mut(&feed_pk) {
                    feed_info.id_pubkey = record.id_pubkey;
                    feed_info.last_seq = mget.seq;
                }

                let msgs = fetch_and_validate_messages(
                    &handle,
                    &message_key,
                    &record.msg_hashes,
                    &record.id_pubkey,
                    feed_pk,
                    &mut dedup,
                    &profile_name,
                    &self_id_pubkey,
                )
                .await;

                if let Some(newest_hash) = record.msg_hashes.first() {
                    if let Some(feed_info) = known_feeds.get_mut(&feed_pk) {
                        feed_info.last_msg_hash = *newest_hash;
                    }
                }

                backlog.extend(msgs);

                fetch_summary_history(
                    &handle,
                    &message_key,
                    record.summary_hash,
                    &record.id_pubkey,
                    feed_pk,
                    &mut dedup,
                    &mut backlog,
                    &profile_name,
                    &self_id_pubkey,
                )
                .await;
                if let Some(feed_info) = known_feeds.get_mut(&feed_pk) {
                    feed_info.last_summary_hash_seen = record.summary_hash;
                }
            }
        }
    }

    for msg in chain_sort(backlog) {
        submit_to_gate(
            &mut gate,
            msg,
            &mut dedup,
            &msg_tx,
            &mut pending_refetches,
            &refetch_tx,
            &handle,
        );
    }

    let _ = msg_tx.send(DisplayMessage {
        id_pubkey: [0u8; 32],
        screen_name: String::new(),
        content: String::new(),
        timestamp: 0,
        is_self: false,
        late: false,
    });

    // --- Steady-state: discovery and feed polling run independently ---

    // Discovery task: runs on its own timer, sends newly-found feed pubkeys
    let (disc_tx, mut disc_rx) = mpsc::unbounded_channel::<[u8; 32]>();
    {
        let handle = handle.clone();
        tokio::spawn(async move {
            run_discovery(handle, channel_key, disc_tx).await;
        });
    }

    let mut expiry_tick = tokio::time::interval(Duration::from_secs(1));
    expiry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Feed polling loop: wakes on its own adaptive schedule, receives new feeds from discovery
    loop {
        let now = Instant::now();
        let earliest_feed_poll = known_feeds.values().map(|f| f.next_poll).min();
        let wake_at = earliest_feed_poll.unwrap_or(now + Duration::from_secs(1));

        tokio::select! {
            _ = tokio::time::sleep_until(wake_at) => {}
            _ = expiry_tick.tick() => {
                let abandoned = gate.expire(Instant::now(), GAP_TIMEOUT, &mut dedup, &msg_tx);
                for hash in abandoned {
                    pending_refetches.remove(&hash);
                }
                continue;
            }
            Some(result) = refetch_rx.recv() => {
                pending_refetches.remove(&result.hash);
                if let Some(data) = result.data {
                    if hash(&data) == result.hash {
                        if let Some(env) =
                            decode_envelope(&message_key, &data, &result.owner)
                        {
                            // Do not pre-insert into the dedup ring here — the
                            // gate will insert on release. Pre-inserting would
                            // make submit_to_gate reject this very message as
                            // duplicate.
                            let pm = envelope_to_pending(
                                env,
                                result.hash,
                                result.feed_pubkey,
                                &self_id_pubkey,
                            );
                            submit_to_gate(
                                &mut gate,
                                pm,
                                &mut dedup,
                                &msg_tx,
                                &mut pending_refetches,
                                &refetch_tx,
                                &handle,
                            );
                        }
                    }
                }
                continue;
            }
            pk = disc_rx.recv() => {
                if let Some(pk) = pk {
                    known_feeds
                        .entry(pk)
                        .and_modify(|f| f.last_active = Instant::now())
                        .or_insert_with(KnownFeed::new);
                }
                // Drain any additional queued discoveries without blocking
                while let Ok(pk) = disc_rx.try_recv() {
                    known_feeds
                        .entry(pk)
                        .and_modify(|f| f.last_active = Instant::now())
                        .or_insert_with(KnownFeed::new);
                }
                continue;
            }
        }

        // Drain any discoveries that arrived while we were sleeping
        while let Ok(pk) = disc_rx.try_recv() {
            known_feeds
                .entry(pk)
                .and_modify(|f| f.last_active = Instant::now())
                .or_insert_with(KnownFeed::new);
        }

        // Expire feeds inactive for longer than DHT TTL
        let now = Instant::now();
        known_feeds
            .retain(|_pk, f| now.duration_since(f.last_active).as_secs() < FEED_EXPIRY_SECS);

        // --- Feed polling: fetch all due feeds concurrently ---
        let due_feeds: Vec<([u8; 32], u64)> = known_feeds
            .iter()
            .filter(|(_pk, f)| f.next_poll <= now)
            .map(|(pk, f)| (*pk, f.last_seq))
            .collect();

        if due_feeds.is_empty() {
            continue;
        }

        if debug::is_enabled() {
            debug::log_event(
                "Feed poll batch",
                "mutable_get",
                &format!("feeds_due={}, total_known={}", due_feeds.len(), known_feeds.len()),
            );
        }

        let poll_start = Instant::now();
        let poll_futures: Vec<_> = due_feeds
            .iter()
            .map(|(pk, cached_seq)| {
                let h = handle.clone();
                let pk = *pk;
                let seq = *cached_seq;
                async move { (pk, h.mutable_get(&pk, seq).await) }
            })
            .collect();

        let poll_results = join_all(poll_futures).await;

        if debug::is_enabled() {
            let elapsed_ms = poll_start.elapsed().as_millis();
            let updated: usize = poll_results
                .iter()
                .filter(|(_, r)| matches!(r, Ok(Some(_))))
                .count();
            debug::log_event(
                "Feed poll complete",
                "mutable_get",
                &format!(
                    "elapsed={}ms, polled={}, updated={}",
                    elapsed_ms,
                    due_feeds.len(),
                    updated
                ),
            );
        }

        for (feed_pk, result) in poll_results {
            let feed_info = match known_feeds.get_mut(&feed_pk) {
                Some(f) => f,
                None => continue,
            };

            match result {
                Ok(Some(mget)) => {
                    if mget.seq <= feed_info.last_seq {
                        feed_info.schedule_next_poll();
                        continue;
                    }
                    feed_info.last_seq = mget.seq;
                    feed_info.last_active = Instant::now();
                    feed_info.last_message_time = Instant::now();
                    feed_info.schedule_next_poll();

                    if let Ok(record) = FeedRecord::deserialize(&mget.value) {
                        if !crypto::verify_ownership_proof(
                            &record.id_pubkey,
                            &feed_pk,
                            &channel_key,
                            &record.ownership_proof,
                        ) {
                            continue;
                        }

                        let first_discovery = feed_info.id_pubkey == [0u8; 32];
                        if first_discovery {
                            feed_info.id_pubkey = record.id_pubkey;
                        } else if record.id_pubkey != feed_info.id_pubkey {
                            continue;
                        }

                        debug::log_event(
                            "Feed record discovered",
                            "mutable_get",
                            &format!(
                                "feed_pubkey={}, id_pubkey={}, msg_count={}, next_feed={}",
                                debug::short_key(&feed_pk),
                                debug::short_key(&record.id_pubkey),
                                record.msg_count,
                                debug::short_key(&record.next_feed_pubkey),
                            ),
                        );

                        let owner_pubkey = feed_info.id_pubkey;
                        let next_feed = record.next_feed_pubkey;

                        if next_feed != [0u8; 32] {
                            known_feeds.entry(next_feed).or_insert_with(KnownFeed::new);
                        }

                        let msgs = fetch_and_validate_messages(
                            &handle,
                            &message_key,
                            &record.msg_hashes,
                            &owner_pubkey,
                            feed_pk,
                            &mut dedup,
                            &profile_name,
                            &self_id_pubkey,
                        )
                        .await;

                        if let Some(newest_hash) = record.msg_hashes.first() {
                            if let Some(fi) = known_feeds.get_mut(&feed_pk) {
                                fi.last_msg_hash = *newest_hash;
                            }
                        }

                        for msg in msgs.into_iter().rev() {
                            submit_to_gate(
                                &mut gate,
                                msg,
                                &mut dedup,
                                &msg_tx,
                                &mut pending_refetches,
                                &refetch_tx,
                                &handle,
                            );
                        }

                        let prior_summary_hash = known_feeds
                            .get(&feed_pk)
                            .map(|f| f.last_summary_hash_seen)
                            .unwrap_or([0u8; 32]);
                        let summary_changed = record.summary_hash != prior_summary_hash
                            && record.summary_hash != [0u8; 32];

                        if first_discovery || summary_changed {
                            let mut history = Vec::new();
                            fetch_summary_history(
                                &handle,
                                &message_key,
                                record.summary_hash,
                                &owner_pubkey,
                                feed_pk,
                                &mut dedup,
                                &mut history,
                                &profile_name,
                                &self_id_pubkey,
                            )
                            .await;
                            for msg in chain_sort(history) {
                                submit_to_gate(
                                    &mut gate,
                                    msg,
                                    &mut dedup,
                                    &msg_tx,
                                    &mut pending_refetches,
                                    &refetch_tx,
                                    &handle,
                                );
                            }
                            if let Some(fi) = known_feeds.get_mut(&feed_pk) {
                                fi.last_summary_hash_seen = record.summary_hash;
                            }
                        }
                    }
                }
                _ => {
                    feed_info.schedule_next_poll();
                }
            }
        }
    }
}

/// Independent discovery task: scans channel topic buckets on a timer,
/// sends newly-found feed pubkeys to the polling loop.
async fn run_discovery(
    handle: HyperDhtHandle,
    channel_key: [u8; 32],
    disc_tx: mpsc::UnboundedSender<[u8; 32]>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(DISCOVERY_INTERVAL_SECS));

    loop {
        interval.tick().await;

        let current_epoch = crypto::current_epoch();
        let epochs = [current_epoch, current_epoch.saturating_sub(1)];
        let disc_start = Instant::now();

        let lookup_futures: Vec<_> = epochs
            .iter()
            .flat_map(|&epoch| (0..4u8).map(move |bucket| (epoch, bucket)))
            .map(|(epoch, bucket)| {
                let h = handle.clone();
                let topic = crypto::announce_topic(&channel_key, epoch, bucket);
                async move { (epoch, bucket, h.lookup(topic).await) }
            })
            .collect();

        let mut new_feeds = 0u32;
        for (epoch, bucket, result) in join_all(lookup_futures).await {
            if let Ok(results) = result {
                let peer_count: usize = results.iter().map(|r| r.peers.len()).sum();
                if debug::is_enabled() && peer_count > 0 {
                    debug::log_event(
                        "Channel scan",
                        "lookup",
                        &format!("epoch={epoch}, bucket={bucket}, results={peer_count}"),
                    );
                }
                for result in &results {
                    for peer in &result.peers {
                        if disc_tx.send(peer.public_key).is_err() {
                            return; // polling loop dropped, shut down
                        }
                        new_feeds += 1;
                    }
                }
            }
        }

        if debug::is_enabled() {
            debug::log_event(
                "Discovery scan complete",
                "lookup",
                &format!(
                    "elapsed={}ms, feeds_sent={}",
                    disc_start.elapsed().as_millis(),
                    new_feeds
                ),
            );
        }
    }
}

/// Validates and fetches messages from a newest-first hash list.
/// Chain validation: each message's prev_msg_hash must equal the hash of the
/// next-older message in the list (msg_hashes[i+1]).
#[allow(clippy::too_many_arguments)]
async fn fetch_and_validate_messages(
    handle: &HyperDhtHandle,
    message_key: &[u8; 32],
    msg_hashes: &[[u8; 32]],
    owner_pubkey: &[u8; 32],
    feed_pubkey: [u8; 32],
    dedup: &mut DedupRing,
    profile_name: &str,
    self_id_pubkey: &[u8; 32],
) -> Vec<PendingMessage> {
    let _ = profile_name;
    let mut messages = Vec::new();

    // Fetch all unseen messages concurrently
    let unseen: Vec<(usize, [u8; 32])> = msg_hashes
        .iter()
        .enumerate()
        .filter(|(_, h)| !dedup.contains(h))
        .map(|(i, h)| (i, *h))
        .collect();

    if probe::is_enabled() {
        eprintln!(
            "[probe] fetch_batch msg_hashes_total={} unseen={}",
            msg_hashes.len(),
            unseen.len(),
        );
    }

    if unseen.is_empty() {
        return messages;
    }

    let fetch_futures: Vec<_> = unseen
        .iter()
        .map(|(i, hash)| {
            let h = handle.clone();
            let hash = *hash;
            let idx = *i;
            async move { (idx, hash, h.immutable_get(hash).await) }
        })
        .collect();

    let mut fetched: HashMap<usize, (Vec<u8>, [u8; 32])> = HashMap::new();
    for (idx, hash, result) in join_all(fetch_futures).await {
        if let Ok(Some(data)) = result {
            fetched.insert(idx, (data, hash));
        }
    }

    // Validate in order (chain validation requires sequential check)
    let mut expected_next_hash: Option<[u8; 32]> = None;
    for (i, msg_hash) in msg_hashes.iter().enumerate() {
        if dedup.contains(msg_hash) {
            expected_next_hash = None;
            continue;
        }
        let (data, _) = match fetched.get(&i) {
            Some(d) => d,
            None => continue,
        };
        if let Ok(plaintext) = wire::decrypt_message(message_key, data) {
            if let Ok(env) = MessageEnvelope::deserialize(&plaintext) {
                if !env.verify() {
                    continue;
                }
                if env.id_pubkey != *owner_pubkey {
                    continue;
                }
                if let Some(expected) = expected_next_hash {
                    if *msg_hash != expected {
                        expected_next_hash = None;
                        continue;
                    }
                }

                let expected_prev = if i + 1 < msg_hashes.len() {
                    msg_hashes[i + 1]
                } else {
                    [0u8; 32]
                };
                if env.prev_msg_hash != expected_prev && expected_prev != [0u8; 32] {
                    continue;
                }

                expected_next_hash = Some(env.prev_msg_hash);

                // NB: do not insert into `dedup` here. The shared ring is
                // populated by `ChainGate::release` so the gate's duplicate
                // check operates on hashes that have actually been emitted
                // to display. Inserting here would mask future late/replay
                // arrivals from the gate's chain logic.
                debug::log_event(
                    "Message received",
                    "immutable_get",
                    &format!(
                        "msg_hash={}, author={}, prev_hash={}, ts={}, content_type=0x{:02x}",
                        debug::short_key(msg_hash),
                        debug::short_key(&env.id_pubkey),
                        debug::short_key(&env.prev_msg_hash),
                        env.timestamp,
                        env.content_type,
                    ),
                );
                let _ = known_users::update_shared(&env.id_pubkey, &env.screen_name);
                let prev_msg_hash = env.prev_msg_hash;
                messages.push(PendingMessage {
                    display: DisplayMessage {
                        id_pubkey: env.id_pubkey,
                        screen_name: env.screen_name,
                        content: env.content,
                        timestamp: env.timestamp,
                        is_self: env.id_pubkey == *self_id_pubkey,
                        late: false,
                    },
                    msg_hash: *msg_hash,
                    prev_msg_hash,
                    feed_pubkey,
                });
            }
        }
    }
    messages
}

#[allow(clippy::too_many_arguments)]
async fn fetch_summary_history(
    handle: &HyperDhtHandle,
    message_key: &[u8; 32],
    mut summary_hash: [u8; 32],
    owner_pubkey: &[u8; 32],
    feed_pubkey: [u8; 32],
    dedup: &mut DedupRing,
    backlog: &mut Vec<PendingMessage>,
    profile_name: &str,
    self_id_pubkey: &[u8; 32],
) {
    let mut depth = 0;
    while summary_hash != [0u8; 32] && depth < MAX_SUMMARY_DEPTH {
        depth += 1;
        let data = match handle.immutable_get(summary_hash).await {
            Ok(Some(d)) => d,
            _ => break,
        };
        let block = match SummaryBlock::deserialize(&data) {
            Ok(b) => b,
            _ => break,
        };
        if !block.verify() || block.id_pubkey != *owner_pubkey {
            break;
        }

        let reversed: Vec<[u8; 32]> = block.msg_hashes.iter().rev().copied().collect();
        let msgs = fetch_and_validate_messages(
            handle,
            message_key,
            &reversed,
            owner_pubkey,
            feed_pubkey,
            dedup,
            profile_name,
            self_id_pubkey,
        )
        .await;
        backlog.extend(msgs);

        summary_hash = block.prev_summary_hash;
    }
}
