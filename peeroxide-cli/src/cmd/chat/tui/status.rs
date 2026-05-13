//! Shared status state observed by publisher / reader / DHT-poll task and
//! consumed by the status bar renderer.
//!
//! All counters are `AtomicUsize` with `Relaxed` ordering — these are
//! advisory display values, not synchronisation primitives.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arc_swap::ArcSwap;
use tokio::sync::Notify;

/// Counters and labels shown on the status bar.
///
/// Created once at session start and shared via `Arc` across:
/// - `join.rs` (channel name, dht peers polling task)
/// - `publisher.rs` (`send_pending`)
/// - `reader.rs` (`recv_pending`, `feed_count`)
/// - `tui::interactive` (renderer, reads all fields)
///
/// Mutators call `dirty.notify_one()` after a write so the renderer can repaint
/// promptly. Renderers can also poll on an idle timer.
pub struct StatusState {
    pub send_pending: AtomicUsize,
    /// Number of DHT `immutable_get` requests currently outstanding for
    /// **message or summary content**. This is the "Receiving (N)" count
    /// the user sees — it represents content the reader is actively pulling
    /// because it knows about new messages (FeedRecord listed unseen hashes,
    /// summary-history walk, or predecessor refetch). Managed via
    /// [`RecvFetchGuard`] which inc/decs atomically across an `await`.
    ///
    /// Background scans (`lookup` for new peers, `mutable_get` of FeedRecords
    /// to *check* for new messages) are **not** counted here — those are
    /// signalled separately by `dht_active`.
    pub recv_pending: AtomicUsize,
    /// Number of any-kind DHT requests currently outstanding (lookup,
    /// mutable_get, immutable_get). Surfaces as a single-character activity
    /// indicator at the far left of the bar so the user can tell when
    /// background DHT chatter is happening even though no message is
    /// incoming. Managed via [`DhtActivityGuard`]; `RecvFetchGuard`
    /// additionally bumps `recv_pending`.
    pub dht_active: AtomicUsize,
    pub feed_count: AtomicUsize,
    pub dht_peers: AtomicUsize,
    pub channel_name: ArcSwap<String>,
    pub dirty: Notify,
}

impl StatusState {
    pub fn new(channel_name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            send_pending: AtomicUsize::new(0),
            recv_pending: AtomicUsize::new(0),
            dht_active: AtomicUsize::new(0),
            feed_count: AtomicUsize::new(0),
            dht_peers: AtomicUsize::new(0),
            channel_name: ArcSwap::from_pointee(channel_name.into()),
            dirty: Notify::new(),
        })
    }

    /// Increment `send_pending` and notify the renderer.
    pub fn inc_send_pending(&self) {
        self.send_pending.fetch_add(1, Ordering::Relaxed);
        self.dirty.notify_one();
    }

    /// Decrement `send_pending` (saturating) and notify.
    pub fn dec_send_pending(&self) {
        // saturating: don't wrap if mismatched inc/dec ever sneak in.
        let _ = self.send_pending.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |v| Some(v.saturating_sub(1)),
        );
        self.dirty.notify_one();
    }

    /// Set `recv_pending` to an absolute count and notify.
    pub fn set_recv_pending(&self, n: usize) {
        let prev = self.recv_pending.swap(n, Ordering::Relaxed);
        if prev != n {
            self.dirty.notify_one();
        }
    }

    /// Increment the in-flight `immutable_get` counter and notify the
    /// renderer. Paired with [`StatusState::dec_recv_in_flight`]; prefer
    /// using [`RecvFetchGuard`] which couples the two and survives early
    /// returns / panics across an `await`.
    pub fn inc_recv_in_flight(&self) {
        self.recv_pending.fetch_add(1, Ordering::Relaxed);
        self.dirty.notify_one();
    }

    /// Decrement the in-flight counter (saturating at zero).
    pub fn dec_recv_in_flight(&self) {
        let _ = self
            .recv_pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
        self.dirty.notify_one();
    }

    /// Increment the any-DHT-op counter (lookup / mutable_get / immutable_get).
    /// Prefer [`DhtActivityGuard`].
    pub fn inc_dht_active(&self) {
        self.dht_active.fetch_add(1, Ordering::Relaxed);
        self.dirty.notify_one();
    }

    /// Decrement the any-DHT-op counter (saturating at zero).
    pub fn dec_dht_active(&self) {
        let _ = self
            .dht_active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
        self.dirty.notify_one();
    }

    /// Set `feed_count` and notify.
    pub fn set_feed_count(&self, n: usize) {
        let prev = self.feed_count.swap(n, Ordering::Relaxed);
        if prev != n {
            self.dirty.notify_one();
        }
    }

    /// Set `dht_peers` and notify.
    pub fn set_dht_peers(&self, n: usize) {
        let prev = self.dht_peers.swap(n, Ordering::Relaxed);
        if prev != n {
            self.dirty.notify_one();
        }
    }

    /// Snapshot a consistent view of the counters for one render pass.
    pub fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            send_pending: self.send_pending.load(Ordering::Relaxed),
            recv_pending: self.recv_pending.load(Ordering::Relaxed),
            dht_active: self.dht_active.load(Ordering::Relaxed) > 0,
            feed_count: self.feed_count.load(Ordering::Relaxed),
            dht_peers: self.dht_peers.load(Ordering::Relaxed),
            channel_name: (**self.channel_name.load()).clone(),
        }
    }
}

