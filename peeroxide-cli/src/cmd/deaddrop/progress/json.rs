#![allow(dead_code)]

//! `JsonEmitter` — on-demand stdout JSON-Lines event emitter.
//!
//! The emitter is synchronous: the orchestrator calls `emit_*` helpers
//! explicitly for start, per-chunk progress, result, ack, and done events.
//! There is no background tick task. Each event is serialized to JSON and
//! written to stdout with a trailing newline via `println!`. JSON events
//! own stdout per the docs convention; bar/log renderers own stderr.

use std::sync::Arc;

use super::events::{
    ProgressEvent, ack, get_result, put_result, snapshot_done, snapshot_progress, snapshot_start,
};
use super::state::ProgressState;

pub struct JsonEmitter {
    pub state: Arc<ProgressState>,
}

impl JsonEmitter {
    pub fn new(state: Arc<ProgressState>) -> Self {
        Self { state }
    }

    /// Serialize event to JSON and write to stdout with a trailing newline.
    /// Silently no-ops on serialization failure (the channel must not panic).
    pub fn emit(&self, event: &ProgressEvent<'_>) {
        if let Ok(json) = serde_json::to_string(event) {
            println!("{}", json);
        }
        // silently no-op on serialization failure
    }

    pub fn emit_start(&self) {
        let event = snapshot_start(&self.state);
        self.emit(&event);
    }

    pub fn emit_progress(&self, rate: f64, eta: Option<f64>) {
        let event = snapshot_progress(&self.state, rate, eta);
        self.emit(&event);
    }

    pub fn emit_done(&self) {
        let event = snapshot_done(&self.state);
        self.emit(&event);
    }

    pub fn emit_put_result(&self, pickup_key: &str) {
        let bytes = self
            .state
            .bytes_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let chunks = self
            .state
            .data_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let event = put_result(
            self.state.phase,
            self.state.version,
            pickup_key.to_string(),
            bytes,
            chunks,
        );
        self.emit(&event);
    }

    pub fn emit_get_result(&self, bytes: u64, crc: &str, output: Option<&str>) {
        let output_str = output.unwrap_or("stdout").to_string();
        let event = get_result(
            self.state.phase,
            self.state.version,
            bytes,
            crc.to_string(),
            output_str,
        );
        self.emit(&event);
    }

    pub fn emit_ack(&self, pickup_number: u64, peer: &str) {
        let event = ack(pickup_number, peer.to_string());
        self.emit(&event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::deaddrop::progress::state::Phase;

    fn make_state(phase: Phase) -> Arc<ProgressState> {
        let state = ProgressState::new(phase, 2, Arc::<str>::from("file.txt"));
        state.set_length(1000, 2, 3);
        state
    }

    #[test]
    fn emit_silently_handles_serialization() {
        let emitter = JsonEmitter::new(make_state(Phase::Put));
        emitter.emit_start();
    }

    #[test]
    fn emit_progress_no_panic() {
        let state = make_state(Phase::Put);
        state.inc_data(100);
        let emitter = JsonEmitter::new(state);
        emitter.emit_progress(50.0, Some(18.0));
    }

    #[test]
    fn emit_done_no_panic() {
        let emitter = JsonEmitter::new(make_state(Phase::Put));
        emitter.emit_done();
    }

    #[test]
    fn emit_put_result_no_panic() {
        let emitter = JsonEmitter::new(make_state(Phase::Put));
        emitter.emit_put_result("abc123deadbeef");
    }

    #[test]
    fn emit_get_result_no_panic() {
        let emitter = JsonEmitter::new(make_state(Phase::Get));
        emitter.emit_get_result(5000, "deadbeef", Some("/tmp/out.bin"));
    }

    #[test]
    fn emit_get_result_stdout_default_no_panic() {
        let emitter = JsonEmitter::new(make_state(Phase::Get));
        emitter.emit_get_result(5000, "deadbeef", None);
    }

    #[test]
    fn emit_ack_no_panic() {
        let emitter = JsonEmitter::new(make_state(Phase::Put));
        emitter.emit_ack(1, "abc");
    }
}
