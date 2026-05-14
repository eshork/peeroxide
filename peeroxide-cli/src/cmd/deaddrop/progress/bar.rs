#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::cmd::deaddrop::progress::{
    format::{
        render_bar_line, render_data_line, render_index_line, render_overall_line,
        render_wire_line,
    },
    rate::RateCalculator,
    state::{Phase, ProgressState},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarLayout {
    Single,
    V2GetMulti,
}

/// indicatif-driven renderer that ticks a background task to refresh the
/// progress bar(s).
///
/// Layout:
///   Single (v1 + v2 PUT): 2 bars — main bar line, wire-stats line.
///   V2GetMulti (v2 GET): 4 bars — index, data, wire, overall.
///
/// The wire line samples `state.wire_bytes_sent` / `state.wire_bytes_received`
/// (which are `Arc<AtomicU64>` shared with `peeroxide_dht::io::WireCounters`)
/// and renders rates plus an amplification factor (wire bytes / payload bytes).
pub struct BarRenderer {
    layout: BarLayout,
    #[allow(dead_code)]
    mp: Option<MultiProgress>,
    bars: Vec<ProgressBar>,
    state: Arc<ProgressState>,
    #[allow(dead_code)]
    rate: Arc<Mutex<RateCalculator>>,
    stop: Arc<Notify>,
    tick_handle: Option<JoinHandle<()>>,
    finished: bool,
}

impl BarRenderer {
    pub fn new(state: Arc<ProgressState>) -> Self {
        let layout = if state.phase == Phase::Get && state.version == 2 {
            BarLayout::V2GetMulti
        } else {
            BarLayout::Single
        };

        let style = ProgressStyle::with_template("{msg}").expect("static template is valid");

        // All layouts now use MultiProgress because we add a wire-stats bar.
        let bar_count = match layout {
            BarLayout::Single => 2, // main + wire
            BarLayout::V2GetMulti => 4, // index + data + wire + overall
        };
        let mp = MultiProgress::new();
        let mut bars = Vec::with_capacity(bar_count);
        for _ in 0..bar_count {
            let bar = mp.add(ProgressBar::new(0));
            bar.set_style(style.clone());
            bar.enable_steady_tick(Duration::from_millis(100));
            bars.push(bar);
        }
        let mp = Some(mp);

        let rate = Arc::new(Mutex::new(RateCalculator::new()));
        // Separate rate calculators for wire-up and wire-down. They share the
        // same window/sample policy but track distinct atomic counters.
        let wire_up_rate = Arc::new(Mutex::new(RateCalculator::new()));
        let wire_down_rate = Arc::new(Mutex::new(RateCalculator::new()));
        let stop = Arc::new(Notify::new());

        let stop_clone = stop.clone();
        let state_clone = state.clone();
        let rate_clone = rate.clone();
        let wire_up_clone = wire_up_rate.clone();
        let wire_down_clone = wire_down_rate.clone();
        let bars_clone = bars.clone();
        let layout_clone = layout;

        let tick_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = std::time::Instant::now();

                        // Payload-throughput rate calc.
                        let mut rate_guard = rate_clone.lock().await;
                        let bytes_done = state_clone.bytes_done.load(Ordering::Relaxed);
                        rate_guard.record(now, bytes_done);
                        let smoothed = rate_guard.rate_bps();
                        let total = state_clone.bytes_total.load(Ordering::Relaxed);
                        let eta = rate_guard.eta_secs(total, bytes_done);
                        drop(rate_guard);

                        // Wire-byte rate calcs (independent up/down).
                        let wire_sent = state_clone.wire_bytes_sent.load(Ordering::Relaxed);
                        let wire_recv = state_clone.wire_bytes_received.load(Ordering::Relaxed);
                        let mut up_guard = wire_up_clone.lock().await;
                        up_guard.record(now, wire_sent);
                        let up_bps = up_guard.rate_bps();
                        drop(up_guard);
                        let mut down_guard = wire_down_clone.lock().await;
                        down_guard.record(now, wire_recv);
                        let down_bps = down_guard.rate_bps();
                        drop(down_guard);
                        let wire_total = wire_sent.saturating_add(wire_recv);
                        let wire_line = render_wire_line(
                            &state_clone, up_bps, down_bps, wire_total,
                        );

                        match layout_clone {
                            BarLayout::Single => {
                                let msg = render_bar_line(&state_clone, smoothed, eta);
                                if let Some(bar) = bars_clone.first() {
                                    bar.set_message(msg);
                                }
                                if let Some(bar) = bars_clone.get(1) {
                                    bar.set_message(wire_line);
                                }
                            }
                            BarLayout::V2GetMulti => {
                                if let Some(bar) = bars_clone.first() {
                                    bar.set_message(render_index_line(&state_clone, smoothed));
                                }
                                if let Some(bar) = bars_clone.get(1) {
                                    bar.set_message(render_data_line(&state_clone, smoothed, eta));
                                }
                                if let Some(bar) = bars_clone.get(2) {
                                    bar.set_message(wire_line);
                                }
                                if let Some(bar) = bars_clone.get(3) {
                                    bar.set_message(render_overall_line(&state_clone));
                                }
                            }
                        }
                    }
                    _ = stop_clone.notified() => break,
                }
            }
        });

        Self {
            layout,
            mp,
            bars,
            state,
            rate,
            stop,
            tick_handle: Some(tick_handle),
            finished: false,
        }
    }

    /// Stop the tick task and clear the bars without consuming `self`.
    /// Idempotent — calling twice is a no-op.
    pub async fn finish_initial(&mut self) {
        if self.finished {
            return;
        }
        self.stop.notify_one();
        if let Some(handle) = self.tick_handle.take() {
            let _ = handle.await;
        }
        for bar in &self.bars {
            bar.finish_with_message("");
        }
        self.finished = true;
    }

    /// Full cleanup, consuming `self`.
    pub async fn finish(mut self) {
        self.finish_initial().await;
    }

    /// Stop the tick task and remove the bar lines from the terminal,
    /// consuming `self`. Used for transient per-operation bars where we
    /// don't want empty placeholder lines left behind.
    pub async fn finish_and_clear(mut self) {
        if !self.finished {
            self.stop.notify_one();
            if let Some(handle) = self.tick_handle.take() {
                let _ = handle.await;
            }
            self.finished = true;
        }
        for bar in &self.bars {
            bar.finish_and_clear();
        }
    }

    pub fn state(&self) -> &Arc<ProgressState> {
        &self.state
    }
}

