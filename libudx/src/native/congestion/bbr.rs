//! BBR congestion control state machine.
//! Exact port of C libudx's `udx_bbr.c`.

use std::time::Instant;

use super::{
    filter::WindowedMaxFilter,
    rate::RateSample,
    BbrMode, CaState,
    BBR_BW_FILTER_CYCLES, BBR_CYCLE_LEN,
    BBR_MIN_RTT_INTERVAL_MS, BBR_MIN_PROBE_RTT_MODE_MS,
    BBR_PACING_MARGIN_PERCENT, BBR_HIGH_GAIN, BBR_DRAIN_GAIN, BBR_CWND_GAIN, BBR_PACING_GAIN,
    BBR_CWND_MIN_TARGET, BBR_FULL_BW_THRESH, BBR_FULL_BW_COUNT,
    BBR_EXTRA_ACKED_GAIN, BBR_EXTRA_ACKED_WIN_RTTS,
};

/// Full BBR state machine.
///
/// Unit conventions: BW = packets/ms, CWND = packets, pacing = bytes/ms.
#[derive(Debug, Clone)]
pub(crate) struct BbrState {
    mode: BbrMode,
    prev_ca_state: CaState,
    // Bandwidth estimation
    bw_filter: WindowedMaxFilter,
    bw: f64,
    // Round tracking
    round_count: u32,
    round_start: bool,
    next_rtt_delivered: u32,
    // Startup
    full_bw: f64,
    full_bw_count: u32,
    full_bw_reached: bool,
    // PROBE_BW
    cycle_index: u32,
    cycle_stamp: Option<Instant>,
    // PROBE_RTT
    min_rtt_ms: u32,
    min_rtt_stamp: Option<Instant>,
    probe_rtt_done_stamp: Option<Instant>,
    probe_rtt_round_done: bool,
    idle_restart: bool,
    // Gains (current)
    pacing_gain: f64,
    cwnd_gain: f64,
    // Recovery
    prior_cwnd: u32,
    packet_conservation: bool,
    // Ack aggregation
    extra_acked: [u32; 2],
    extra_acked_win_rtts: u32,
    extra_acked_win_rtts_thresh: u32,
    ack_epoch_acked: u32,
    ack_epoch_stamp: Option<Instant>,
    extra_acked_idx: usize,
}

impl BbrState {
    /// Initialise BBR state. Corresponds to `bbr_init()` in C.
    pub(crate) fn new(initial_rtt_ms: u32) -> Self {
        Self {
            mode: BbrMode::Startup,
            prev_ca_state: CaState::Open,
            bw_filter: WindowedMaxFilter::new(),
            bw: 0.0,
            round_count: 0,
            round_start: false,
            next_rtt_delivered: 0,
            full_bw: 0.0,
            full_bw_count: 0,
            full_bw_reached: false,
            cycle_index: 0,
            cycle_stamp: None,
            min_rtt_ms: initial_rtt_ms,
            min_rtt_stamp: Some(Instant::now()),
            probe_rtt_done_stamp: None,
            probe_rtt_round_done: false,
            idle_restart: false,
            pacing_gain: BBR_HIGH_GAIN,
            cwnd_gain: BBR_HIGH_GAIN,
            prior_cwnd: 0,
            packet_conservation: false,
            extra_acked: [0; 2],
            extra_acked_win_rtts: 0,
            extra_acked_win_rtts_thresh: BBR_EXTRA_ACKED_WIN_RTTS,
            ack_epoch_acked: 0,
            ack_epoch_stamp: None,
            extra_acked_idx: 0,
        }
    }

    /// Returns the current BBR mode.
    pub(crate) fn mode(&self) -> BbrMode {
        self.mode
    }

