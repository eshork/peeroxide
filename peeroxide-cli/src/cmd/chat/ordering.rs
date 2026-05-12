use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::cmd::chat::display::DisplayMessage;
use crate::cmd::chat::probe;

/// Default capacity for the shared receiver-side message-hash dedup ring.
pub const DEDUP_RING_CAPACITY: usize = 1000;

/// Bounded FIFO set of message hashes seen by the receiver.
///
/// One shared instance is threaded through fetch-side filtering and the
/// `ChainGate` so a hash that has ever been admitted is never processed
/// again, regardless of which code path re-encounters it. When the ring
/// reaches capacity the oldest hash is evicted; for chat traffic 1000
/// entries comfortably covers a session-length window.
pub struct DedupRing {
    capacity: usize,
    set: HashSet<[u8; 32]>,
    queue: VecDeque<[u8; 32]>,
}

impl DedupRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "DedupRing capacity must be positive");
        Self {
            capacity,
            set: HashSet::with_capacity(capacity),
            queue: VecDeque::with_capacity(capacity),
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEDUP_RING_CAPACITY)
    }

    pub fn contains(&self, h: &[u8; 32]) -> bool {
        self.set.contains(h)
    }

    /// Insert `h` into the ring. Returns `true` if newly added, `false` if
    /// already present. When capacity is exceeded the oldest hash is evicted.
    pub fn insert(&mut self, h: [u8; 32]) -> bool {
        if !self.set.insert(h) {
            return false;
        }
        self.queue.push_back(h);
        if self.queue.len() > self.capacity {
            if let Some(old) = self.queue.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for DedupRing {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

static RELEASE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn short_hex(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(8);
    for byte in &b[..4] {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

pub struct PendingMessage {
    pub display: DisplayMessage,
    pub msg_hash: [u8; 32],
    pub prev_msg_hash: [u8; 32],
}

type BufferedByPrev = HashMap<[u8; 32], (PendingMessage, Instant)>;

/// Tracks per-sender chain state and enforces strict `prev_msg_hash` ordering.
///
/// Callers submit messages oldest-first. The first message seen for a given
/// `id_pubkey` anchors the chain; subsequent messages must link to the last
/// released hash, or they are buffered until their predecessor arrives.
pub struct ChainGate {
    last_released: HashMap<[u8; 32], [u8; 32]>,
    pending: HashMap<[u8; 32], BufferedByPrev>,
}

#[derive(Debug)]
pub enum SubmitOutcome {
    Released,
    Buffered { missing_predecessor: [u8; 32] },
    Duplicate,
}

impl ChainGate {
    pub fn new() -> Self {
        Self {
            last_released: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Submit one message. If its predecessor has been released (or this is
    /// the first message we've seen for this sender), release immediately and
    /// drain any chain-linked buffered descendants. Otherwise buffer and
    /// return the predecessor hash so the caller can kick off a refetch.
    ///
    /// `dedup` is the shared receiver-wide message-hash ring. Any hash already
    /// present is rejected as `Duplicate` before chain logic runs, so a hash
    /// is never released twice even if upstream code paths submit it more
    /// than once.
    pub fn submit(
        &mut self,
        msg: PendingMessage,
        dedup: &mut DedupRing,
        tx: &mpsc::UnboundedSender<DisplayMessage>,
    ) -> SubmitOutcome {
        let id = msg.display.id_pubkey;
        let prev = msg.prev_msg_hash;
        let own = msg.msg_hash;

        if dedup.contains(&own) || self.last_released.get(&id) == Some(&own) {
            return SubmitOutcome::Duplicate;
        }

        let anchor = !self.last_released.contains_key(&id);
        let chains = self.last_released.get(&id) == Some(&prev);

        if anchor || chains {
            self.release(msg, dedup, tx);
            self.drain(&id, dedup, tx);
            return SubmitOutcome::Released;
        }

        self.pending
            .entry(id)
            .or_default()
            .insert(prev, (msg, Instant::now()));
        SubmitOutcome::Buffered {
            missing_predecessor: prev,
        }
    }

    fn release(
        &mut self,
        msg: PendingMessage,
        dedup: &mut DedupRing,
        tx: &mpsc::UnboundedSender<DisplayMessage>,
    ) {
        let id = msg.display.id_pubkey;
        let hash = msg.msg_hash;
        // Mark this hash as seen in the shared ring so no other code path can
        // re-release it. `insert` is a no-op if it was already present.
        dedup.insert(hash);
        if probe::is_enabled() {
            let n = RELEASE_COUNTER.fetch_add(1, AtomicOrdering::Relaxed) + 1;
            let preview: String = msg.display.content.chars().take(40).collect();
            eprintln!(
                "[probe] release#{n} msg_hash={} late={} content={:?}",
                short_hex(&hash),
                msg.display.late,
                preview,
            );
        }
        let _ = tx.send(msg.display);
        self.last_released.insert(id, hash);
    }

    fn drain(
        &mut self,
        id: &[u8; 32],
        dedup: &mut DedupRing,
        tx: &mpsc::UnboundedSender<DisplayMessage>,
    ) {
        loop {
            let cursor = match self.last_released.get(id) {
                Some(h) => *h,
                None => return,
            };
            let next = self
                .pending
                .get_mut(id)
                .and_then(|per_id| per_id.remove(&cursor));
            let Some((msg, _)) = next else {
                return;
            };
            self.release(msg, dedup, tx);
        }
    }

    /// Force-release any buffered messages older than `timeout`. Each released
    /// message is tagged `late = true` and `last_released` is reset so future
    /// in-order messages chain forward from the late release. Returns the list
    /// of predecessor hashes whose buffered descendants were force-released —
    /// the caller should stop refetching them.
    pub fn expire(
        &mut self,
        now: Instant,
        timeout: Duration,
        dedup: &mut DedupRing,
        tx: &mpsc::UnboundedSender<DisplayMessage>,
    ) -> Vec<[u8; 32]> {
        let mut abandoned_predecessors: Vec<[u8; 32]> = Vec::new();

        let ids: Vec<[u8; 32]> = self.pending.keys().copied().collect();
        for id in ids {
            let expired_prevs: Vec<[u8; 32]> = {
                let per_id = match self.pending.get(&id) {
                    Some(p) => p,
                    None => continue,
                };
                per_id
                    .iter()
                    .filter(|(_, (_, t))| now.duration_since(*t) >= timeout)
                    .map(|(k, _)| *k)
                    .collect()
            };

            if expired_prevs.is_empty() {
                continue;
            }

            let mut expired_msgs: Vec<PendingMessage> = Vec::new();
            if let Some(per_id) = self.pending.get_mut(&id) {
                for prev in &expired_prevs {
                    if let Some((mut m, _)) = per_id.remove(prev) {
                        m.display.late = true;
                        expired_msgs.push(m);
                    }
                }
            }

            expired_msgs.sort_by_key(|m| m.display.timestamp);

            for m in expired_msgs {
                let prev = m.prev_msg_hash;
                self.release(m, dedup, tx);
                self.drain(&id, dedup, tx);
                abandoned_predecessors.push(prev);
            }
        }

        abandoned_predecessors
    }

    pub fn buffered_predecessors(&self) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        for per_id in self.pending.values() {
            for prev in per_id.keys() {
                out.push(*prev);
            }
        }
        out
    }
}

/// Sort a batch of messages so each sender's chain plays oldest-first.
///
/// For each `id_pubkey`, walks the `prev_msg_hash` chain starting from the
/// message whose `prev_msg_hash` is not the `msg_hash` of any other message
/// in the batch (i.e. the chain root from the batch's perspective). Messages
/// not reachable from any root are appended at the end in arrival order.
pub fn chain_sort(messages: Vec<PendingMessage>) -> Vec<PendingMessage> {
    let mut by_sender: HashMap<[u8; 32], Vec<PendingMessage>> = HashMap::new();
    for m in messages {
        by_sender.entry(m.display.id_pubkey).or_default().push(m);
    }

    let mut out: Vec<PendingMessage> = Vec::new();
    for (_id, batch) in by_sender {
        let mut by_prev: HashMap<[u8; 32], PendingMessage> = HashMap::new();
        let mut own_hashes: std::collections::HashSet<[u8; 32]> =
            std::collections::HashSet::new();
        for m in batch {
            own_hashes.insert(m.msg_hash);
            by_prev.insert(m.prev_msg_hash, m);
        }

        let roots: Vec<[u8; 32]> = by_prev
            .iter()
            .filter(|(prev, _)| !own_hashes.contains(*prev))
            .map(|(prev, _)| *prev)
            .collect();

        for root in roots {
            let mut cursor = root;
            while let Some(m) = by_prev.remove(&cursor) {
                cursor = m.msg_hash;
                out.push(m);
            }
        }

        // Anything left has a cycle (shouldn't happen) — flush in arrival order.
        for (_, m) in by_prev {
            out.push(m);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn h(b: u8) -> [u8; 32] {
        [b; 32]
    }

    fn msg(id: u8, own: u8, prev: u8, ts: u64) -> PendingMessage {
        PendingMessage {
            display: DisplayMessage {
                id_pubkey: h(id),
                screen_name: String::new(),
                content: format!("msg-{own}"),
                timestamp: ts,
                is_self: false,
                late: false,
            },
            msg_hash: h(own),
            prev_msg_hash: h(prev),
        }
    }

    fn collect(rx: &mut mpsc::UnboundedReceiver<DisplayMessage>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push(m.content);
        }
        out
    }

    fn collect_with_late(
        rx: &mut mpsc::UnboundedReceiver<DisplayMessage>,
    ) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push((m.content, m.late));
        }
        out
    }

    #[test]
    fn in_order_release() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        assert!(matches!(
            g.submit(msg(1, 1, 0, 1), &mut d, &tx),
            SubmitOutcome::Released
        ));
        assert!(matches!(
            g.submit(msg(1, 2, 1, 2), &mut d, &tx),
            SubmitOutcome::Released
        ));
        assert!(matches!(
            g.submit(msg(1, 3, 2, 3), &mut d, &tx),
            SubmitOutcome::Released
        ));
        assert_eq!(collect(&mut rx), vec!["msg-1", "msg-2", "msg-3"]);
    }

    #[test]
    fn reverse_arrival_buffers_then_drains() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        // First message anchors the chain.
        let r1 = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        assert!(matches!(r1, SubmitOutcome::Released));
        // msg 3 arrives before msg 2 — must buffer.
        let r3 = g.submit(msg(1, 3, 2, 3), &mut d, &tx);
        assert!(matches!(
            r3,
            SubmitOutcome::Buffered { missing_predecessor } if missing_predecessor == h(2)
        ));
        // msg 2 arrives — releases 2 then drains 3.
        let r2 = g.submit(msg(1, 2, 1, 2), &mut d, &tx);
        assert!(matches!(r2, SubmitOutcome::Released));
        assert_eq!(collect(&mut rx), vec!["msg-1", "msg-2", "msg-3"]);
    }

    #[test]
    fn gap_timeout_releases_late() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        let _ = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        // Skip msg 2; submit msg 3 — buffered.
        let _ = g.submit(msg(1, 3, 2, 3), &mut d, &tx);
        // Drain msg 1.
        let _ = collect(&mut rx);

        let later = Instant::now() + Duration::from_secs(10);
        let abandoned = g.expire(later, Duration::from_secs(5), &mut d, &tx);
        assert_eq!(abandoned, vec![h(2)]);
        let got = collect_with_late(&mut rx);
        assert_eq!(got, vec![("msg-3".to_string(), true)]);
    }

    #[test]
    fn gap_timeout_then_chain_resumes() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        let _ = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        let _ = g.submit(msg(1, 3, 2, 3), &mut d, &tx);
        let _ = collect(&mut rx);

        let later = Instant::now() + Duration::from_secs(10);
        let _ = g.expire(later, Duration::from_secs(5), &mut d, &tx);
        let _ = collect(&mut rx);

        // After timeout, last_released should be msg 3's hash. msg 4 chains forward.
        let r4 = g.submit(msg(1, 4, 3, 4), &mut d, &tx);
        assert!(matches!(r4, SubmitOutcome::Released));
        assert_eq!(collect(&mut rx), vec!["msg-4"]);
    }

    #[test]
    fn two_sender_interleave_preserves_per_sender_chain() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        // A1, B1, A2, B2 arriving interleaved
        let _ = g.submit(msg(1, 10, 0, 1), &mut d, &tx);
        let _ = g.submit(msg(2, 20, 0, 1), &mut d, &tx);
        let _ = g.submit(msg(1, 11, 10, 2), &mut d, &tx);
        let _ = g.submit(msg(2, 21, 20, 2), &mut d, &tx);
        let got = collect(&mut rx);
        // Cross-sender order is arrival-order, not enforced; but per-sender chain is.
        assert_eq!(got, vec!["msg-10", "msg-20", "msg-11", "msg-21"]);
    }

    #[test]
    fn anchor_on_mid_stream_join() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        // We join when sender has already published; the first thing we receive
        // is msg 5 (no predecessor available locally). It should anchor.
        let r = g.submit(msg(1, 5, 4, 5), &mut d, &tx);
        assert!(matches!(r, SubmitOutcome::Released));
        // msg 6 chains forward.
        let r6 = g.submit(msg(1, 6, 5, 6), &mut d, &tx);
        assert!(matches!(r6, SubmitOutcome::Released));
        assert_eq!(collect(&mut rx), vec!["msg-5", "msg-6"]);
    }

    #[test]
    fn duplicate_submit_ignored() {
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        let _ = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        let r = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        assert!(matches!(r, SubmitOutcome::Duplicate));
        assert_eq!(collect(&mut rx), vec!["msg-1"]);
    }

    #[test]
    fn dedup_ring_blocks_re_release_after_chain_moves_on() {
        // Reproduces the test2.out symptom: a hash is released, the chain
        // advances past it, then the same hash is re-submitted later (e.g.
        // via a refetch path or a duplicate FeedRecord entry). Without the
        // shared dedup ring the per-sender `last_released` no longer matches
        // and the gate would re-release. With the ring, it is rejected.
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        let _ = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        let _ = g.submit(msg(1, 2, 1, 2), &mut d, &tx);
        let _ = g.submit(msg(1, 3, 2, 3), &mut d, &tx);
        assert_eq!(collect(&mut rx), vec!["msg-1", "msg-2", "msg-3"]);

        // Same hash arrives again from a different code path — must be a no-op.
        let r = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        assert!(matches!(r, SubmitOutcome::Duplicate));
        let r = g.submit(msg(1, 2, 1, 2), &mut d, &tx);
        assert!(matches!(r, SubmitOutcome::Duplicate));
        assert!(collect(&mut rx).is_empty());
    }

    #[test]
    fn dedup_ring_blocks_re_release_after_expire() {
        // A buffered message is force-released as late; submitting it again
        // afterwards (e.g. a slow refetch finally returns) must not re-emit.
        let (tx, mut rx) = unbounded_channel();
        let mut g = ChainGate::new();
        let mut d = DedupRing::new(1000);
        let _ = g.submit(msg(1, 1, 0, 1), &mut d, &tx);
        let _ = g.submit(msg(1, 3, 2, 3), &mut d, &tx);
        let _ = collect(&mut rx);

        let later = Instant::now() + Duration::from_secs(10);
        let _ = g.expire(later, Duration::from_secs(5), &mut d, &tx);
        let _ = collect(&mut rx);

        // msg 3 is now in the ring. Re-submitting it must be a Duplicate.
        let r = g.submit(msg(1, 3, 2, 3), &mut d, &tx);
        assert!(matches!(r, SubmitOutcome::Duplicate));
        assert!(collect(&mut rx).is_empty());
    }

    #[test]
    fn dedup_ring_bounded_evicts_oldest() {
        let mut d = DedupRing::new(3);
        assert!(d.insert(h(1)));
        assert!(d.insert(h(2)));
        assert!(d.insert(h(3)));
        assert!(d.contains(&h(1)));
        // Fourth insert evicts the first.
        assert!(d.insert(h(4)));
        assert!(!d.contains(&h(1)));
        assert!(d.contains(&h(2)));
        assert!(d.contains(&h(3)));
        assert!(d.contains(&h(4)));
        // Duplicate insert is a no-op and does not advance eviction.
        assert!(!d.insert(h(4)));
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn chain_sort_orders_oldest_first() {
        // Submit newest-first; chain_sort should reverse into chain order.
        let input = vec![msg(1, 3, 2, 3), msg(1, 2, 1, 2), msg(1, 1, 0, 1)];
        let sorted = chain_sort(input);
        let contents: Vec<_> = sorted.iter().map(|m| m.display.content.clone()).collect();
        assert_eq!(contents, vec!["msg-1", "msg-2", "msg-3"]);
    }

    #[test]
    fn chain_sort_two_senders_independent() {
        let input = vec![
            msg(1, 3, 2, 3),
            msg(2, 30, 20, 3),
            msg(1, 2, 1, 2),
            msg(2, 20, 10, 2),
            msg(1, 1, 0, 1),
            msg(2, 10, 0, 1),
        ];
        let sorted = chain_sort(input);
        // Within each sender, order is chain-correct; cross-sender is unspecified.
        let by_sender: HashMap<[u8; 32], Vec<String>> =
            sorted.iter().fold(HashMap::new(), |mut acc, m| {
                acc.entry(m.display.id_pubkey)
                    .or_default()
                    .push(m.display.content.clone());
                acc
            });
        assert_eq!(by_sender[&h(1)], vec!["msg-1", "msg-2", "msg-3"]);
        assert_eq!(by_sender[&h(2)], vec!["msg-10", "msg-20", "msg-30"]);
    }
}
