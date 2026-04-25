//! Rate sampling for BBR congestion control.
//!
//! Port of [`udx_rate.c`](https://github.com/holepunchto/libudx/blob/main/src/udx_rate.c).
//! Tracks delivery rate by storing per-packet metadata at send time and
//! computing intervals at ACK time.

use std::time::{Duration, Instant};

/// Per-packet metadata stored at send time.
/// Matches C: pkt->first_sent_ts, pkt->delivered_ts, pkt->delivered, pkt->is_app_limited.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketRateInfo {
    /// Timestamp when the first packet in this send batch was sent.
    pub(crate) first_sent_ts: Instant,
    /// Timestamp when the last ACK was received before this packet was sent.
    pub(crate) delivered_ts: Instant,
    /// Total delivered count at the time this packet was sent.
    pub(crate) delivered: u32,
    /// Whether the app was limiting the send rate when this packet was sent.
    pub(crate) is_app_limited: bool,
}

/// Rate sample computed from ACK processing.
/// Default: all zeros/false/None.
#[derive(Debug, Clone, Default)]
pub(crate) struct RateSample {
    pub(crate) delivered: u32,
    pub(crate) interval_ms: u32,
    pub(crate) rtt_ms: u32,
    pub(crate) losses: u32,
    pub(crate) acked_sacked: u32,
    pub(crate) is_app_limited: bool,
    /// Prior delivered count (from the ACKed packet with highest delivered).
    pub(crate) prior_delivered: u32,
    /// The ACKed packet's delivered_ts (when last ACK was received before that packet was sent).
    pub(crate) prior_timestamp: Option<Instant>,
    /// The ACKed packet's first_sent_ts (when the send batch started for that packet).
    prior_first_sent_ts: Option<Instant>,
}

/// Rate sampling state tracked at the stream level.
#[derive(Debug, Clone)]
pub(crate) struct RateState {
    /// Current rate sample being built.
    pub(crate) current: RateSample,
    pub(crate) rate_delivered: u32,
    pub(crate) rate_interval_ms: u32,
}

impl RateState {
    /// Create a new rate sampling state.
    pub(crate) fn new() -> Self {
        Self {
            current: RateSample::default(),
            rate_delivered: 0,
            rate_interval_ms: 0,
        }
    }

    /// Create PacketRateInfo snapshot for a packet about to be sent.
    /// C reference: udx__rate_pkt_send()
    /// This is a static method — it just captures the current delivery state.
    pub(crate) fn on_packet_sent(
        delivered: u32,
        delivered_ts: Instant,
        first_sent_ts: Instant,
        is_app_limited: bool,
    ) -> PacketRateInfo {
        PacketRateInfo {
            first_sent_ts,
            delivered_ts,
            delivered,
            is_app_limited,
        }
    }

    /// Process an ACKed packet's rate info.
    /// Among all ACKed packets in a batch, pick the one with highest pkt_info.delivered
    /// (most recently sent). Set current.prior_delivered, prior_timestamp, is_app_limited.
    /// C reference: udx__rate_pkt_delivered()
    pub(crate) fn on_packet_delivered(
        &mut self,
        pkt_info: &PacketRateInfo,
        time_sent: Instant,
        rtt_ms: u32,
    ) -> Option<Instant> {
        if pkt_info.delivered >= self.current.prior_delivered {
            self.current.prior_delivered = pkt_info.delivered;
            self.current.prior_timestamp = Some(pkt_info.delivered_ts);
            self.current.prior_first_sent_ts = Some(pkt_info.first_sent_ts);
            self.current.is_app_limited = pkt_info.is_app_limited;
            self.current.rtt_ms = rtt_ms;
            return Some(time_sent);
        }
        None
    }