    /// Main BBR update on ACK. Returns `(new_cwnd, new_pacing_bytes_per_ms)`.
    /// Corresponds to `bbr_main()` in C.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_ack(
        &mut self,
        rs: &RateSample,
        acked: u32,
        cwnd: u32,
        mss: u32,
        delivered: u32,
        lost: u32,
        inflight: u32,
        ca_state: CaState,
        now: Instant,
    ) -> (u32, u32, Option<u32>) {
        let ssthresh_out = self.update_model(rs, cwnd, mss, delivered, lost, inflight, now);
        let pacing = self.set_pacing_rate(self.bw, mss, self.pacing_gain);
        let new_cwnd = self.set_cwnd(rs, acked, cwnd, self.bw, self.cwnd_gain, self.min_rtt_ms, inflight, mss, ca_state, delivered);
        (new_cwnd, pacing, ssthresh_out)
    }

    /// Handle RTO event. Corresponds to `bbr_on_rto()` in C.
    pub(crate) fn on_rto(&mut self) {
        self.full_bw = 0.0;
        self.full_bw_count = 0;
        self.full_bw_reached = false;
        self.round_start = true;
        self.prev_ca_state = CaState::Loss;
    }

    /// Handle start of a transmit opportunity.
    pub(crate) fn on_transmit_start(&mut self, app_limited: bool, _mss: u32, now: Instant) {
        if app_limited {
            self.idle_restart = true;
            self.ack_epoch_stamp = Some(now);
            self.ack_epoch_acked = 0;
        }
    }

    /// Returns the cwnd to save (used as ssthresh). Corresponds to `bbr_save_cwnd()` in C.
    pub(crate) fn save_cwnd(&mut self, cwnd: u32) {
        if self.prev_ca_state < CaState::Recovery && self.mode != BbrMode::ProbeRtt {
            self.prior_cwnd = cwnd;
        } else {
            self.prior_cwnd = self.prior_cwnd.max(cwnd);
        }
    }

    /// Returns the current minimum RTT in ms; never returns 0.
    pub(crate) fn min_rtt_ms(&self) -> u32 {
        if self.min_rtt_ms == 0 { 1 } else { self.min_rtt_ms }
    }
}

// ─── Private update pipeline ─────────────────────────────────────────────────

impl BbrState {
    /// Full model update. Inner body of `bbr_main()`.
    #[allow(clippy::too_many_arguments)]
    fn update_model(
        &mut self,
        rs: &RateSample,
        cwnd: u32,
        mss: u32,
        delivered: u32,
        _lost: u32,
        inflight: u32,
        now: Instant,
    ) -> Option<u32> {
        self.update_bw(rs, delivered);
        self.update_ack_aggregation(rs, cwnd, now);
        self.update_cycle_phase(rs, inflight, self.min_rtt_ms, now);
        self.check_full_bw_reached(rs);
        let ssthresh_out = self.check_drain(inflight, mss, now);
        self.update_min_rtt(rs, cwnd, inflight, now);
        self.update_gains();
        ssthresh_out
    }

    /// Update bandwidth estimate and round counter.
    fn update_bw(&mut self, rs: &RateSample, delivered: u32) {
        // Round detection: a new round starts when we ACK past the RTT boundary.
        self.round_start = rs.prior_delivered >= self.next_rtt_delivered;
        if self.round_start {
            self.round_count = self.round_count.wrapping_add(1);
            self.next_rtt_delivered = delivered;
        }

        if rs.delivered == 0 || rs.interval_ms == 0 {
            return;
        }

        let delivery_rate = rs.delivered as f64 / rs.interval_ms as f64;
        // Only update filter if not app-limited, or if the new rate beats the current max.
        if !rs.is_app_limited || delivery_rate > self.bw_filter.get() {
            self.bw_filter.update(delivery_rate, self.round_count, BBR_BW_FILTER_CYCLES);
            self.bw = self.bw_filter.get();
        }
    }

    /// Update ack aggregation accounting.
    fn update_ack_aggregation(&mut self, rs: &RateSample, cwnd: u32, now: Instant) {
        if rs.acked_sacked == 0 || rs.delivered == 0 || rs.interval_ms == 0 {
            return;
        }

        // On new round: advance window slot if threshold reached.
        if self.round_start {
            self.extra_acked_win_rtts = self.extra_acked_win_rtts.saturating_add(1).min(0xff);
            if self.extra_acked_win_rtts >= self.extra_acked_win_rtts_thresh {
                self.extra_acked_win_rtts = 0;
                // Rotate: discard old window, start fresh slot.
                self.extra_acked_idx ^= 1;
                self.extra_acked[self.extra_acked_idx] = 0;
            }
        }

        // Initialise epoch on first call.
        if self.ack_epoch_stamp.is_none() {
            self.ack_epoch_stamp = Some(now);
            self.ack_epoch_acked = 0;
        }

        let elapsed_ms = self
            .ack_epoch_stamp
            .and_then(|stamp| now.checked_duration_since(stamp))
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut expected_acked = (elapsed_ms as f64 * self.bw) as u32;

        if self.ack_epoch_acked <= expected_acked || (self.ack_epoch_acked as u64).saturating_add(rs.acked_sacked as u64) >= (u32::MAX as u64) /* bbr_ack_epoch_acked_reset_thresh = UINT32_MAX in C */ {
            self.ack_epoch_acked = 0;
            self.ack_epoch_stamp = Some(now);
            expected_acked = 0;
        }

        self.ack_epoch_acked = self.ack_epoch_acked.saturating_add(rs.acked_sacked);
        let extra_acked = self.ack_epoch_acked.saturating_sub(expected_acked).min(cwnd);

        if extra_acked > self.extra_acked[self.extra_acked_idx] {
            self.extra_acked[self.extra_acked_idx] = extra_acked;
        }
    }

