#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::cmd::deaddrop::progress::{
    format::render_log_line,
    rate::RateCalculator,
    state::ProgressState,
};

/// Non-TTY progress renderer that prints one formatted line to stderr every
/// 2 seconds. Mirrors the cancellation pattern used by `BarRenderer`:
/// `Arc<Notify>` + `tokio::select!`, with `Drop` issuing a sync
/// `notify_one()` so the tick task exits cleanly without async-in-Drop.
pub struct PeriodicLogRenderer {
    state: Arc<ProgressState>,
    #[allow(dead_code)]
    rate: Arc<Mutex<RateCalculator>>,
    stop: Arc<Notify>,
    tick_handle: Option<JoinHandle<()>>,
    finished: bool,
}

impl PeriodicLogRenderer {
    pub fn new(state: Arc<ProgressState>) -> Self {
        let rate = Arc::new(Mutex::new(RateCalculator::new()));
        let stop = Arc::new(Notify::new());

        let stop_clone = stop.clone();
        let state_clone = state.clone();
        let rate_clone = rate.clone();

        let tick_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // First tick fires immediately by default; advance past it so the
            // first log line lands ~2s after construction.
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = std::time::Instant::now();
                        let bytes_done = state_clone.bytes_done.load(Ordering::Relaxed);
                        let mut rate_guard = rate_clone.lock().await;
                        rate_guard.record(now, bytes_done);
                        let smoothed = rate_guard.rate_bps();
                        let total = state_clone.bytes_total.load(Ordering::Relaxed);
                        let done = state_clone.bytes_done.load(Ordering::Relaxed);
                        let eta = rate_guard.eta_secs(total, done);
                        drop(rate_guard);
                        let line = render_log_line(&state_clone, smoothed, eta);
                        eprintln!("{}", line);
                    }
                    _ = stop_clone.notified() => break,
                }
            }
        });

        Self {
            state,
            rate,
            stop,
            tick_handle: Some(tick_handle),
            finished: false,
        }
    }

    /// Stop the tick task without consuming `self`. Idempotent — calling
    /// twice is a no-op. Lets the reporter survive a PUT refresh-loop
    /// handoff before final cleanup.
    pub async fn finish_initial(&mut self) {
        if self.finished {
            return;
        }
        self.stop.notify_one();
        if let Some(handle) = self.tick_handle.take() {
            let _ = handle.await;
        }
        self.finished = true;
    }

    /// Full cleanup, consuming `self`.
    pub async fn finish(mut self) {
        self.finish_initial().await;
    }

    pub fn state(&self) -> &Arc<ProgressState> {
        &self.state
    }
}

impl Drop for PeriodicLogRenderer {
    fn drop(&mut self) {
        // Cancellation signal is sync-safe; the spawned tick task will
        // observe it on its next select poll. We do NOT await here.
        self.stop.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::deaddrop::progress::state::Phase;
    use tokio::time::timeout;

    fn put_v2_state() -> Arc<ProgressState> {
        ProgressState::new(Phase::Put, 2, Arc::<str>::from("file.txt"))
    }

    #[tokio::test]
    async fn new_creates_renderer() {
        let renderer = PeriodicLogRenderer::new(put_v2_state());
        assert!(!renderer.finished);
        assert!(renderer.tick_handle.is_some());
        renderer.finish().await;
    }

    #[tokio::test]
    async fn finish_initial_idempotent() {
        let mut renderer = PeriodicLogRenderer::new(put_v2_state());
        renderer.finish_initial().await;
        renderer.finish_initial().await;
        assert!(renderer.finished);
        assert!(renderer.tick_handle.is_none());
    }

    #[tokio::test]
    async fn finish_completes_within_timeout() {
        let renderer = PeriodicLogRenderer::new(put_v2_state());
        let result = timeout(Duration::from_millis(500), renderer.finish()).await;
        assert!(result.is_ok(), "finish() should complete within 500ms");
    }

    #[tokio::test]
    async fn drop_does_not_panic() {
        drop(PeriodicLogRenderer::new(put_v2_state()));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn tick_does_not_fire_before_first_interval() {
        let renderer = PeriodicLogRenderer::new(put_v2_state());
        assert!(!renderer.finished);
        assert!(renderer.tick_handle.is_some());
        renderer.finish().await;
    }
}
