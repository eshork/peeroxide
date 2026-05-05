use std::collections::{HashMap, HashSet};

use peeroxide_dht::hyperdht::HyperDhtHandle;
use tokio::sync::mpsc;

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::display::DisplayMessage;
use crate::cmd::chat::profile;
use crate::cmd::chat::wire::{self, FeedRecord, MessageEnvelope, SummaryBlock};

struct KnownFeed {
    id_pubkey: [u8; 32],
    last_seq: u64,
    unchanged_count: u32,
    last_msg_hash: [u8; 32],
}

const MAX_SUMMARY_DEPTH: usize = 100;

pub async fn run_reader(
    handle: HyperDhtHandle,
    channel_key: [u8; 32],
    message_key: [u8; 32],
    msg_tx: mpsc::UnboundedSender<DisplayMessage>,
    profile_name: String,
) {
    let mut known_feeds: HashMap<[u8; 32], KnownFeed> = HashMap::new();
    let mut seen_msg_hashes: HashSet<[u8; 32]> = HashSet::new();
    let mut backlog: Vec<DisplayMessage> = Vec::new();

    let current_epoch = crypto::current_epoch();
    let scan_start = current_epoch.saturating_sub(19);
    for epoch in scan_start..=current_epoch {
        for bucket in 0..4u8 {
            let topic = crypto::announce_topic(&channel_key, epoch, bucket);
            if let Ok(results) = handle.lookup(topic).await {
                let peer_count: usize = results.iter().map(|r| r.peers.len()).sum();
                if debug::is_enabled() && peer_count > 0 {
                    debug::log_event(
                        "Channel scan",
                        "lookup",
                        &format!(
                            "epoch={epoch}, bucket={bucket}, results={peer_count}",
                        ),
                    );
                }
                for result in &results {
                    for peer in &result.peers {
                        let feed_pk = peer.public_key;
                        known_feeds.entry(feed_pk).or_insert(KnownFeed {
                            id_pubkey: [0u8; 32],
                            last_seq: 0,
                            unchanged_count: 0,
                            last_msg_hash: [0u8; 32],
                        });
                    }
                }
            }
        }
    }

    for (feed_pk, feed_info) in known_feeds.iter_mut() {
        if let Ok(Some(mget)) = handle.mutable_get(feed_pk, 0).await {
            if let Ok(record) = FeedRecord::deserialize(&mget.value) {
                if !crypto::verify_ownership_proof(
                    &record.id_pubkey,
                    feed_pk,
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
                        debug::short_key(feed_pk),
                        debug::short_key(&record.id_pubkey),
                        record.msg_count,
                        debug::short_key(&record.next_feed_pubkey),
                    ),
                );

                feed_info.id_pubkey = record.id_pubkey;
                feed_info.last_seq = mget.seq;

                let msgs = fetch_and_validate_messages(
                    &handle,
                    &message_key,
                    &record.msg_hashes,
                    &record.id_pubkey,
                    &mut seen_msg_hashes,
                    &profile_name,
                )
                .await;

                if let Some(newest_hash) = record.msg_hashes.first() {
                    feed_info.last_msg_hash = *newest_hash;
                }

                backlog.extend(msgs);

                fetch_summary_history(
                    &handle,
                    &message_key,
                    record.summary_hash,
                    &record.id_pubkey,
                    &mut seen_msg_hashes,
                    &mut backlog,
                    &profile_name,
                )
                .await;
            }
        }
    }

    backlog.sort_by_key(|m| m.timestamp);
    for msg in backlog {
        let _ = msg_tx.send(msg);
    }

    let _ = msg_tx.send(DisplayMessage {
        id_pubkey: [0u8; 32],
        screen_name: String::new(),
        content: String::new(),
        timestamp: 0,
        is_self: false,
    });

    let poll_interval = tokio::time::Duration::from_secs(6);
    let mut interval = tokio::time::interval(poll_interval);

    loop {
        interval.tick().await;

        let current_epoch = crypto::current_epoch();
        for epoch in [current_epoch, current_epoch.saturating_sub(1)] {
            for bucket in 0..4u8 {
                let topic = crypto::announce_topic(&channel_key, epoch, bucket);
                if let Ok(results) = handle.lookup(topic).await {
                    let peer_count: usize = results.iter().map(|r| r.peers.len()).sum();
                    if debug::is_enabled() && peer_count > 0 {
                        debug::log_event(
                            "Channel scan",
                            "lookup",
                            &format!(
                                "epoch={epoch}, bucket={bucket}, results={peer_count}",
                            ),
                        );
                    }
                    for result in &results {
                        for peer in &result.peers {
                            let feed_pk = peer.public_key;
                            known_feeds
                                .entry(feed_pk)
                                .and_modify(|f| f.unchanged_count = 0)
                                .or_insert(KnownFeed {
                                    id_pubkey: [0u8; 32],
                                    last_seq: 0,
                                    unchanged_count: 0,
                                    last_msg_hash: [0u8; 32],
                                });
                        }
                    }
                }
            }
        }

        let feed_pks: Vec<[u8; 32]> = known_feeds.keys().copied().collect();
        for feed_pk in feed_pks {
            let feed_info = known_feeds.get(&feed_pk).unwrap();
            if feed_info.unchanged_count >= 3 {
                continue;
            }

            match handle.mutable_get(&feed_pk, 0).await {
                Ok(Some(mget)) => {
                    let feed_info = known_feeds.get_mut(&feed_pk).unwrap();
                    if mget.seq <= feed_info.last_seq {
                        feed_info.unchanged_count += 1;
                        continue;
                    }
                    feed_info.last_seq = mget.seq;
                    feed_info.unchanged_count = 0;

                    if let Ok(record) = FeedRecord::deserialize(&mget.value) {
                        if !crypto::verify_ownership_proof(
                            &record.id_pubkey,
                            &feed_pk,
                            &channel_key,
                            &record.ownership_proof,
                        ) {
                            continue;
                        }

                        if feed_info.id_pubkey == [0u8; 32] {
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
                            if let std::collections::hash_map::Entry::Vacant(e) =
                                known_feeds.entry(next_feed)
                            {
                                e.insert(KnownFeed {
                                    id_pubkey: [0u8; 32],
                                    last_seq: 0,
                                    unchanged_count: 0,
                                    last_msg_hash: [0u8; 32],
                                });
                            }
                        }

                        let msgs = fetch_and_validate_messages(
                            &handle,
                            &message_key,
                            &record.msg_hashes,
                            &owner_pubkey,
                            &mut seen_msg_hashes,
                            &profile_name,
                        )
                        .await;

                        if let Some(newest_hash) = record.msg_hashes.first() {
                            let feed_info = known_feeds.get_mut(&feed_pk).unwrap();
                            feed_info.last_msg_hash = *newest_hash;
                        }

                        for msg in msgs {
                            let _ = msg_tx.send(msg);
                        }
                    }
                }
                _ => {
                    let feed_info = known_feeds.get_mut(&feed_pk).unwrap();
                    feed_info.unchanged_count += 1;
                }
            }
        }
    }
}