/// RAII guard that increments `recv_pending` on construction and decrements
/// on drop. Wrap each `immutable_get` call (for message / summary content)
/// in one of these so the in-flight count stays consistent across early
/// returns, errors, and panics that unwind through the await.
pub struct RecvFetchGuard {
    status: Arc<StatusState>,
}

impl RecvFetchGuard {
    pub fn new(status: Arc<StatusState>) -> Self {
        status.inc_recv_in_flight();
        Self { status }
    }
}

impl Drop for RecvFetchGuard {
    fn drop(&mut self) {
        self.status.dec_recv_in_flight();
    }
}

/// RAII guard that increments `dht_active` on construction and decrements on
/// drop. Wrap **every** DHT read call (lookup / mutable_get / immutable_get)
/// in one of these so the left-edge activity dot lights up while any DHT op
/// is in flight. For content fetches that should also surface as
/// `Receiving (N)`, additionally use [`RecvFetchGuard`].
pub struct DhtActivityGuard {
    status: Arc<StatusState>,
}

impl DhtActivityGuard {
    pub fn new(status: Arc<StatusState>) -> Self {
        status.inc_dht_active();
        Self { status }
    }
}

impl Drop for DhtActivityGuard {
    fn drop(&mut self) {
        self.status.dec_dht_active();
    }
}

/// A point-in-time copy of the status counters and channel name. Cheap to
/// pass to the pure-function renderer in this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub send_pending: usize,
    pub recv_pending: usize,
    /// True when at least one DHT op (lookup / mutable_get / immutable_get)
    /// is currently in flight. Drives the left-edge activity dot.
    pub dht_active: bool,
    pub feed_count: usize,
    pub dht_peers: usize,
    pub channel_name: String,
}

/// Truncation level applied to the status bar based on terminal width.
///
/// Levels are ordered from most-detailed (`Full`) to least (`ChannelOnly`).
/// The renderer chooses the most-detailed level whose natural width fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncLevel {
    /// `Sending... (3)  Receiving... (12)` ··· `Feeds: 7  DHT: 42  #room-name`
    Full,
    /// `Sending  Receiving` ··· `Feeds: 7  DHT: 42  #room-name`
    DropWords,
    /// `S:3 R:12` ··· `F:7 D:42 #room-name`
    Short,
    /// `S:3 R:12` ··· `D:42 #room-name`
    ShortDropF,
    /// `S:3 R:12` ··· `#room-name`
    ShortDropFD,
    /// `Ready` ··· `#room-name`  (or just `#room-name` if no left activity)
    ChannelAndReady,
    /// `#room-name` (possibly truncated with `…`)
    ChannelOnly,
}