    /// Advance the ProbeBw cycle phase when conditions are met.
    fn update_cycle_phase(&mut self, rs: &RateSample, inflight: u32, min_rtt_ms: u32, now: Instant) {
        if self.mode != BbrMode::ProbeBw {
            return;
        }

        let elapsed_enough = self
            .cycle_stamp
            .and_then(|stamp| now.checked_duration_since(stamp))
            .map(|d| d.as_millis() as u64 > min_rtt_ms as u64)
            .unwrap_or(true);

        if self.pacing_gain == 1.0 {
            if elapsed_enough {
                self.advance_cycle_phase(now);
            }
            return;
        }

        if self.pacing_gain > 1.0 {
            let target = self.target_cwnd(self.bw, self.pacing_gain, min_rtt_ms, 0);
            if elapsed_enough && (rs.losses > 0 || inflight > target) {
                self.advance_cycle_phase(now);
            }
            return;
        }

        let target = self.target_cwnd(self.bw, 1.0, min_rtt_ms, 0);
        if elapsed_enough || inflight <= target {
            self.advance_cycle_phase(now);
        }
    }

    /// Check whether full bandwidth has been reached (Startup only).
    fn check_full_bw_reached(&mut self, rs: &RateSample) {
        if self.full_bw_reached || !self.round_start || rs.is_app_limited {
            return;
        }
        if self.bw >= self.full_bw * BBR_FULL_BW_THRESH {
            // BW grew by threshold: reset baseline.
            self.full_bw = self.bw;
            self.full_bw_count = 0;
            return;
        }
        self.full_bw_count += 1;
        self.full_bw_reached = self.full_bw_count >= BBR_FULL_BW_COUNT;
    }

    /// Transition from Drain to ProbeBw once the queue has drained.
    fn check_drain(&mut self, inflight: u32, mss: u32, now: Instant) -> Option<u32> {
        let mut ssthresh_out = None;
        if self.mode == BbrMode::Startup && self.full_bw_reached {
            tracing::debug!(from = ?BbrMode::Startup, to = ?BbrMode::Drain, "bbr: mode transition");
            self.mode = BbrMode::Drain;
            ssthresh_out = Some(self.target_cwnd(self.bw, 1.0, self.min_rtt_ms, mss));
        }
        if self.mode == BbrMode::Drain {
            let target = self.target_cwnd(self.bw, 1.0, self.min_rtt_ms, mss);
            if inflight <= target {
                tracing::debug!(from = ?BbrMode::Drain, to = ?BbrMode::ProbeBw, "bbr: mode transition");
                self.mode = BbrMode::ProbeBw;
                self.cycle_index = 3;
                self.advance_cycle_phase(now); // → cycle_index becomes 4
            }
        }
        ssthresh_out
    }

