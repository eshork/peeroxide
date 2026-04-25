pub(crate) mod bbr;
pub(crate) mod rate;
pub(crate) mod filter;
pub(crate) mod pacing;

use std::time::{Duration, Instant};

/// BBR cycle length.
pub(crate) const BBR_CYCLE_LEN: u32 = 8;
/// Number of cycles used by the bandwidth filter.
pub(crate) const BBR_BW_FILTER_CYCLES: u32 = BBR_CYCLE_LEN + 2;
/// Minimum interval between min-RTT samples, in milliseconds.
pub(crate) const BBR_MIN_RTT_INTERVAL_MS: u64 = 10_000;
/// Minimum time spent in ProbeRTT, in milliseconds.
pub(crate) const BBR_MIN_PROBE_RTT_MODE_MS: u64 = 200;
/// Default pacing margin.
pub(crate) const BBR_PACING_MARGIN_PERCENT: f64 = 0.99;
/// Startup/high gain constant.
pub(crate) const BBR_HIGH_GAIN: f64 = 2.885_39;
/// Drain gain constant.
pub(crate) const BBR_DRAIN_GAIN: f64 = 1.0 / 2.885_39;
/// Congestion window gain constant.
pub(crate) const BBR_CWND_GAIN: f64 = 2.0;
/// Per-cycle pacing gain table.
pub(crate) const BBR_PACING_GAIN: [f64; 8] = [1.25, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
/// Minimum BBR cwnd target.
pub(crate) const BBR_CWND_MIN_TARGET: u32 = 4;
/// Threshold for declaring full bandwidth.
pub(crate) const BBR_FULL_BW_THRESH: f64 = 1.25;
/// Consecutive rounds required for full bandwidth.
pub(crate) const BBR_FULL_BW_COUNT: u32 = 3;
/// Extra-acked gain.
pub(crate) const BBR_EXTRA_ACKED_GAIN: f64 = 1.0;
/// Window length for extra-acked accounting, in RTTs.
pub(crate) const BBR_EXTRA_ACKED_WIN_RTTS: u32 = 5;
/// Maximum extra-acked sample age, in milliseconds.
pub(crate) const BBR_EXTRA_ACKED_MAX_MS: u64 = 100;

/// BBR operating mode, mirroring the four phases of the BBR algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BbrMode {
    /// Exponential bandwidth probing at startup.
    Startup = 0,
    /// Drain inflight back to BDP after startup overshoot.
    Drain = 1,
    /// Steady-state cycling through pacing gains.
    ProbeBw = 2,
    /// Periodic min-RTT measurement phase.
    ProbeRtt = 3,
}

/// Congestion avoidance state, tracking loss/recovery transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
pub(crate) enum CaState {
    /// Normal operation — no loss detected.
    Open = 1,
    /// Fast recovery after partial loss.
    Recovery = 2,
    /// RTO-triggered loss state.
    Loss = 3,
}

/// Congestion-control scaffold for the native libudx rewrite.
///
/// Wraps BBR state, rate sampling, and pacing into a single controller
/// that stream logic calls on send, ACK, and loss events.
pub(crate) struct CongestionController {
    /// Internal BBR algorithm state machine.
    bbr: bbr::BbrState,
    /// Delivery-rate sampling state (tracks `delivered`, timestamps, etc.).
    rate: rate::RateState,
    /// Token-bucket pacer gating outbound transmissions.
    pub(crate) pacing: pacing::TokenBucket,
    /// Cumulative count of packets acknowledged by the peer.
    pub(crate) delivered: u32,
    /// Cumulative count of packets declared lost.
    pub(crate) lost: u32,
    /// Sequence number at which the sender became application-limited (0 = not limited).
    pub(crate) app_limited: u32,
    /// Timestamp of the first packet sent in the current rate sample interval.
    pub(crate) first_sent_ts: Option<Instant>,
    /// Timestamp of the most recently delivered packet.
    pub(crate) delivered_ts: Option<Instant>,
    /// Current congestion-avoidance state (open / recovery / loss).
    pub(crate) ca_state: CaState,
    /// Highest sequence number at the time loss/recovery was entered.
    pub(crate) high_seq: u32,
    /// Current congestion window in packets.
    pub(crate) cwnd: u32,
    /// Slow-start threshold in packets.
    pub(crate) ssthresh: u32,
    /// BBR-computed pacing rate in bytes per millisecond.
    pub(crate) pacing_bytes_per_ms: u32,
    /// Maximum segment size in bytes.
    mss: u32,
}