/// Identifier for a status-bar segment, used to key sticky slot widths
/// across renders. Two segments are "the same slot" iff their `LeftSeg` /
/// `RightSeg` value is equal — so when a counter goes to zero and the
/// `Sending` segment disappears, its slot is released and the `Ready`
/// segment that takes its place starts fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeftSeg {
    Sending,
    Receiving,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightSeg {
    Feeds,
    Dht,
    Channel,
}

/// Sticky slot widths for the left and right segment groups. Once a slot has
/// grown to fit a value, it stays at that width until the terminal is resized
/// (which calls [`SlotWidths::reset`]).
///
/// Left-side slots are also positionally sticky: once `Sending` or `Receiving`
/// has appeared at least once, its slot remains reserved in the bar (rendered
/// as padded blanks when the underlying counter is zero) so subsequent
/// segments don't visually shift left when an upstream segment goes idle.
/// Slot widths grow monotonically and are only released by [`Self::reset`].
#[derive(Debug, Default)]
pub struct SlotWidths {
    pub left: std::collections::HashMap<LeftSeg, usize>,
    pub right: std::collections::HashMap<RightSeg, usize>,
}

impl SlotWidths {
    pub fn reset(&mut self) {
        self.left.clear();
        self.right.clear();
    }
}

/// Pure function: choose a truncation level given a snapshot and terminal width.
///
/// Returns the most detailed level whose natural rendered width fits within
/// `cols`, accounting for one space of padding on each end of the bar, the
/// activity-dot slot at the far left (2 cols: dot + separator), and a
/// minimum 2-column gap between the left and right groups.
pub fn pick_level(snap: &StatusSnapshot, cols: usize) -> TruncLevel {
    // Padding (1 left + 1 right) + dot slot (2) + minimum gap (2) = 6 reserved.
    let avail = cols.saturating_sub(6);

    for level in [
        TruncLevel::Full,
        TruncLevel::DropWords,
        TruncLevel::Short,
        TruncLevel::ShortDropF,
        TruncLevel::ShortDropFD,
        TruncLevel::ChannelAndReady,
        TruncLevel::ChannelOnly,
    ] {
        let (l, r) = natural_widths(snap, level);
        if l + r <= avail {
            return level;
        }
    }
    TruncLevel::ChannelOnly
}

