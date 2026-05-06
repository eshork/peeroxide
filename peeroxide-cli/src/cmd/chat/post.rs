use peeroxide_dht::crypto::hash;
use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::feed::FeedState;
use crate::cmd::chat::wire::{self, MessageEnvelope, SummaryBlock};

/// Prepares a message for posting: encrypts, computes hash, updates feed state,
/// then spawns all network operations (immutable_put, mutable_put, announce) in
/// the background. Returns immediately after state mutation so the input loop
/// is never blocked by network latency.
pub fn post_message(
    handle: &HyperDhtHandle,
    feed_state: &mut FeedState,
    id_keypair: &KeyPair,
    message_key: &[u8; 32],
    channel_key: &[u8; 32],
    screen_name: &str,
    content: &str,
) -> Result<(), String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let envelope = MessageEnvelope::sign(
        &id_keypair.secret_key,
        id_keypair.public_key,
        feed_state.prev_msg_hash,
        timestamp,
        wire::CONTENT_TYPE_TEXT,
        screen_name,
        content,
    );

    let plaintext = envelope.serialize();
    let encrypted = wire::encrypt_message(message_key, &plaintext)
        .map_err(|e| format!("encryption failed: {e}"))?;

    if encrypted.len() > wire::MAX_RECORD_SIZE {
        return Err(format!(
            "message too large: {} bytes (max {})",
            encrypted.len(),
            wire::MAX_RECORD_SIZE
        ));
    }

    let msg_hash = hash(&encrypted);

    debug::log_event(
        "Message posted",
        "immutable_put",
        &format!(
            "msg_hash={}, author={}, prev_hash={}, ts={timestamp}, content_type=0x{:02x}",
            debug::short_key(&msg_hash),
            debug::short_key(&id_keypair.public_key),
            debug::short_key(&feed_state.prev_msg_hash),
            envelope.content_type,
        ),
    );

    // Summary block eviction (if needed) — computed locally, network op spawned
    let mut summary_data: Option<Vec<u8>> = None;
    if feed_state.msg_hashes.len() >= 20 {
        let evict_count = 15;
        let total = feed_state.msg_hashes.len();
        let keep = total - evict_count;
        let evicted: Vec<[u8; 32]> = feed_state.msg_hashes[keep..].to_vec();
        let evicted_oldest_first: Vec<[u8; 32]> = evicted.into_iter().rev().collect();

        let summary = SummaryBlock::sign(
            &id_keypair.secret_key,
            id_keypair.public_key,
            feed_state.summary_hash,
            evicted_oldest_first,
        );

        let data = summary
            .serialize()
            .map_err(|e| format!("summary serialize: {e}"))?;
        let summary_hash = hash(&data);

        debug::log_event(
            "Summary block",
            "immutable_put",
            &format!(
                "summary_hash={}, id_pubkey={}, msg_count={}, prev_summary={}",
                debug::short_key(&summary_hash),
                debug::short_key(&id_keypair.public_key),
                evict_count,
                debug::short_key(&feed_state.summary_hash),
            ),
        );

        feed_state.summary_hash = summary_hash;
        feed_state.msg_hashes.truncate(keep);
        feed_state.msg_count = feed_state.msg_hashes.len() as u8;
        summary_data = Some(data);
    }

    // Update feed state synchronously — hash is deterministic
    feed_state.msg_hashes.insert(0, msg_hash);
    feed_state.msg_count = feed_state.msg_hashes.len() as u8;
    feed_state.prev_msg_hash = msg_hash;
    feed_state.seq += 1;

    let feed_record_data = feed_state.serialize_feed_record();
    let epoch = crypto::current_epoch();
    let bucket = feed_state.next_bucket();
    let topic = crypto::announce_topic(channel_key, epoch, bucket);
    let feed_kp = feed_state.feed_keypair.clone();
    let seq = feed_state.seq;
    let msg_count = feed_state.msg_count;

    // Spawn all network operations as a background task chain
    let h = handle.clone();
    tokio::spawn(async move {
        // immutable_put for message (and summary if needed)
        let (msg_put, _) = tokio::join!(
            h.immutable_put(&encrypted),
            async {
                if let Some(data) = summary_data {
                    if let Err(e) = h.immutable_put(&data).await {
                        eprintln!("warning: summary immutable_put failed: {e}");
                    }
                }
            }
        );

        if let Err(e) = msg_put {
            eprintln!("warning: message immutable_put failed: {e}");
            return;
        }

        // mutable_put + announce fire concurrently
        let h2 = h.clone();
        let (put_res, _) = tokio::join!(
            async {
                let r = h.mutable_put(&feed_kp, &feed_record_data, seq).await;
                if r.is_ok() {
                    debug::log_event(
                        "Feed record update",
                        "mutable_put",
                        &format!(
                            "feed_pubkey={}, seq={seq}, msg_count={msg_count}",
                            debug::short_key(&feed_kp.public_key),
                        ),
                    );
                }
                r
            },
            async {
                let _ = h2.announce(topic, &feed_kp, &[]).await;
                debug::log_event(
                    "Channel announce",
                    "announce",
                    &format!(
                        "feed_pubkey={}, epoch={epoch}, bucket={bucket}, topic={}",
                        debug::short_key(&feed_kp.public_key),
                        debug::short_key(&topic),
                    ),
                );
            }
        );

        if let Err(e) = put_res {
            eprintln!("warning: feed mutable_put failed: {e}");
        }
    });

    Ok(())
}
