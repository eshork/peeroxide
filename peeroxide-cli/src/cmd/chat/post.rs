use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};

use crate::cmd::chat::crypto;
use crate::cmd::chat::feed::FeedState;
use crate::cmd::chat::wire::{self, MessageEnvelope};

pub async fn post_message(
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

    let put_result = handle
        .immutable_put(&encrypted)
        .await
        .map_err(|e| format!("immutable_put failed: {e}"))?;

    let msg_hash = put_result.hash;

    if feed_state.msg_hashes.len() >= 20 {
        publish_summary_block(handle, feed_state, id_keypair)
            .await
            .map_err(|e| format!("summary block failed: {e}"))?;
    }

    feed_state.msg_hashes.insert(0, msg_hash);
    feed_state.msg_count = feed_state.msg_hashes.len() as u8;
    feed_state.prev_msg_hash = msg_hash;
    feed_state.seq += 1;

    let feed_record_data = feed_state.serialize_feed_record();
    handle
        .mutable_put(&feed_state.feed_keypair, &feed_record_data, feed_state.seq)
        .await
        .map_err(|e| format!("mutable_put (feed) failed: {e}"))?;

    let epoch = crypto::current_epoch();
    let bucket = feed_state.next_bucket();
    let topic = crypto::announce_topic(channel_key, epoch, bucket);
    let _ = handle
        .announce(topic, &feed_state.feed_keypair, &[])
        .await;

    Ok(())
}

async fn publish_summary_block(
    handle: &HyperDhtHandle,
    feed_state: &mut FeedState,
    id_keypair: &KeyPair,
) -> Result<(), String> {
    let evict_count = 15;
    let total = feed_state.msg_hashes.len();
    if total < 20 {
        return Ok(());
    }

    let keep = total - evict_count;
    let evicted: Vec<[u8; 32]> = feed_state.msg_hashes[keep..].to_vec();
    let evicted_oldest_first: Vec<[u8; 32]> = evicted.into_iter().rev().collect();

    let summary = wire::SummaryBlock::sign(
        &id_keypair.secret_key,
        id_keypair.public_key,
        feed_state.summary_hash,
        evicted_oldest_first,
    );

    let summary_data = summary
        .serialize()
        .map_err(|e| format!("summary serialize: {e}"))?;
    let put_result = handle
        .immutable_put(&summary_data)
        .await
        .map_err(|e| format!("immutable_put (summary) failed: {e}"))?;

    feed_state.summary_hash = put_result.hash;
    feed_state.msg_hashes.truncate(keep);
    feed_state.msg_count = feed_state.msg_hashes.len() as u8;

    Ok(())
}