/// Natural rendered width of the left and right groups at a given truncation
/// level. Does not include sticky-slot padding (that's added at layout time)
/// nor end-padding/gap (those are added by `pick_level` / `render_bar`).
fn natural_widths(snap: &StatusSnapshot, level: TruncLevel) -> (usize, usize) {
    let activity_present = snap.send_pending > 0 || snap.recv_pending > 0;
    let l = match level {
        TruncLevel::Full => {
            let mut parts: Vec<String> = Vec::new();
            if snap.send_pending > 0 {
                parts.push(format!("Sending... ({})", snap.send_pending));
            }
            if snap.recv_pending > 0 {
                parts.push(format!("Receiving... ({})", snap.recv_pending));
            }
            if parts.is_empty() {
                "Ready".len()
            } else {
                parts.join("  ").len()
            }
        }
        TruncLevel::DropWords => {
            let mut parts: Vec<&str> = Vec::new();
            if snap.send_pending > 0 {
                parts.push("Sending");
            }
            if snap.recv_pending > 0 {
                parts.push("Receiving");
            }
            if parts.is_empty() {
                "Ready".len()
            } else {
                parts.join("  ").len()
            }
        }
        TruncLevel::Short
        | TruncLevel::ShortDropF
        | TruncLevel::ShortDropFD => {
            let mut parts: Vec<String> = Vec::new();
            if snap.send_pending > 0 {
                parts.push(format!("S:{}", snap.send_pending));
            }
            if snap.recv_pending > 0 {
                parts.push(format!("R:{}", snap.recv_pending));
            }
            parts.join(" ").len()
        }
        TruncLevel::ChannelAndReady => {
            if activity_present { 0 } else { "Ready".len() }
        }
        TruncLevel::ChannelOnly => 0,
    };
    let r = match level {
        TruncLevel::Full | TruncLevel::DropWords => {
            // Feeds: N  DHT: N  #channel
            let f = format!("Feeds: {}", snap.feed_count);
            let d = format!("DHT: {}", snap.dht_peers);
            f.len() + 2 + d.len() + 2 + snap.channel_name.len()
        }
        TruncLevel::Short => {
            let f = format!("F:{}", snap.feed_count);
            let d = format!("D:{}", snap.dht_peers);
            f.len() + 1 + d.len() + 1 + snap.channel_name.len()
        }
        TruncLevel::ShortDropF => {
            let d = format!("D:{}", snap.dht_peers);
            d.len() + 1 + snap.channel_name.len()
        }
        TruncLevel::ShortDropFD | TruncLevel::ChannelAndReady => snap.channel_name.len(),
        TruncLevel::ChannelOnly => snap.channel_name.len(),
    };
    (l, r)
}

