#![allow(dead_code)]

//! `ProgressReporter` — enum facade over the four progress channels.
//!
//! The rest of the codebase only ever interacts with `ProgressReporter`.
//! Construction picks a variant based on `ProgressMode`, and lifecycle /
//! event-dispatch methods fan out to the underlying renderer (or no-op
//! for `Off`). The `Bar` and `Log` renderers run their own internal tick
//! tasks; the `Json` variant is caller-driven and owns a `RateCalculator`
//! so it can fill the rate/eta fields on each progress snapshot.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::Mutex;

use crate::cmd::deaddrop::progress::{
    bar::BarRenderer,
    json::JsonEmitter,
    log::PeriodicLogRenderer,
    mode::ProgressMode,
    rate::RateCalculator,
    state::ProgressState,
};

pub enum ProgressReporter {
    Bar(BarRenderer),
    Log(PeriodicLogRenderer),
    Json {
        emitter: JsonEmitter,
        rate: Arc<Mutex<RateCalculator>>,
    },
    Off,
}

impl ProgressReporter {
    pub fn new(mode: ProgressMode, state: Arc<ProgressState>) -> Self {
        match mode {
            ProgressMode::Bar => Self::Bar(BarRenderer::new(state)),
            ProgressMode::PeriodicLog => Self::Log(PeriodicLogRenderer::new(state)),
            ProgressMode::Json => Self::Json {
                emitter: JsonEmitter::new(state),
                rate: Arc::new(Mutex::new(RateCalculator::new())),
            },
            ProgressMode::Off => Self::Off,
        }
    }

    /// Stop the tick task; leave `self` alive for the PUT refresh-loop
    /// handoff. For Json, emit a `done` event since there is no tick to
    /// stop. Off is a no-op.
    pub async fn finish_initial(&mut self) {
        match self {
            Self::Bar(r) => r.finish_initial().await,
            Self::Log(r) => r.finish_initial().await,
            Self::Json { emitter, .. } => emitter.emit_done(),
            Self::Off => {}
        }
    }

    /// Full shutdown — consumes `self`.
    pub async fn finish(self) {
        match self {
            Self::Bar(r) => r.finish().await,
            Self::Log(r) => r.finish().await,
            Self::Json { emitter, .. } => emitter.emit_done(),
            Self::Off => {}
        }
    }

    /// Called after each data chunk is fetched/stored. Bar and Log
    /// renderers have their own internal tick — this is a no-op for
    /// them. The caller drives explicit progress emission for Json via
    /// `emit_progress_snapshot`.
    pub fn on_chunk_done(&self) {}

    /// Called after each index chunk is fetched (v2 GET). Same as
    /// `on_chunk_done` — internal tick handles Bar/Log; caller drives
    /// Json.
    pub fn on_index_done(&self) {}

    /// Emit a `start` event. Json only; other variants no-op.
    pub fn on_start(&self) {
        if let Self::Json { emitter, .. } = self {
            emitter.emit_start();
        }
    }

    /// Signal completion to the active channel. Equivalent to
    /// `finish_initial` — emits a Json `done` event or stops the
    /// renderer tick task.
    pub async fn on_done(&mut self) {
        self.finish_initial().await;
    }

    /// Emit a `put_result` event with the assembled pickup key. Json
    /// only.
    pub fn on_put_result(&self, key: &str) {
        if let Self::Json { emitter, .. } = self {
            emitter.emit_put_result(key);
        }
    }

    /// Emit a `get_result` event. Json only.
    pub fn on_get_result(&self, bytes: u64, crc: &str, output: Option<&str>) {
        if let Self::Json { emitter, .. } = self {
            emitter.emit_get_result(bytes, crc, output);
        }
    }

    /// Emit an `ack` event for a notify-pickup. Json only.
    pub fn on_ack(&self, pickup_number: u64, peer: &str) {
        if let Self::Json { emitter, .. } = self {
            emitter.emit_ack(pickup_number, peer);
        }
    }

    /// Emit a periodic progress snapshot for the Json channel. Bar/Log
    /// have their own tick tasks and ignore this; Off no-ops. The
    /// caller is expected to invoke this from the orchestrator's tick
    /// loop so the rate/eta fields stay fresh.
    pub async fn emit_progress_snapshot(&self) {
        if let Self::Json { emitter, rate } = self {
            let now = std::time::Instant::now();
            let bytes_done = emitter.state.bytes_done.load(Ordering::Relaxed);
            let total = emitter.state.bytes_total.load(Ordering::Relaxed);
            let mut r = rate.lock().await;
            r.record(now, bytes_done);
            let rate_bps = r.rate_bps();
            let eta = r.eta_secs(total, bytes_done);
            drop(r);
            emitter.emit_progress(rate_bps, eta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::deaddrop::progress::state::Phase;

    fn make_state() -> Arc<ProgressState> {
        let s = ProgressState::new(Phase::Put, 1, Arc::<str>::from("test.bin"));
        s.set_length(1000, 0, 2);
        s
    }

    #[test]
    fn off_variant_constructs() {
        let r = ProgressReporter::new(ProgressMode::Off, make_state());
        assert!(matches!(r, ProgressReporter::Off));
    }

    #[tokio::test]
    async fn off_variant_lifecycle_is_noop() {
        let mut r = ProgressReporter::new(ProgressMode::Off, make_state());
        r.finish_initial().await;
        r.finish().await;
    }

    #[tokio::test]
    async fn off_event_methods_noop() {
        let r = ProgressReporter::new(ProgressMode::Off, make_state());
        r.on_start();
        r.on_chunk_done();
        r.on_index_done();
        r.on_put_result("key");
        r.on_get_result(100, "crc", None);
        r.on_ack(1, "peer");
        r.emit_progress_snapshot().await;
    }

    #[test]
    fn json_variant_constructs() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        assert!(matches!(r, ProgressReporter::Json { .. }));
    }

    #[tokio::test]
    async fn json_on_start_no_panic() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.on_start();
    }

    #[tokio::test]
    async fn json_on_put_result_no_panic() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.on_put_result("abc123key");
    }

    #[tokio::test]
    async fn json_on_get_result_no_panic() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.on_get_result(5000, "deadbeef", Some("/tmp/out.bin"));
    }

    #[tokio::test]
    async fn json_on_ack_no_panic() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.on_ack(2, "peer-id");
    }

    #[tokio::test]
    async fn json_emit_progress_snapshot_no_panic() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.emit_progress_snapshot().await;
    }

    #[tokio::test]
    async fn json_finish_initial_emits_done() {
        let mut r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.finish_initial().await;
    }

    #[tokio::test]
    async fn json_finish_consumes_and_emits_done() {
        let r = ProgressReporter::new(ProgressMode::Json, make_state());
        r.finish().await;
    }

    #[tokio::test]
    async fn bar_variant_constructs() {
        let r = ProgressReporter::new(ProgressMode::Bar, make_state());
        assert!(matches!(r, ProgressReporter::Bar(_)));
        r.finish().await;
    }

    #[tokio::test]
    async fn log_variant_constructs() {
        let r = ProgressReporter::new(ProgressMode::PeriodicLog, make_state());
        assert!(matches!(r, ProgressReporter::Log(_)));
        r.finish().await;
    }

    #[tokio::test]
    async fn bar_finish_initial_then_finish() {
        let mut r = ProgressReporter::new(ProgressMode::Bar, make_state());
        r.finish_initial().await;
        r.finish().await;
    }
}
