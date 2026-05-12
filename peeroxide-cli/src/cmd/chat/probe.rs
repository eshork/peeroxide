//! Receiver-side message-flow probes for chat.
//!
//! When enabled via `--probe`, emits structured one-line records to stderr
//! at the key transitions in the publish/receive pipeline:
//!
//! * `stdin#N read=...`        — every line read from stdin by the publisher
//! * `post#N content=...`      — every call to `post_message`
//! * `post#N msg_hash=... prev=...` — the hash chain link recorded for each post
//! * `fetch_batch msg_hashes_total=X unseen=Y` — every receiver fetch batch
//! * `release#N msg_hash=... late=... content=...` — every gate release
//!
//! Useful for diagnosing publisher↔receiver ordering bugs and duplicate
//! releases without recompiling. Counter IDs are global to the process so
//! turning the flag on mid-session may produce non-zero starting indices.

use std::sync::atomic::{AtomicBool, Ordering};

static PROBE_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() {
    PROBE_ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    PROBE_ENABLED.load(Ordering::Relaxed)
}