impl CongestionController {
    /// Initial congestion window in packets — matches C libudx `udx_stream_init`.
    const INITIAL_CWND: u32 = 10;
    /// Initial slow-start threshold — matches C libudx `bbr_init` (`0xffff`).
    const INITIAL_SSTHRESH: u32 = 0xffff;

    /// Creates a congestion controller initialized with BBR state.
    pub(crate) fn new(mss: u32, _initial_rtt_ms: u32) -> Self {
        Self {
            bbr: bbr::BbrState::new(0),
            rate: rate::RateState::new(),
            pacing: pacing::TokenBucket::new(mss),
            delivered: 0,
            lost: 0,
            app_limited: u32::MAX, // initially app-limited (matches C `~0`)
            first_sent_ts: None,
            delivered_ts: None,
            ca_state: CaState::Open,
            high_seq: 0,
            cwnd: Self::INITIAL_CWND,
            ssthresh: Self::INITIAL_SSTHRESH,
            pacing_bytes_per_ms: 0,
            mss,
        }
    }

    /// Records a packet send and returns the associated rate info.
    pub(crate) fn on_packet_sent(&mut self, _seq: u32, inflight: u32, now: Instant) -> rate::PacketRateInfo {
        if inflight == 0 || self.first_sent_ts.is_none() {
            self.first_sent_ts = Some(now);
            self.delivered_ts = Some(now);
        }
        rate::RateState::on_packet_sent(
            self.delivered,
            self.delivered_ts.unwrap_or(now),
            self.first_sent_ts.unwrap_or(now),
            self.app_limited > 0,
        )
    }

    /// Processes acknowledgements and updates BBR state.
    pub(crate) fn on_ack(
        &mut self,
        acked_packets: &[(u32, rate::PacketRateInfo)],
        lost_count: u32,
        inflight: u32,
        now: Instant,
    ) {
        let acked_count = acked_packets.len() as u32;
        self.delivered += acked_count;
        self.delivered_ts = Some(now);

        // Recovery/Loss exit: once the highest ACK'd seq reaches the recovery
        // boundary, all packets from the loss epoch have been resolved.
        if self.ca_state >= CaState::Recovery {
            let max_acked = acked_packets.iter().map(|(seq, _)| *seq).max().unwrap_or(0);
            if max_acked >= self.high_seq {
                self.ca_state = CaState::Open;
            }
        }

        // Clear app-limited once enough data has been delivered (C: udx__rate_gen)
        if self.app_limited > 0 && self.delivered > self.app_limited {
            self.app_limited = 0;
        }

        // Rate sampling — clamp sub-ms RTTs to 1ms so BBR can measure on localhost
        for (_seq, info) in acked_packets {
            let dur = now
                .checked_duration_since(info.first_sent_ts)
                .unwrap_or(Duration::ZERO);
            let rtt_ms = if dur.is_zero() { 0 } else { (dur.as_millis() as u32).max(1) };
            if let Some(new_first_sent) = self.rate.on_packet_delivered(info, info.first_sent_ts, rtt_ms) {
                self.first_sent_ts = Some(new_first_sent);
            }
        }
        let min_rtt = self.bbr.min_rtt_ms();
        let rs = self
            .rate
            .generate(
                self.delivered,
                self.first_sent_ts.unwrap_or(now),
                self.delivered_ts.unwrap_or(now),
                min_rtt,
                lost_count,
                acked_count,
            )
            .clone();

        // BBR main
        let (new_cwnd, new_pacing, ssthresh_out) = self.bbr.on_ack(
            &rs,
            acked_count,
            self.cwnd,
            self.mss,
            self.delivered,
            self.lost,
            inflight,
            self.ca_state,
            now,
        );
        if let Some(ssth) = ssthresh_out {
            self.ssthresh = ssth;
        }
        self.cwnd = new_cwnd;
        self.pacing_bytes_per_ms = new_pacing;
        self.pacing.set_rate(new_pacing);

        // Mark app-limited during ProbeRtt (C: bbr_main sets app_limited after ProbeRtt)
        if self.bbr.mode() == BbrMode::ProbeRtt {
            self.app_limited = (self.delivered + inflight).max(1);
        }
        tracing::debug!(
            mode = ?self.bbr.mode(),
            cwnd = self.cwnd,
            pacing_bytes_per_ms = self.pacing_bytes_per_ms,
            delivered = self.delivered,
            inflight,
            "bbr: on_ack update"
        );
    }

