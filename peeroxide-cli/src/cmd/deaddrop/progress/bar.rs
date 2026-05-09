#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::cmd::deaddrop::progress::{
    format::{render_bar_line, render_data_line, render_index_line, render_overall_line},
    rate::RateCalculator,
    state::{Phase, ProgressState},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarLayout {
    Single,
    V2GetMulti,
}

/// indicatif-driven renderer that ticks a background task to refresh the
/// progress bar(s). Single-bar mode for v1 and v2 PUT, 3-bar MultiProgress
/// for v2 GET.
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

        let (mp, bars) = match layout {
            BarLayout::Single => {
                let bar = ProgressBar::new(0);
                bar.set_style(style);
                bar.enable_steady_tick(Duration::from_millis(100));
                (None, vec![bar])
            }
            BarLayout::V2GetMulti => {
                let mp = MultiProgress::new();
                let mut bars = Vec::with_capacity(3);
                for _ in 0..3 {
                    let bar = mp.add(ProgressBar::new(0));
                    bar.set_style(style.clone());
                    bar.enable_steady_tick(Duration::from_millis(100));
                    bars.push(bar);
                }
                (Some(mp), bars)
            }
        };

        let rate = Arc::new(Mutex::new(RateCalculator::new()));
        let stop = Arc::new(Notify::new());

        let stop_clone = stop.clone();
        let state_clone = state.clone();
        let rate_clone = rate.clone();
        let bars_clone = bars.clone();
        let layout_clone = layout;

        let tick_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let mut rate_guard = rate_clone.lock().await;
                        let now = std::time::Instant::now();
                        let bytes_done = state_clone.bytes_done.load(Ordering::Relaxed);
                        rate_guard.record(now, bytes_done);
                        let smoothed = rate_guard.rate_bps();
                        let total = state_clone.bytes_total.load(Ordering::Relaxed);
                        let done = state_clone.bytes_done.load(Ordering::Relaxed);
                        let eta = rate_guard.eta_secs(total, done);
                        drop(rate_guard);

                        match layout_clone {
                            BarLayout::Single => {
                                let msg = render_bar_line(&state_clone, smoothed, eta);
                                if let Some(bar) = bars_clone.first() {
                                    bar.set_message(msg);
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
        assert_eq!(renderer.bars.len(), 1);
        assert!(renderer.mp.is_none());
        renderer.finish().await;
    }

    #[tokio::test]
    async fn multi_layout_for_v2_get() {
        let renderer = BarRenderer::new(get_v2_state());
        assert_eq!(renderer.layout, BarLayout::V2GetMulti);
        assert_eq!(renderer.bars.len(), 3);
        assert!(renderer.mp.is_some());
        renderer.finish().await;
    }

    #[tokio::test]
    async fn single_layout_for_v2_put() {
        let renderer = BarRenderer::new(put_v2_state());
        assert_eq!(renderer.layout, BarLayout::Single);
        assert_eq!(renderer.bars.len(), 1);
        assert!(renderer.mp.is_none());
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
