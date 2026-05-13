//! Generic inbox polling logic shared by the `chat inbox` CLI command and
//! the `chat join` inbox monitor.
//!
//! Per CHAT.md §8.5: the recipient's inbox topic is keyed_blake2b'd over
//! `(id_pubkey, epoch_u64_le, bucket_u8)` with a 1-minute epoch and 4
//! buckets per epoch. Senders announce on a random bucket of the current
//! epoch; readers scan **current + previous epoch × 4 buckets = 8 lookups**
//! per polling cycle. For each unique invite-feed pubkey discovered, the
//! reader `mutable_get`s its record, decrypts using ECDH with its identity
//! key, verifies the ownership proof, and surfaces the resulting invite.
//!
//! ## Concurrency
//!
//! [`InboxMonitor`] is designed to be shared as `Arc<InboxMonitor>` between
//! a polling task and `/inbox` slash-command handlers. All mutable state
//! sits behind a single internal `std::sync::Mutex`; the lock is held only
//! for brief CPU-bound merges of poll results and never across a DHT
//! `.await`. Concretely [`InboxMonitor::poll_once`]:
//!
//! 1. Briefly locks to snapshot the `(feed_pubkey -> seen seq)` watermark.
//! 2. Releases the lock, then does all DHT lookups + `mutable_get`s +
//!    decrypt/verify with the snapshot as the dedup reference.
//! 3. Briefly relocks to merge the candidates into `seen` + the unread
//!    buffer, assigning sequential numbers under the lock so multiple
//!    overlapping pollers (unusual but legal) don't collide.
//!
//! This means `/inbox` calls always acquire the lock quickly even mid-poll,
//! so user-facing slash commands never block on a multi-second DHT scan.

use std::collections::HashMap;
use std::sync::Mutex;

use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::inbox::{self, DecodedInvite};
use crate::cmd::chat::known_users::KnownUser;
use crate::cmd::chat::wire::INVITE_TYPE_DM;

/// A decoded invite with its stable session-scope sequence number ("#N" in
/// the display format).
#[derive(Debug, Clone)]
pub struct NumberedInvite {
    pub number: u32,
    pub invite: DecodedInvite,
}

/// Inner mutable state — sits behind a `std::sync::Mutex` so all access is
/// brief and lock-free across `.await`.
struct InboxMonitorInner {
    /// `feed_pubkey -> last seen seq`. Subsequent observations at the same
    /// (or lower) seq are ignored — they're rebroadcasts of an invite we've
    /// already surfaced.
    seen_invite_feeds: HashMap<[u8; 32], u64>,
    /// Running counter of invites surfaced this session. Increments by 1
    /// per new surfacing; used as the `[INVITE #N]` number.
    all_time_count: u32,
    /// Invites the user hasn't viewed yet. Pushed to by `poll_once`,
    /// drained by `take_unread`.
    unread: Vec<NumberedInvite>,
}

/// Owns the polling watermark + unread buffer behind a single internal
/// lock. Cheap to clone the `Arc<InboxMonitor>` and share between a
/// polling task and `/inbox` handlers.
pub struct InboxMonitor {
    inner: Mutex<InboxMonitorInner>,
    /// Read-only at construction. Holds the cached known-users list for
    /// the display (vendor name fallback).
    cached_users: Vec<KnownUser>,
}

impl InboxMonitor {
    pub fn new(cached_users: Vec<KnownUser>) -> Self {
        Self {
            inner: Mutex::new(InboxMonitorInner {
                seen_invite_feeds: HashMap::new(),
                all_time_count: 0,
                unread: Vec::new(),
            }),
            cached_users,
        }
    }