/// Validates and fetches messages from a newest-first hash list.
/// Chain validation: each message's prev_msg_hash must equal the hash of the
/// next-older message in the list (msg_hashes[i+1]).
async fn fetch_and_validate_messages(
    handle: &HyperDhtHandle,
    message_key: &[u8; 32],
    msg_hashes: &[[u8; 32]],
    owner_pubkey: &[u8; 32],
    seen_msg_hashes: &mut HashSet<[u8; 32]>,
    profile_name: &str,
) -> Vec<DisplayMessage> {
    let mut messages = Vec::new();
    let mut expected_next_hash: Option<[u8; 32]> = None;

    for (i, msg_hash) in msg_hashes.iter().enumerate() {
        if seen_msg_hashes.contains(msg_hash) {
            expected_next_hash = None;
            continue;
        }
        if let Ok(Some(data)) = handle.immutable_get(*msg_hash).await {
            if let Ok(plaintext) = wire::decrypt_message(message_key, &data) {
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

                    seen_msg_hashes.insert(*msg_hash);
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
                    let _ = profile::append_known_user(
                        profile_name,
                        &env.id_pubkey,
                        &env.screen_name,
                    );
                    messages.push(DisplayMessage {
                        id_pubkey: env.id_pubkey,
                        screen_name: env.screen_name,
                        content: env.content,
                        timestamp: env.timestamp,
                        is_self: false,
                    });
                }
            }
        }
    }
    messages
}

async fn fetch_summary_history(
    handle: &HyperDhtHandle,
    message_key: &[u8; 32],
    mut summary_hash: [u8; 32],
    owner_pubkey: &[u8; 32],
    seen_msg_hashes: &mut HashSet<[u8; 32]>,
    backlog: &mut Vec<DisplayMessage>,
    profile_name: &str,
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
            seen_msg_hashes,
            profile_name,
        )
        .await;
        backlog.extend(msgs);

        summary_hash = block.prev_summary_hash;
    }
}