/// Render the plain-text content of the status bar (no terminal escapes) at
/// the chosen level, applying sticky slot widths. Returns a `String` exactly
/// `cols` wide (padded with spaces) — the caller wraps it in grey colouring.
pub fn render_bar(snap: &StatusSnapshot, level: TruncLevel, cols: usize, slots: &mut SlotWidths) -> String {
    if cols < 4 {
        // Pathological — terminal is essentially unusable for a bar. Return
        // exactly `cols` spaces; caller still gets a coloured row.
        return " ".repeat(cols);
    }

    // Activity dot at the far left. Always 1 visible cell: '●' when any DHT
    // op is in flight, ' ' otherwise. Followed by a 1-cell separator so left
    // segments don't visually touch the dot. The slot is always reserved
    // regardless of activity state — keeps left segment positions stable.
    // Only included if cols ≥ 6 (otherwise the bar is too narrow and we drop
    // the dot to preserve room for the channel name).
    let show_dot_slot = cols >= 6;
    let dot_prefix: String = if show_dot_slot {
        let ch = if snap.dht_active { '●' } else { ' ' };
        format!("{ch} ")
    } else {
        String::new()
    };

    // Build segment lists for both groups, tagged by segment kind so we can
    // look up sticky slot widths.
    let (left_segs, right_segs) = build_segments(snap, level);

    // Drop right-side slot entries for segments that aren't present this
    // frame (right ordering is fixed and only changes on level transitions,
    // which only happen on resize → `reset()`, so this rarely fires; kept
    // for safety against stale entries).
    let right_kinds: std::collections::HashSet<RightSeg> =
        right_segs.iter().map(|(k, _)| *k).collect();
    slots.right.retain(|k, _| right_kinds.contains(k));

    // Left side: positionally sticky until `slots.reset()` (called on resize).
    // We update sticky widths from the segments that ARE active this frame,
    // but never drop a Sending/Receiving slot once it's been reserved.
    //
    // The `Ready` slot is special: it represents the all-idle state, and is
    // only meaningful as long as no real activity slot has been reserved.
    // If `Sending` or `Receiving` has appeared at least once, drop Ready —
    // the reserved blank slots already communicate idle state.
    for (k, s) in &left_segs {
        let w = s.chars().count();
        let entry = slots.left.entry(*k).or_insert(0);
        if w > *entry {
            *entry = w;
        }
    }
    let has_real_sticky = slots.left.contains_key(&LeftSeg::Sending)
        || slots.left.contains_key(&LeftSeg::Receiving);
    if has_real_sticky {
        slots.left.remove(&LeftSeg::Ready);
    }

    // Grow right-side slots to fit current values (monotonic).
    for (k, s) in &right_segs {
        let w = s.chars().count();
        let entry = slots.right.entry(*k).or_insert(0);
        if w > *entry {
            *entry = w;
        }
    }

    // Active-segment lookup for the left side, so we can fill sticky slots
    // whose kind isn't active this frame with blanks of the slot's width.
    let active_left: std::collections::HashMap<LeftSeg, &str> = left_segs
        .iter()
        .map(|(k, s)| (*k, s.as_str()))
        .collect();

    // Padded left segment strings. When real sticky slots are reserved,
    // iterate in fixed positional order (Sending → Receiving) so positions
    // stay stable across frames; absent kinds render as space-padding. When
    // no real sticky slots are reserved (initial idle state), render whatever
    // `build_segments` returned (typically `Ready`, or nothing at low levels).
    let left_rendered: Vec<String> = if has_real_sticky {
        [LeftSeg::Sending, LeftSeg::Receiving]
            .iter()
            .filter_map(|k| {
                let w = *slots.left.get(k)?;
                let s = active_left.get(k).copied().unwrap_or("");
                Some(pad_right(s, w))
            })
            .collect()
    } else {
        left_segs
            .iter()
            .map(|(k, s)| {
                let w = slots
                    .left
                    .get(k)
                    .copied()
                    .unwrap_or_else(|| s.chars().count());
                pad_right(s, w)
            })
            .collect()
    };
    let right_rendered: Vec<String> = right_segs
        .iter()
        .map(|(k, s)| {
            let w = slots.right.get(k).copied().unwrap_or_else(|| s.chars().count());
            pad_left(s, w)
        })
        .collect();

    // Left group joined by single space; right group joined by single space.
    // (The natural-width computation above counts inter-segment spaces too.)
    let left_join = left_rendered.join(" ");
    let right_join = right_rendered.join(" ");

    // Available content area: cols minus 1 left-pad minus 1 right-pad minus
    // the dot slot (2 cells) if present.
    let dot_slot_w = dot_prefix.chars().count();
    let inner = cols.saturating_sub(2 + dot_slot_w);

    // If at ChannelOnly and the channel name doesn't fit, truncate with ellipsis.
    if matches!(level, TruncLevel::ChannelOnly) && right_join.chars().count() > inner {
        let mut name = snap.channel_name.clone();
        if inner == 0 {
            return " ".repeat(cols);
        }
        // chars() not bytes — channel names are ASCII in practice but be safe.
        let take = inner.saturating_sub(1);
        name = name.chars().take(take).collect::<String>();
        name.push('…');
        return format!(" {dot_prefix}{:>width$} ", name, width = inner);
    }

    // Fit left + right inside `inner` with at least one space gap.
    let left_len = left_join.chars().count();
    let right_len = right_join.chars().count();
    let mut gap = inner.saturating_sub(left_len + right_len);
    if gap == 0 {
        gap = 1; // Ensure visual separation; allow slight overflow trimming below.
    }
    let mut body = String::new();
    body.push_str(&left_join);
    for _ in 0..gap {
        body.push(' ');
    }
    body.push_str(&right_join);

    // If body got too long (shouldn't, given pick_level), trim from the left.
    let body_len = body.chars().count();
    if body_len > inner {
        let drop = body_len - inner;
        body = body.chars().skip(drop).collect();
    } else if body_len < inner {
        for _ in 0..(inner - body_len) {
            body.push(' ');
        }
    }

    format!(" {dot_prefix}{body} ")
}

type LeftSegments = Vec<(LeftSeg, String)>;
type RightSegments = Vec<(RightSeg, String)>;

