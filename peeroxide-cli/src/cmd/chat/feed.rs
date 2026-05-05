use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};
use rand::Rng;
use tokio::sync::watch;

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::wire::FeedRecord;

pub struct FeedState {
    pub feed_keypair: KeyPair,
    pub id_keypair: KeyPair,
    pub channel_key: [u8; 32],
    pub ownership_proof: [u8; 64],
    pub msg_hashes: Vec<[u8; 32]>,
    pub msg_count: u8,
    pub summary_hash: [u8; 32],
    pub next_feed_pubkey: [u8; 32],
    pub seq: u64,
    pub prev_msg_hash: [u8; 32],
    pub bucket_permutation: [u8; 4],
    pub bucket_index: usize,
    pub feed_lifetime_minutes: u64,
    pub feed_lifetime_secs: u64,
    pub created_at: std::time::Instant,
}

impl FeedState {
    pub fn new(
        feed_keypair: KeyPair,
        id_keypair: KeyPair,
        channel_key: [u8; 32],
        ownership_proof: [u8; 64],
        feed_lifetime_minutes: u64,
    ) -> Self {
        let mut rng = rand::rng();
        let mut bucket_permutation: [u8; 4] = [0, 1, 2, 3];
        for i in (1..4).rev() {
            let j = rng.random_range(0..=i);
            bucket_permutation.swap(i, j);
        }

        let wobble: f64 = rng.random_range(0.5..1.5);
        let feed_lifetime_secs = (feed_lifetime_minutes as f64 * 60.0 * wobble) as u64;

        Self {
            feed_keypair,
            id_keypair,
            channel_key,
            ownership_proof,
            msg_hashes: Vec::new(),
            msg_count: 0,
            summary_hash: [0u8; 32],
            next_feed_pubkey: [0u8; 32],
            seq: 0,
            prev_msg_hash: [0u8; 32],
            bucket_permutation,
            bucket_index: 0,
            feed_lifetime_minutes,
            feed_lifetime_secs,
            created_at: std::time::Instant::now(),
        }
    }

    pub fn next_bucket(&mut self) -> u8 {
        let b = self.bucket_permutation[self.bucket_index % 4];
        self.bucket_index += 1;
        b
    }

    pub fn serialize_feed_record(&self) -> Vec<u8> {
        let record = FeedRecord {
            id_pubkey: self.id_keypair.public_key,
            ownership_proof: self.ownership_proof,
            next_feed_pubkey: self.next_feed_pubkey,
            summary_hash: self.summary_hash,
            msg_count: self.msg_count,
            msg_hashes: self.msg_hashes.clone(),
        };
        record.serialize().unwrap_or_default()
    }

    pub fn needs_rotation(&self) -> bool {
        self.created_at.elapsed().as_secs() >= self.feed_lifetime_secs
    }

    /// Rotate to a new feed keypair. Sets `next_feed_pubkey` on the current
    /// state (so the old feed points to the new one), then returns a fresh
    /// `FeedState` for the new keypair.
    pub fn rotate(&mut self) -> FeedState {
        let new_keypair = KeyPair::generate();
        self.next_feed_pubkey = new_keypair.public_key;

        let new_ownership = crypto::ownership_proof(
            &self.id_keypair.secret_key,
            &new_keypair.public_key,
            &self.channel_key,
        );

        FeedState::new(
            new_keypair,
            self.id_keypair.clone(),
            self.channel_key,
            new_ownership,
            self.feed_lifetime_minutes,
        )
    }
}