impl Drop for BarRenderer {
    fn drop(&mut self) {
        // Cancellation signal is sync-safe; the spawned tick task will
        // observe it on its next select poll. We do NOT await here.
        self.stop.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    fn put_v1_state() -> Arc<ProgressState> {
        ProgressState::new(Phase::Put, 1, Arc::<str>::from("file.txt"))
    }

    fn put_v2_state() -> Arc<ProgressState> {
        ProgressState::new(Phase::Put, 2, Arc::<str>::from("file.txt"))
    }

    fn get_v2_state() -> Arc<ProgressState> {
        ProgressState::new(Phase::Get, 2, Arc::<str>::from("file.txt"))
    }

    #[tokio::test]
    async fn single_layout_for_v1() {
        let renderer = BarRenderer::new(put_v1_state());
        assert_eq!(renderer.layout, BarLayout::Single);
        // 2 bars: main line + wire-stats line.
        assert_eq!(renderer.bars.len(), 2);
        assert!(renderer.mp.is_some());
        renderer.finish().await;
    }

    #[tokio::test]
    async fn multi_layout_for_v2_get() {
        let renderer = BarRenderer::new(get_v2_state());
        assert_eq!(renderer.layout, BarLayout::V2GetMulti);
        // 4 bars: index + data + wire + overall.
        assert_eq!(renderer.bars.len(), 4);
        assert!(renderer.mp.is_some());
        renderer.finish().await;
    }

    #[tokio::test]
    async fn single_layout_for_v2_put() {
        let renderer = BarRenderer::new(put_v2_state());
        assert_eq!(renderer.layout, BarLayout::Single);
        assert_eq!(renderer.bars.len(), 2);
        assert!(renderer.mp.is_some());
        renderer.finish().await;
    }

    #[tokio::test]
    async fn finish_initial_idempotent() {
        let mut renderer = BarRenderer::new(put_v1_state());
        renderer.finish_initial().await;
        renderer.finish_initial().await;
        assert!(renderer.finished);
    }

    #[tokio::test]
    async fn finish_completes_within_timeout() {
        let renderer = BarRenderer::new(get_v2_state());
        let result = timeout(Duration::from_millis(500), renderer.finish()).await;
        assert!(result.is_ok(), "finish() should complete within 500ms");
    }
}