    /// One polling round: scan the current + previous epoch across all four
    /// buckets, decoding any new invites. Returns the just-surfaced
    /// invites in arrival order; also appends each to the unread buffer.
    ///
    /// The lock is held only briefly at the start (to snapshot the seen
    /// watermark) and at the end (to merge results); the DHT lookups and
    /// `mutable_get`s in between happen with NO locks held, so other
    /// callers (notably `/inbox` slash-command handlers) can always
    /// acquire the lock quickly.
    pub async fn poll_once(
        &self,
        handle: &HyperDhtHandle,
        id_keypair: &KeyPair,
    ) -> Vec<NumberedInvite> {
        // 1. Snapshot the seen map under brief lock. The snapshot is used as
        //    the dedup reference during the lock-free DHT phase. Stale
        //    reads are safe — at worst we re-process an invite the merge
        //    phase will then dedup against the (now-up-to-date) map.
        let seen_snapshot: HashMap<[u8; 32], u64> = {
            let inner = self.inner.lock().expect("inbox monitor mutex poisoned");
            inner.seen_invite_feeds.clone()
        };

        // 2. Lock-free DHT phase: scan + decrypt + verify. Each pass
        //    collects (feed_pk, seq, DecodedInvite) candidates.
        let candidates = perform_dht_scan(handle, id_keypair, &seen_snapshot).await;

        // 3. Brief-lock merge: re-check seq against the (possibly newer)
        //    seen map, assign #N, append to unread.
        let mut surfaced: Vec<NumberedInvite> = Vec::new();
        if !candidates.is_empty() {
            let mut inner = self.inner.lock().expect("inbox monitor mutex poisoned");
            for (feed_pk, seq, invite) in candidates {
                // Re-check under lock (defensive against concurrent pollers).
                if matches!(inner.seen_invite_feeds.get(&feed_pk).copied(), Some(s) if seq <= s) {
                    continue;
                }
                inner.seen_invite_feeds.insert(feed_pk, seq);
                inner.all_time_count = inner.all_time_count.saturating_add(1);
                let numbered = NumberedInvite {
                    number: inner.all_time_count,
                    invite,
                };
                inner.unread.push(numbered.clone());
                surfaced.push(numbered);
            }
        }
        surfaced
    }

    /// Drain the unread buffer; subsequent `unread_count` returns 0 until
    /// new invites arrive.
    pub fn take_unread(&self) -> Vec<NumberedInvite> {
        let mut inner = self.inner.lock().expect("inbox monitor mutex poisoned");
        std::mem::take(&mut inner.unread)
    }

    /// Number of unread invites currently buffered (cheap; for the bar).
    pub fn unread_count(&self) -> usize {
        let inner = self.inner.lock().expect("inbox monitor mutex poisoned");
        inner.unread.len()
    }

    /// Total invites surfaced this session (cumulative, never decrements).
    pub fn all_time_count(&self) -> u32 {
        let inner = self.inner.lock().expect("inbox monitor mutex poisoned");
        inner.all_time_count
    }

    /// Borrow the cached known-users for use by `format_invite_lines`.
    /// Immutable after construction; no lock needed.
    pub fn known_users(&self) -> &[KnownUser] {
        &self.cached_users
    }
}

/// Lock-free DHT scan: fan out all 8 (epoch, bucket) lookups in parallel,
/// then for each lookup result fan out the per-peer `mutable_get`s in
/// parallel, then post-process (decrypt + verify) all candidates. Does
/// NOT touch any `InboxMonitor` state — purely an async I/O helper.
///
/// Errors from individual lookups / gets are swallowed silently — best-
/// effort, network-flaky operations. Per-event debug logs go through
/// `debug::log_event`.
///
/// Parallelism note: an earlier version of this scanned epochs and
/// buckets serially, which made the whole poll cycle take 10-20 s on a
/// typical public DHT — too slow for the background monitor's 15 s
/// cadence. Fanning out via `join_all` cuts the wall-clock to roughly
/// the slowest single round-trip plus the slowest mutable_get fan-out
/// per lookup.
async fn perform_dht_scan(
    handle: &HyperDhtHandle,
    id_keypair: &KeyPair,
    seen_snapshot: &HashMap<[u8; 32], u64>,
) -> Vec<([u8; 32], u64, DecodedInvite)> {
    let current_epoch = crypto::current_epoch();

    // ── phase 1: 8 lookups in parallel ────────────────────────────────
    let lookup_futures = [current_epoch, current_epoch.saturating_sub(1)]
        .into_iter()
        .flat_map(|epoch| (0..4u8).map(move |bucket| (epoch, bucket)))
        .map(|(epoch, bucket)| async move {
            let topic = crypto::inbox_topic(&id_keypair.public_key, epoch, bucket);
            let res = handle.lookup(topic).await;
            (epoch, bucket, res)
        });
    let lookup_results = futures::future::join_all(lookup_futures).await;

    // Collect a unique set of feed_pubkeys to fetch. The same peer may
    // appear on multiple buckets / both epochs; dedup so we don't fire
    // multiple `mutable_get`s for the same target.
    let mut to_fetch: HashMap<[u8; 32], ()> = HashMap::new();
    for (epoch, bucket, res) in &lookup_results {
        let Ok(results) = res else { continue };
        let peer_count: usize = results.iter().map(|r| r.peers.len()).sum();
        debug::log_event(
            "Inbox check",
            "lookup",
            &format!("epoch={epoch}, bucket={bucket}, results={peer_count}"),
        );
        for result in results {
            for peer in &result.peers {
                to_fetch.entry(peer.public_key).or_insert(());
            }
        }
    }

    // ── phase 2: fan out all mutable_gets in parallel ─────────────────
    let get_futures = to_fetch.keys().copied().map(|feed_pk| async move {
        let res = handle.mutable_get(&feed_pk, 0).await;
        (feed_pk, res)
    });
    let get_results = futures::future::join_all(get_futures).await;

    // ── phase 3: decrypt + verify; collect candidates ─────────────────
    let mut out: Vec<([u8; 32], u64, DecodedInvite)> = Vec::new();
    for (feed_pk, res) in get_results {
        let Ok(Some(mget)) = res else { continue };
        let prev_seq = seen_snapshot.get(&feed_pk).copied();
        if matches!(prev_seq, Some(s) if mget.seq <= s) {
            continue;
        }
        let Ok(invite) = inbox::decrypt_and_verify_invite(
            &mget.value,
            &feed_pk,
            id_keypair,
        ) else {
            continue;
        };
        debug::log_event(
            "Invite received",
            "mutable_get",
            &format!(
                "invite_feed_pk={}, sender={}, invite_type=0x{:02x}, payload_len={}",
                debug::short_key(&feed_pk),
                debug::short_key(&invite.sender_pubkey),
                invite.invite_type,
                invite.payload.len(),
            ),
        );
        out.push((feed_pk, mget.seq, invite));
    }
    out
}