    /// Update minimum RTT estimate and handle ProbeRtt entry / exit.
    fn update_min_rtt(&mut self, rs: &RateSample, cwnd: u32, inflight: u32, now: Instant) {
        let min_rtt_expired = self
            .min_rtt_stamp
            .and_then(|stamp| now.checked_duration_since(stamp))
            .map(|d| d.as_millis() as u64 > BBR_MIN_RTT_INTERVAL_MS)
            .unwrap_or(false);

        if rs.rtt_ms > 0 && (self.min_rtt_ms == 0 || rs.rtt_ms < self.min_rtt_ms || min_rtt_expired) {
            self.min_rtt_ms = rs.rtt_ms;
            self.min_rtt_stamp = Some(now);
        }

        // Enter ProbeRtt if the min-RTT sample has gone stale.
        if min_rtt_expired && !self.idle_restart && self.mode != BbrMode::ProbeRtt {
            tracing::debug!(from = ?self.mode, to = ?BbrMode::ProbeRtt, "bbr: mode transition");
            self.mode = BbrMode::ProbeRtt;
            self.save_cwnd(cwnd);
            self.probe_rtt_done_stamp = None;
            self.probe_rtt_round_done = false;
        }

        if self.mode == BbrMode::ProbeRtt {
            // Keep min_rtt_stamp fresh during probing.
            self.min_rtt_stamp = Some(now);

            if inflight <= BBR_CWND_MIN_TARGET {
                if self.probe_rtt_done_stamp.is_none() {
                    // Start the dwell timer.
                    self.probe_rtt_done_stamp = Some(now);
                    self.probe_rtt_round_done = false;
                } else {
                    if self.round_start {
                        self.probe_rtt_round_done = true;
                    }

                    let dwell_done = self
                        .probe_rtt_done_stamp
                        .and_then(|stamp| now.checked_duration_since(stamp))
                        .map(|d| d.as_millis() as u64 > BBR_MIN_PROBE_RTT_MODE_MS)
                        .unwrap_or(false);

                    if dwell_done && self.probe_rtt_round_done {
                        // Exit ProbeRtt.
                        self.probe_rtt_done_stamp = None;
                        self.probe_rtt_round_done = false;
                        self.min_rtt_stamp = Some(now);
                        if self.full_bw_count > 0 {
                            // Full BW already reached: enter ProbeBw.
                            tracing::debug!(from = ?BbrMode::ProbeRtt, to = ?BbrMode::ProbeBw, "bbr: mode transition");
                            self.mode = BbrMode::ProbeBw;
                            self.cycle_index = 3;
                            self.advance_cycle_phase(now);
                        } else {
                            tracing::debug!(from = ?BbrMode::ProbeRtt, to = ?BbrMode::Startup, "bbr: mode transition");
                            self.mode = BbrMode::Startup;
                        }
                    }
                }
            } else {
                // Not yet drained to min target; reset dwell timer.
                self.probe_rtt_done_stamp = None;
                self.probe_rtt_round_done = false;
            }
        }

        if rs.delivered > 0 {
            self.idle_restart = false;
        }
    }

    /// Refresh `pacing_gain` / `cwnd_gain` from the current mode.
    fn update_gains(&mut self) {
        match self.mode {
            BbrMode::Startup => {
                self.pacing_gain = BBR_HIGH_GAIN;
                self.cwnd_gain = BBR_HIGH_GAIN;
            }
            BbrMode::Drain => {
                self.pacing_gain = BBR_DRAIN_GAIN;
                self.cwnd_gain = BBR_HIGH_GAIN;
            }
            BbrMode::ProbeBw => {
                self.pacing_gain = BBR_PACING_GAIN[self.cycle_index as usize];
                self.cwnd_gain = BBR_CWND_GAIN;
            }
            BbrMode::ProbeRtt => {
                self.pacing_gain = 1.0;
                self.cwnd_gain = 1.0;
            }
        }
    }

    /// Compute target pacing rate (bytes/ms).
    fn set_pacing_rate(&self, bw: f64, mss: u32, pacing_gain: f64) -> u32 {
        (bw * mss as f64 * pacing_gain * BBR_PACING_MARGIN_PERCENT) as u32
    }