    /// Generate rate sample after all ACKs in batch processed.
    /// C reference: udx__rate_gen()
    ///
    /// delivered = stream_delivered - prior_delivered
    /// snd_ms = stream_first_sent_ts - acked_pkt_first_sent_ts
    /// ack_ms = stream_delivered_ts - acked_pkt_delivered_ts
    /// interval_ms = max(snd_ms, ack_ms)
    /// Discards if interval < min_rtt_ms (sets delivered = 0).
    pub(crate) fn generate(
        &mut self,
        stream_delivered: u32,
        stream_first_sent_ts: Instant,
        stream_delivered_ts: Instant,
        min_rtt_ms: u32,
        lost_count: u32,
        delivered_count: u32,
    ) -> &RateSample {
        self.current.losses = lost_count;
        self.current.acked_sacked = delivered_count;

        let (prior_ts, prior_first_sent) = match (
            self.current.prior_timestamp,
            self.current.prior_first_sent_ts,
        ) {
            (Some(ts), Some(fst)) => (ts, fst),
            _ => {
                self.current.delivered = 0;
                self.current.interval_ms = 0;
                return &self.current;
            }
        };

        self.current.delivered = stream_delivered.saturating_sub(self.current.prior_delivered);

        // snd_ms = stream.first_sent_ts - acked_pkt.first_sent_ts
        let snd_dur = stream_first_sent_ts
            .checked_duration_since(prior_first_sent)
            .unwrap_or(Duration::ZERO);
        let snd_ms = if snd_dur.is_zero() { 0 } else { (snd_dur.as_millis() as u32).max(1) };

        // ack_ms = stream.delivered_ts - acked_pkt.delivered_ts
        let ack_dur = stream_delivered_ts
            .checked_duration_since(prior_ts)
            .unwrap_or(Duration::ZERO);
        let ack_ms = if ack_dur.is_zero() { 0 } else { (ack_dur.as_millis() as u32).max(1) };

        self.current.interval_ms = snd_ms.max(ack_ms);

        // Discard if interval < min_rtt (too small to be meaningful)
        if self.current.interval_ms < min_rtt_ms {
            self.current.delivered = 0;
            self.current.interval_ms = 0;
            return &self.current;
        }

        let rate_delivered = self.rate_delivered;
        let rate_interval_ms = self.rate_interval_ms;
        if !self.current.is_app_limited
            || (self.current.delivered as u64 * rate_interval_ms as u64
                >= rate_delivered as u64 * self.current.interval_ms as u64)
        {
            self.rate_delivered = self.current.delivered;
            self.rate_interval_ms = self.current.interval_ms;
        }

        &self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_on_packet_sent_captures_state() {
        let now = Instant::now();
        let info = RateState::on_packet_sent(5, now, now, false);
        assert_eq!(info.delivered, 5);
        assert!(!info.is_app_limited);
    }

    #[test]
    fn test_on_packet_delivered_picks_highest() {
        let now = Instant::now();
        let mut rs = RateState::new();

        let info1 = PacketRateInfo {
            first_sent_ts: now,
            delivered_ts: now,
            delivered: 3,
            is_app_limited: false,
        };
        let info2 = PacketRateInfo {
            first_sent_ts: now,
            delivered_ts: now,
            delivered: 7,
            is_app_limited: true,
        };

        rs.on_packet_delivered(&info1, now, 0);
        rs.on_packet_delivered(&info2, now, 0);
        assert_eq!(rs.current.prior_delivered, 7);
        assert!(rs.current.is_app_limited);
    }

    #[test]
    fn test_generate_correct_interval() {
        let t0 = Instant::now();
        let t10 = t0 + Duration::from_millis(10);

        let mut rs = RateState::new();

        // Packet sent at t0 with delivered=0
        let pkt_info = PacketRateInfo {
            first_sent_ts: t0,
            delivered_ts: t0,
            delivered: 0,
            is_app_limited: false,
        };
        rs.on_packet_delivered(&pkt_info, t0, 0);

        // At t10: stream has delivered=1
        let sample = rs.generate(
            1,
            t10,
            t10,
            5,
            0,
            1,
        );

        assert_eq!(sample.delivered, 1);
        assert_eq!(sample.interval_ms, 10);
    }

    #[test]
    fn test_generate_discards_sub_rtt() {
        let t0 = Instant::now();
        let t2 = t0 + Duration::from_millis(2);

        let mut rs = RateState::new();

        let pkt_info = PacketRateInfo {
            first_sent_ts: t0,
            delivered_ts: t0,
            delivered: 0,
            is_app_limited: false,
        };
        rs.on_packet_delivered(&pkt_info, t0, 0);

        // interval=2ms < min_rtt=5ms → discard
        let sample = rs.generate(1, t2, t2, 5, 0, 1);
        assert_eq!(sample.delivered, 0);
    }

    #[test]
    fn test_generate_no_prior() {
        let now = Instant::now();
        let mut rs = RateState::new();
        // No on_packet_delivered called → no prior timestamps
        let sample = rs.generate(10, now, now, 5, 0, 10);
        assert_eq!(sample.delivered, 0);
        assert_eq!(sample.interval_ms, 0);
    }
}