/// Render a numbered invite as the same multi-line string format the
/// `chat inbox` CLI command produces on stdout. Returns one element per
/// output line so the caller can route each line through either
/// `println!` (CLI) or `ChatUi::render_system` (TUI `/inbox`).
///
/// Format (matches the original `inbox::display_invite` output verbatim):
/// ```text
/// [INVITE #N] DM from <name> (<short>)
///   "<lure>"        (only if non-empty for DM)
///   → peeroxide chat dm <hex> --profile <p>
/// ```
/// Or for a private/group channel invite:
/// ```text
/// [INVITE #N] Channel "<name>" from <name> (<short>)
///   → peeroxide chat join "<name>" --group "<salt>" --profile <p>
/// ```
pub fn format_invite_lines(
    numbered: &NumberedInvite,
    profile_name: &str,
    known_users: &[KnownUser],
) -> Vec<String> {
    let invite = &numbered.invite;
    let number = numbered.number;
    let sender_hex = hex::encode(invite.sender_pubkey);
    let short = &sender_hex[..8];
    let sender_name = known_users
        .iter()
        .find(|u| u.pubkey == invite.sender_pubkey)
        .map(|u| u.screen_name.as_str())
        .unwrap_or(short);

    let mut out = Vec::with_capacity(3);
    if invite.invite_type == INVITE_TYPE_DM {
        let lure = String::from_utf8_lossy(&invite.payload);
        out.push(format!("[INVITE #{number}] DM from {sender_name} ({short})"));
        if !lure.is_empty() {
            out.push(format!("  \"{lure}\""));
        }
        out.push(format!(
            "  → peeroxide chat dm {sender_hex} --profile {profile_name}"
        ));
        return out;
    }

    // Channel invite: payload is [name_len(1) | name(N) | salt_len(2 LE) | salt(M)].
    if invite.payload.len() >= 3 {
        let name_len = invite.payload[0] as usize;
        if invite.payload.len() >= 1 + name_len + 2 {
            let name = String::from_utf8_lossy(&invite.payload[1..1 + name_len]);
            let salt_len = u16::from_le_bytes([
                invite.payload[1 + name_len],
                invite.payload[2 + name_len],
            ]) as usize;
            if invite.payload.len() >= 3 + name_len + salt_len {
                let salt =
                    String::from_utf8_lossy(&invite.payload[3 + name_len..3 + name_len + salt_len]);
                out.push(format!(
                    "[INVITE #{number}] Channel \"{name}\" from {sender_name} ({short})"
                ));
                out.push(format!(
                    "  → peeroxide chat join \"{name}\" --group \"{salt}\" --profile {profile_name}"
                ));
                return out;
            }
        }
    }
    out.push(format!(
        "[INVITE #{number}] Channel invite from {sender_name} ({short})"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite_dm(sender_byte: u8, lure: &str) -> DecodedInvite {
        DecodedInvite {
            sender_pubkey: [sender_byte; 32],
            next_feed_pubkey: [0; 32],
            invite_type: INVITE_TYPE_DM,
            payload: lure.as_bytes().to_vec(),
        }
    }

    fn invite_channel(name: &str, salt: &str) -> DecodedInvite {
        let mut p: Vec<u8> = Vec::new();
        p.push(name.len() as u8);
        p.extend_from_slice(name.as_bytes());
        p.extend_from_slice(&(salt.len() as u16).to_le_bytes());
        p.extend_from_slice(salt.as_bytes());
        DecodedInvite {
            sender_pubkey: [0xab; 32],
            next_feed_pubkey: [0; 32],
            invite_type: crate::cmd::chat::wire::INVITE_TYPE_PRIVATE,
            payload: p,
        }
    }

    /// Push an invite directly into the unread buffer (bypassing DHT) for
    /// testing the take_unread / unread_count surface.
    fn push_for_test(m: &InboxMonitor, invite: DecodedInvite) {
        let mut inner = m.inner.lock().unwrap();
        inner.all_time_count = inner.all_time_count.saturating_add(1);
        let n = NumberedInvite {
            number: inner.all_time_count,
            invite,
        };
        inner.unread.push(n);
    }

    #[test]
    fn new_monitor_is_empty() {
        let m = InboxMonitor::new(vec![]);
        assert_eq!(m.unread_count(), 0);
        assert_eq!(m.all_time_count(), 0);
    }

    #[test]
    fn take_unread_drains_and_resets_count() {
        let m = InboxMonitor::new(vec![]);
        push_for_test(&m, invite_dm(1, "hi"));
        push_for_test(&m, invite_dm(2, "yo"));
        assert_eq!(m.unread_count(), 2);
        assert_eq!(m.all_time_count(), 2);
        let drained = m.take_unread();
        assert_eq!(drained.len(), 2);
        assert_eq!(m.unread_count(), 0);
        // All-time count is unaffected by drain.
        assert_eq!(m.all_time_count(), 2);
    }

    #[test]
    fn take_unread_assigns_sequential_numbers() {
        let m = InboxMonitor::new(vec![]);
        push_for_test(&m, invite_dm(1, "a"));
        push_for_test(&m, invite_dm(2, "b"));
        push_for_test(&m, invite_dm(3, "c"));
        let drained = m.take_unread();
        assert_eq!(drained.iter().map(|n| n.number).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn format_invite_lines_dm_with_lure() {
        let inv = NumberedInvite {
            number: 7,
            invite: invite_dm(0x42, "wanna chat?"),
        };
        let lines = format_invite_lines(&inv, "default", &[]);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("[INVITE #7] DM from 42424242"));
        assert_eq!(lines[1], "  \"wanna chat?\"");
        assert!(lines[2].contains("peeroxide chat dm "));
        assert!(lines[2].contains("--profile default"));
    }

    #[test]
    fn format_invite_lines_dm_without_lure() {
        let inv = NumberedInvite {
            number: 3,
            invite: invite_dm(0x11, ""),
        };
        let lines = format_invite_lines(&inv, "alice", &[]);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("[INVITE #3] DM from 11111111"));
        assert!(lines[1].contains("--profile alice"));
    }

    #[test]
    fn format_invite_lines_dm_uses_known_user_name() {
        let users = vec![KnownUser {
            pubkey: [0x42; 32],
            screen_name: "Alice".to_string(),
        }];
        let inv = NumberedInvite {
            number: 1,
            invite: invite_dm(0x42, ""),
        };
        let lines = format_invite_lines(&inv, "default", &users);
        assert!(lines[0].contains("DM from Alice (42424242)"), "got: {}", lines[0]);
    }

    #[test]
    fn format_invite_lines_channel_with_salt() {
        let inv = NumberedInvite {
            number: 4,
            invite: invite_channel("secret-room", "salty"),
        };
        let lines = format_invite_lines(&inv, "default", &[]);
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].starts_with("[INVITE #4] Channel \"secret-room\" from"),
            "got: {}",
            lines[0]
        );
        assert!(lines[1].contains("--group \"salty\""));
        assert!(lines[1].contains("--profile default"));
    }

    /// Quick concurrency sanity check: while a long-running fake "poll"
    /// task holds the lock briefly to merge results, another caller can
    /// always acquire the lock without contention. This is more of a
    /// design-doc test than a true stress test — it just verifies that
    /// the API surface uses &self (not &mut self) so multiple callers
    /// can share an `Arc<InboxMonitor>`.
    #[test]
    fn monitor_methods_take_shared_ref() {
        let m = std::sync::Arc::new(InboxMonitor::new(vec![]));
        let m2 = m.clone();
        // Two shared-ref methods can be called from different references.
        let _ = m.unread_count();
        let _ = m2.all_time_count();
        let _ = m.take_unread();
        let _ = m2.known_users();
    }
}