    /// Compute new congestion window (packets).
    #[allow(clippy::too_many_arguments)]
    fn set_cwnd(
        &mut self,
        rs: &RateSample,
        acked: u32,
        cwnd: u32,
        bw: f64,
        cwnd_gain: f64,
        min_rtt_ms: u32,
        inflight: u32,
        mss: u32,
        ca_state: CaState,
        delivered: u32,
    ) -> u32 {
        let mut new_cwnd = cwnd;
        if rs.losses > 0 {
            new_cwnd = new_cwnd.saturating_sub(rs.losses).max(1);
        }

        if ca_state == CaState::Recovery && self.prev_ca_state != CaState::Recovery {
            self.packet_conservation = true;
            self.next_rtt_delivered = delivered;
            new_cwnd = inflight.saturating_add(acked);
        } else if self.prev_ca_state >= CaState::Recovery && ca_state == CaState::Open {
            new_cwnd = new_cwnd.max(self.prior_cwnd);
            self.packet_conservation = false;
        }

        self.prev_ca_state = ca_state;

        if self.packet_conservation {
            return new_cwnd.max(inflight.saturating_add(acked)).max(BBR_CWND_MIN_TARGET);
        }

        if rs.delivered == 0 {
            return new_cwnd.max(BBR_CWND_MIN_TARGET);
        }

        let target = self
            .target_cwnd(bw, cwnd_gain, min_rtt_ms, mss)
            .saturating_add(self.extra_acked());

        if self.full_bw_reached {
            // Full BW reached: grow up to BDP-based target (matches C bbr_full_bw_reached()).
            new_cwnd = new_cwnd.saturating_add(acked).min(target);
        } else {
            // Still in Startup: grow freely.
            new_cwnd = new_cwnd.saturating_add(acked);
        }

        // In ProbeRtt, enforce a tiny window to obtain a fresh RTT sample.
        if self.mode == BbrMode::ProbeRtt {
            new_cwnd = new_cwnd.min(BBR_CWND_MIN_TARGET);
        }

        // Always enforce the floor.
        new_cwnd.max(BBR_CWND_MIN_TARGET)
    }

    /// Advance to the next ProbeBw cycle slot.
    fn advance_cycle_phase(&mut self, now: Instant) {
        self.cycle_stamp = Some(now);
        self.cycle_index = (self.cycle_index + 1) % BBR_CYCLE_LEN;
    }

    /// Compute target cwnd (packets) as BDP × gain.
    fn target_cwnd(&self, bw: f64, gain: f64, min_rtt_ms: u32, _mss: u32) -> u32 {
        let bdp = (bw * min_rtt_ms as f64) as u32;
        let cwnd = (bdp as f64 * gain) as u32;
        cwnd.max(BBR_CWND_MIN_TARGET)
    }