/// Build the ordered, kind-tagged list of segments for each group at the
/// chosen level. Segments are excluded when their underlying counter is zero
/// or omitted at that level.
fn build_segments(snap: &StatusSnapshot, level: TruncLevel) -> (LeftSegments, RightSegments) {
    let activity = snap.send_pending > 0 || snap.recv_pending > 0;
    let left: Vec<(LeftSeg, String)> = match level {
        TruncLevel::Full => {
            let mut v = Vec::new();
            if snap.send_pending > 0 {
                v.push((LeftSeg::Sending, format!("Sending... ({})", snap.send_pending)));
            }
            if snap.recv_pending > 0 {
                v.push((
                    LeftSeg::Receiving,
                    format!("Receiving... ({})", snap.recv_pending),
                ));
            }
            if v.is_empty() {
                vec![(LeftSeg::Ready, "Ready".to_string())]
            } else {
                v
            }
        }
        TruncLevel::DropWords => {
            let mut v = Vec::new();
            if snap.send_pending > 0 {
                v.push((LeftSeg::Sending, "Sending".to_string()));
            }
            if snap.recv_pending > 0 {
                v.push((LeftSeg::Receiving, "Receiving".to_string()));
            }
            if v.is_empty() {
                vec![(LeftSeg::Ready, "Ready".to_string())]
            } else {
                v
            }
        }
        TruncLevel::Short | TruncLevel::ShortDropF | TruncLevel::ShortDropFD => {
            let mut v = Vec::new();
            if snap.send_pending > 0 {
                v.push((LeftSeg::Sending, format!("S:{}", snap.send_pending)));
            }
            if snap.recv_pending > 0 {
                v.push((LeftSeg::Receiving, format!("R:{}", snap.recv_pending)));
            }
            v
        }
        TruncLevel::ChannelAndReady => {
            if activity {
                Vec::new()
            } else {
                vec![(LeftSeg::Ready, "Ready".to_string())]
            }
        }
        TruncLevel::ChannelOnly => Vec::new(),
    };
    let right: Vec<(RightSeg, String)> = match level {
        TruncLevel::Full | TruncLevel::DropWords => vec![
            (RightSeg::Feeds, format!("Feeds: {}", snap.feed_count)),
            (RightSeg::Dht, format!("DHT: {}", snap.dht_peers)),
            (RightSeg::Channel, snap.channel_name.clone()),
        ],
        TruncLevel::Short => vec![
            (RightSeg::Feeds, format!("F:{}", snap.feed_count)),
            (RightSeg::Dht, format!("D:{}", snap.dht_peers)),
            (RightSeg::Channel, snap.channel_name.clone()),
        ],
        TruncLevel::ShortDropF => vec![
            (RightSeg::Dht, format!("D:{}", snap.dht_peers)),
            (RightSeg::Channel, snap.channel_name.clone()),
        ],
        TruncLevel::ShortDropFD | TruncLevel::ChannelAndReady | TruncLevel::ChannelOnly => {
            vec![(RightSeg::Channel, snap.channel_name.clone())]
        }
    };
    (left, right)
}

fn pad_right(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        let mut out = String::from(s);
        for _ in 0..(width - len) {
            out.push(' ');
        }
        out
    }
}

