//! Debug logging for the chat subsystem.
//!
//! When enabled via `--debug`, prints timestamped event lines to stderr
//! for high-value network events useful for tracing and diagnostics.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Local;

static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() {
    DEBUG_ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// Format: `[YYYY-MM-DD HH:MM:SS] [DEBUG] {event}: [{op}] {details}`
pub fn log_event(event: &str, op: &str, details: &str) {
    if !is_enabled() {
        return;
    }
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S");
    eprintln!("[{ts}] [DEBUG] {event}: [{op}] {details}");
}

/// Truncates to `first6...last6` when longer than 16 chars.
pub fn short_hex(hex: &str) -> String {
    if hex.len() <= 16 {
        hex.to_string()
    } else {
        format!("{}...{}", &hex[..6], &hex[hex.len() - 6..])
    }
}

pub fn short_key(key: &[u8; 32]) -> String {
    short_hex(&hex::encode(key))
}