pub async fn run_feed_refresh(
    handle: HyperDhtHandle,
    feed_keypair: KeyPair,
    mut state_rx: watch::Receiver<(Vec<u8>, u64)>,
    channel_key: [u8; 32],
) {
    let refresh_interval = tokio::time::Duration::from_secs(480);
    let mut interval = tokio::time::interval(refresh_interval);

    loop {
        interval.tick().await;
        let (record_data, seq) = state_rx.borrow_and_update().clone();
        match handle.mutable_put(&feed_keypair, &record_data, seq).await {
            Ok(_) => {
                debug::log_event(
                    "Feed refresh",
                    "mutable_put",
                    &format!(
                        "feed_pubkey={}, seq={seq}",
                        debug::short_key(&feed_keypair.public_key),
                    ),
                );
            }
            Err(e) => {
                tracing::warn!("feed refresh failed: {e}");
            }
        }
        let epoch = crypto::current_epoch();
        let bucket = (epoch % 4) as u8;
        let topic = crypto::announce_topic(&channel_key, epoch, bucket);
        let _ = handle.announce(topic, &feed_keypair, &[]).await;

        debug::log_event(
            "Channel announce",
            "announce",
            &format!(
                "feed_pubkey={}, epoch={epoch}, bucket={bucket}, topic={}",
                debug::short_key(&feed_keypair.public_key),
                debug::short_key(&topic),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_feed_state(lifetime_minutes: u64) -> FeedState {
        let feed_kp = KeyPair::generate();
        let id_kp = KeyPair::generate();
        let channel_key = [0x42u8; 32];
        let ownership = crypto::ownership_proof(&id_kp.secret_key, &feed_kp.public_key, &channel_key);
        FeedState::new(feed_kp, id_kp, channel_key, ownership, lifetime_minutes)
    }

    #[test]
    fn new_feed_state_starts_empty() {
        let fs = make_feed_state(60);
        assert_eq!(fs.msg_hashes.len(), 0);
        assert_eq!(fs.msg_count, 0);
        assert_eq!(fs.seq, 0);
        assert_eq!(fs.next_feed_pubkey, [0u8; 32]);
        assert_eq!(fs.summary_hash, [0u8; 32]);
    }

    #[test]
    fn needs_rotation_false_when_fresh() {
        let fs = make_feed_state(60);
        assert!(!fs.needs_rotation());
    }

    #[test]
    fn rotate_sets_next_feed_pubkey() {
        let mut fs = make_feed_state(60);
        let old_pk = fs.feed_keypair.public_key;
        let new_fs = fs.rotate();
        assert_ne!(fs.next_feed_pubkey, [0u8; 32]);
        assert_eq!(fs.next_feed_pubkey, new_fs.feed_keypair.public_key);
        assert_ne!(new_fs.feed_keypair.public_key, old_pk);
    }

    #[test]
    fn rotate_preserves_identity() {
        let mut fs = make_feed_state(60);
        let id_pk = fs.id_keypair.public_key;
        let new_fs = fs.rotate();
        assert_eq!(new_fs.id_keypair.public_key, id_pk);
        assert_eq!(new_fs.channel_key, fs.channel_key);
    }

    #[test]
    fn rotate_new_feed_starts_clean() {
        let mut fs = make_feed_state(60);
        fs.msg_hashes.push([1u8; 32]);
        fs.msg_count = 1;
        fs.seq = 5;
        let new_fs = fs.rotate();
        assert_eq!(new_fs.msg_hashes.len(), 0);
        assert_eq!(new_fs.msg_count, 0);
        assert_eq!(new_fs.seq, 0);
    }

    #[test]
    fn next_bucket_cycles_through_permutation() {
        let mut fs = make_feed_state(60);
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(fs.next_bucket());
        }
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    #[test]
    fn serialize_feed_record_not_empty() {
        let fs = make_feed_state(60);
        let data = fs.serialize_feed_record();
        assert!(!data.is_empty());
    }

    #[test]
    fn feed_lifetime_has_wobble() {
        let fs1 = make_feed_state(60);
        let fs2 = make_feed_state(60);
        let fs3 = make_feed_state(60);
        let lifetimes = [fs1.feed_lifetime_secs, fs2.feed_lifetime_secs, fs3.feed_lifetime_secs];
        let all_same = lifetimes[0] == lifetimes[1] && lifetimes[1] == lifetimes[2];
        let min = 60 * 60 / 2;
        let max = 60 * 60 * 3 / 2;
        for l in &lifetimes {
            assert!(*l >= min && *l <= max, "lifetime {l} not in expected range [{min}, {max}]");
        }
        assert!(!all_same || lifetimes[0] != 3600, "extremely unlikely: 3 feeds with identical non-60min lifetime");
    }
}