fn pad_left(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        let mut out = String::new();
        for _ in 0..(width - len) {
            out.push(' ');
        }
        out.push_str(s);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(s: usize, r: usize, f: usize, d: usize, name: &str) -> StatusSnapshot {
        StatusSnapshot {
            send_pending: s,
            recv_pending: r,
            dht_active: false,
            feed_count: f,
            dht_peers: d,
            channel_name: name.to_string(),
        }
    }

    fn snap_active(s: usize, r: usize, f: usize, d: usize, name: &str) -> StatusSnapshot {
        StatusSnapshot {
            send_pending: s,
            recv_pending: r,
            dht_active: true,
            feed_count: f,
            dht_peers: d,
            channel_name: name.to_string(),
        }
    }

    #[test]
    fn picks_full_when_room() {
        let s = snap(3, 12, 7, 42, "#room-name");
        assert_eq!(pick_level(&s, 120), TruncLevel::Full);
    }

    #[test]
    fn falls_back_progressively() {
        let s = snap(3, 12, 7, 42, "#room-name");
        // 120 → Full; shrink to find each level.
        let levels: Vec<TruncLevel> = (10..=120)
            .map(|w| pick_level(&s, w))
            .collect();
        // Should be monotone-non-increasing in "detail" (i.e. as we go from
        // narrow to wide, level moves Full → ChannelOnly direction).
        // Spot-check: at very narrow widths we end at ChannelOnly.
        assert_eq!(pick_level(&s, 10), TruncLevel::ChannelOnly);
        // Sanity: somewhere in between we hit Short.
        assert!(levels.iter().any(|l| matches!(l, TruncLevel::Short)));
    }

    #[test]
    fn idle_shows_ready() {
        let s = snap(0, 0, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::Full, 80, &mut slots);
        assert_eq!(bar.chars().count(), 80);
        assert!(bar.contains("Ready"), "bar = {bar:?}");
        assert!(bar.contains("Feeds: 7"));
        assert!(bar.contains("DHT: 42"));
        assert!(bar.contains("#room"));
    }

    #[test]
    fn activity_dot_shows_when_dht_active() {
        let s = snap_active(0, 0, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::Full, 80, &mut slots);
        // Bar layout: " ● {body} "
        assert!(bar.starts_with(" ● "), "bar = {bar:?}");
    }

    #[test]
    fn activity_dot_hidden_when_idle() {
        let s = snap(0, 0, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::Full, 80, &mut slots);
        // Idle: same slot width but ' ' instead of '●'. Bar starts with "   "
        // (1 lead pad + 1 dot slot + 1 separator).
        assert!(!bar.contains('●'), "bar = {bar:?}");
        assert!(bar.starts_with("   "));
    }

    #[test]
    fn activity_dot_dropped_in_extreme_narrow() {
        // cols < 6: dot slot is dropped entirely to give channel name room.
        let s = snap_active(0, 0, 0, 0, "#r");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::ChannelOnly, 5, &mut slots);
        assert!(!bar.contains('●'), "bar = {bar:?}");
        assert_eq!(bar.chars().count(), 5);
    }

    #[test]
    fn recv_in_flight_shows_count() {
        // recv_pending now represents in-flight DHT immutable_gets. A value
        // of 3 should render as "Receiving... (3)".
        let s = snap(0, 3, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::Full, 80, &mut slots);
        assert!(bar.contains("Receiving... (3)"), "bar = {bar:?}");
        assert!(!bar.contains("Ready"));
    }

    #[test]
    fn active_shows_counts() {
        let s = snap(3, 12, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::Full, 80, &mut slots);
        assert!(bar.contains("Sending... (3)"));
        assert!(bar.contains("Receiving... (12)"));
        assert!(!bar.contains("Ready"));
    }

    #[test]
    fn short_level_uses_abbreviations() {
        let s = snap(3, 12, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::Short, 40, &mut slots);
        assert!(bar.contains("S:3"));
        assert!(bar.contains("R:12"));
        assert!(bar.contains("F:7"));
        assert!(bar.contains("D:42"));
        assert!(bar.contains("#room"));
    }

    #[test]
    fn channel_only_truncates_with_ellipsis() {
        let s = snap(0, 0, 0, 0, "#very-long-channel-name");
        let mut slots = SlotWidths::default();
        let bar = render_bar(&s, TruncLevel::ChannelOnly, 12, &mut slots);
        assert_eq!(bar.chars().count(), 12);
        assert!(bar.contains('…'), "bar = {bar:?}");
    }

    #[test]
    fn sticky_slots_grow_monotonically() {
        let s1 = snap(3, 0, 7, 42, "#room");
        let s2 = snap(123456, 0, 7, 42, "#room");
        let s3 = snap(3, 0, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let _ = render_bar(&s1, TruncLevel::Full, 80, &mut slots);
        let w1 = *slots.left.get(&LeftSeg::Sending).unwrap();
        let _ = render_bar(&s2, TruncLevel::Full, 80, &mut slots);
        let w2 = *slots.left.get(&LeftSeg::Sending).unwrap();
        assert!(w2 > w1, "slot should grow with bigger value");
        let _ = render_bar(&s3, TruncLevel::Full, 80, &mut slots);
        let w3 = *slots.left.get(&LeftSeg::Sending).unwrap();
        assert_eq!(w3, w2, "slot should NOT shrink when value shrinks");
    }

    #[test]
    fn slot_kept_sticky_when_segment_disappears() {
        // Sticky semantics: once Sending has appeared, its slot stays
        // reserved (rendered as blanks when idle) until `reset()` is called
        // — which happens on terminal resize. Ready is suppressed once any
        // real activity slot is reserved.
        let active = snap(3, 0, 7, 42, "#room");
        let idle = snap(0, 0, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let _ = render_bar(&active, TruncLevel::Full, 80, &mut slots);
        assert!(slots.left.contains_key(&LeftSeg::Sending));
        assert!(!slots.left.contains_key(&LeftSeg::Ready));
        let _ = render_bar(&idle, TruncLevel::Full, 80, &mut slots);
        assert!(
            slots.left.contains_key(&LeftSeg::Sending),
            "Sending slot should remain sticky after going idle"
        );
        assert!(
            !slots.left.contains_key(&LeftSeg::Ready),
            "Ready should not appear once a real activity slot is reserved"
        );

        // After reset (simulates a terminal resize) the sticky state clears
        // and the next idle render picks Ready up again.
        slots.reset();
        let _ = render_bar(&idle, TruncLevel::Full, 80, &mut slots);
        assert!(!slots.left.contains_key(&LeftSeg::Sending));
        assert_eq!(
            slots.left.get(&LeftSeg::Ready).copied(),
            Some("Ready".len())
        );
    }

    #[test]
    fn receiving_position_sticky_when_sending_goes_idle() {
        // The collapse-left bug: once both Sending and Receiving have been
        // seen, Receiving must keep its slot position even after Sending's
        // counter drops to zero.
        let both = snap(3, 5, 7, 42, "#room");
        let only_recv = snap(0, 5, 7, 42, "#room");
        let mut slots = SlotWidths::default();
        let bar_both = render_bar(&both, TruncLevel::Full, 80, &mut slots);
        // Find columns of "Receiving" while both are active.
        let col_both = bar_both
            .find("Receiving")
            .expect("Receiving present when active");

        let bar_recv = render_bar(&only_recv, TruncLevel::Full, 80, &mut slots);
        let col_recv = bar_recv
            .find("Receiving")
            .expect("Receiving still present after Sending goes idle");
        assert_eq!(
            col_both, col_recv,
            "Receiving column must be sticky when Sending goes idle\n  both: {bar_both:?}\n  recv: {bar_recv:?}"
        );
        // And Sending's slot is now blanks of its previous width.
        let sending_w = slots.left.get(&LeftSeg::Sending).copied().unwrap();
        assert!(sending_w >= "Sending... (3)".len());
    }

    #[test]
    fn bar_is_always_cols_wide() {
        for cols in [4_usize, 10, 20, 40, 80, 120] {
            let s = snap(3, 12, 7, 42, "#room");
            let level = pick_level(&s, cols);
            let mut slots = SlotWidths::default();
            let bar = render_bar(&s, level, cols, &mut slots);
            assert_eq!(bar.chars().count(), cols, "level={level:?} cols={cols}");
        }
    }

    #[test]
    fn pad_helpers() {
        assert_eq!(pad_right("ab", 5), "ab   ");
        assert_eq!(pad_left("ab", 5), "   ab");
        assert_eq!(pad_right("abcdef", 3), "abcdef");
    }
}