    /// Current extra-acked estimate (packets), scaled by `BBR_EXTRA_ACKED_GAIN`.
    fn extra_acked(&self) -> u32 {
        let max_extra = self.extra_acked[0].max(self.extra_acked[1]);
        (max_extra as f64 * BBR_EXTRA_ACKED_GAIN) as u32
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Construct a RateSample with the fields accessible from this crate.
    fn make_rs(delivered: u32, interval_ms: u32, prior_delivered: u32, app_limited: bool) -> RateSample {
        let mut rs = RateSample::default();
        rs.delivered = delivered;
        rs.interval_ms = interval_ms;
        rs.rtt_ms = interval_ms;
        rs.prior_delivered = prior_delivered;
        rs.is_app_limited = app_limited;
        rs
    }

    // ── 1. Startup gains after new() ─────────────────────────────────────────

    #[test]
    fn test_new_startup_gains() {
        let state = BbrState::new(100);
        assert_eq!(state.mode, BbrMode::Startup);
        assert_eq!(state.pacing_gain, BBR_HIGH_GAIN);
        assert_eq!(state.cwnd_gain, BBR_HIGH_GAIN);
    }

    // ── 2. Full-BW detection ─────────────────────────────────────────────────

    #[test]
    fn test_full_bw_detection() {
        let mut state = BbrState::new(100);
        let now = Instant::now();
        let mss = 1180_u32;
        let cwnd = 10_u32;

        // Round 0: establishes full_bw baseline (bw >= 0.0 * 1.25 == 0.0).
        let rs = make_rs(10, 10, 0, false);
        state.on_ack(&rs, 10, cwnd, mss, 10, 0, 5, CaState::Open, now);

        // Rounds 1-3: same rate, no 25 % growth → count increments each round.
        for i in 1_u32..=3 {
            let rs = make_rs(10, 10, i * 10, false);
            state.on_ack(&rs, 10, cwnd, mss, (i + 1) * 10, 0, 5, CaState::Open, now);
        }

        assert_eq!(state.full_bw_count, BBR_FULL_BW_COUNT);
    }

    // ── 3. Drain → ProbeBw transition ────────────────────────────────────────

    #[test]
    fn test_drain_to_probe_bw() {
        let mut state = BbrState::new(100);
        let now = Instant::now();
        let mss = 1180_u32;
        let cwnd = 10_u32;

        // Drive to Drain: 4 rounds of constant BW with inflight >> target so that
        // check_drain does not fire in the same tick as check_full_bw_reached.
        // After min_rtt_ms settles to 10 ms, target ≈ bw(1.0) × 10 × gain(2) = 20 pkts.
        for i in 0_u32..4 {
            let rs = make_rs(10, 10, i * 10, false);
            state.on_ack(&rs, 10, cwnd, mss, (i + 1) * 10, 0, 100, CaState::Open, now);
        }
        assert_eq!(state.mode, BbrMode::Drain);

        // Drop inflight below the target → transition to ProbeBw.
        let rs = make_rs(10, 10, 40, false);
        state.on_ack(&rs, 10, cwnd, mss, 50, 0, 1, CaState::Open, now);

        assert_eq!(state.mode, BbrMode::ProbeBw);
    }

    // ── 4. ProbeBw cycle advance ─────────────────────────────────────────────

    #[test]
    fn test_probe_bw_cycle_advance() {
        let mut state = BbrState::new(10);
        let now = Instant::now();
        let mss = 1180_u32;
        let cwnd = 10_u32;

        // Drive to ProbeBw.
        for i in 0_u32..4 {
            let rs = make_rs(10, 10, i * 10, false);
            state.on_ack(&rs, 10, cwnd, mss, (i + 1) * 10, 0, 5, CaState::Open, now);
        }
        let rs = make_rs(10, 10, 40, false);
        state.on_ack(&rs, 10, cwnd, mss, 50, 0, 1, CaState::Open, now);
        assert_eq!(state.mode, BbrMode::ProbeBw);

        let before = state.cycle_index;

        // Advance time past min_rtt_ms so the cycle phase advances.
        let later = now + Duration::from_millis(state.min_rtt_ms as u64 + 5);
        let rs = make_rs(10, 10, 50, false);
        state.on_ack(&rs, 10, cwnd, mss, 60, 0, 1, CaState::Open, later);

        let expected = (before + 1) % BBR_CYCLE_LEN;
        assert_eq!(state.cycle_index, expected);
    }

    // ── 5. on_rto resets ─────────────────────────────────────────────────────

    #[test]
    fn test_on_rto_resets() {
        let mut state = BbrState::new(100);
        state.on_rto();
        assert_eq!(state.full_bw, 0.0);
        assert!(state.round_start);
        assert_eq!(state.prev_ca_state, CaState::Loss);
    }

    // ── 6. save_cwnd behaviour ───────────────────────────────────────────────

    #[test]
    fn test_save_cwnd() {
        let mut state = BbrState::new(100);
        state.save_cwnd(42);
        assert_eq!(state.prior_cwnd, 42);

        let mut probe = BbrState::new(100);
        probe.mode = BbrMode::ProbeRtt;
        probe.save_cwnd(42);
        assert_eq!(probe.prior_cwnd, 42);
    }

    // ── 7. set_pacing_rate applies 0.99 margin ───────────────────────────────

    #[test]
    fn test_set_pacing_rate_margin() {
        let state = BbrState::new(100);
        // 1.0 packets/ms × 1180 bytes/pkt × 1.0 gain × 0.99 = 1168.2 → 1168
        let pacing = state.set_pacing_rate(1.0, 1180, 1.0);
        assert_eq!(pacing, 1168);
    }

    // ── 8. set_cwnd never goes below BBR_CWND_MIN_TARGET ─────────────────────

    #[test]
    fn test_set_cwnd_minimum() {
        let mut state = BbrState::new(100);
        let rs = make_rs(1, 10, 0, false);
        // cwnd=0, acked=0 → must return at least BBR_CWND_MIN_TARGET.
        let new_cwnd = state.set_cwnd(&rs, 0, 0, 0.0, BBR_CWND_GAIN, 100, 0, 1180, CaState::Open, 0);
        assert!(new_cwnd >= BBR_CWND_MIN_TARGET);
    }

    // ── 9. min_rtt_ms() never returns 0 ─────────────────────────────────────

    #[test]
    fn test_min_rtt_ms_nonzero() {
        let state = BbrState::new(0);
        assert!(state.min_rtt_ms() >= 1);
    }
}