    /// Records a loss event and transitions CA state.
    pub(crate) fn on_packet_lost(&mut self, _seq: u32) {
        self.lost += 1;
        if self.ca_state == CaState::Open {
            self.ca_state = CaState::Recovery;
            self.bbr.save_cwnd(self.cwnd);
            self.ssthresh = self.cwnd;
        }
    }

    /// Handles an RTO event.
    pub(crate) fn on_rto(&mut self) {
        self.ca_state = CaState::Loss;
        self.bbr.on_rto();
    }

    /// Marks the start of a transmit opportunity.
    pub(crate) fn on_transmit_start(&mut self, now: Instant) {
        self.bbr.on_transmit_start(self.app_limited > 0, self.mss, now);
        self.pacing.reset(now);
    }

    /// Returns whether sending is allowed given current inflight and remote receive window.
    /// Matches C: `send_window_in_packets = min(cwnd, send_rwnd / max_payload)`.
    pub(crate) fn can_send(&self, inflight: u32, send_rwnd: u32) -> bool {
        let rwnd_pkts = send_rwnd / self.mss;
        inflight < self.cwnd.min(rwnd_pkts)
    }

    /// Computes pacing delay for the requested byte count.
    pub(crate) fn pacing_delay(&mut self, bytes: u32, now: Instant) -> Duration {
        self.pacing.refill(now);
        self.pacing.delay_for(bytes, now)
    }

    /// Returns whether the sender is app-limited.
    pub(crate) fn is_app_limited(
        &self,
        queued_bytes: usize,
        inflight: u32,
        retransmit_pending: bool,
    ) -> bool {
        (queued_bytes as u32) < self.mss && inflight < self.cwnd && !retransmit_pending
    }

    /// Scale congestion window after an MTU change (matches C libudx).
    pub(crate) fn on_mtu_change(&mut self, old_mtu: usize, new_mtu: usize) {
        if old_mtu > 0 {
            self.cwnd = self.cwnd * new_mtu as u32 / old_mtu as u32;
        }
        self.mss = (new_mtu - crate::native::header::HEADER_SIZE) as u32;
    }

    /// Update app-limited state before each packet send (C: `udx__rate_check_app_limited`).
    pub(crate) fn check_app_limited(&mut self, queued_bytes: usize, inflight: u32) {
        if self.is_app_limited(queued_bytes, inflight, false) {
            self.app_limited = (self.delivered + inflight).max(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_does_not_panic() {
        let controller = CongestionController::new(1200, 50);
        assert_eq!(controller.ca_state, CaState::Open);
        assert_eq!(controller.cwnd, CongestionController::INITIAL_CWND);
        assert_eq!(controller.ssthresh, CongestionController::INITIAL_SSTHRESH);
    }

    #[test]
    fn test_can_send_default_true() {
        let controller = CongestionController::new(1200, 50);
        assert!(controller.can_send(0, 4_194_304));
    }

    #[test]
    fn test_pacing_delay_default_zero() {
        let mut controller = CongestionController::new(1200, 50);
        assert_eq!(controller.pacing_delay(1200, Instant::now()), Duration::ZERO);
    }

    #[test]
    fn test_full_send_ack_cycle() {
        let mut c = CongestionController::new(1180, 100);
        let now = Instant::now();
        let rate_info = c.on_packet_sent(1, 0, now);
        let later = now + std::time::Duration::from_millis(10);
        c.on_ack(&[(1, rate_info)], 0, 0, later);
        assert!(c.cwnd >= BBR_CWND_MIN_TARGET);
        assert!(c.can_send(0, 4_194_304));
        assert!(!c.can_send(c.cwnd, 4_194_304));
    }
}
