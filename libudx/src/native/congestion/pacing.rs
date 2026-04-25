//! Token-bucket pacer for BBR send-rate gating.

use std::time::{Duration, Instant};

/// Token bucket pacing for BBR congestion control.
/// Matches C libudx's `tb_available` / `tb_last_refill_ms` fields.
/// Pure computation — no tokio timers.
#[derive(Debug, Clone)]
pub(crate) struct TokenBucket {
    /// Bytes currently available to send.
    available: u32,
    /// Last time tokens were refilled.
    last_refill: Option<Instant>,
    /// Pacing rate set by BBR (bytes per millisecond).
    rate_bytes_per_ms: u32,
    /// Maximum burst size (cap on available tokens).
    max_burst: u32,
}

impl TokenBucket {
    /// Create a new token bucket with initial burst of 2 * MSS.
    pub(crate) fn new(mss: u32) -> Self {
        Self {
            available: 2 * mss,
            last_refill: None,
            rate_bytes_per_ms: 0,
            max_burst: 2 * mss,
        }
    }

    /// Set the pacing rate (called when BBR updates pacing).
    pub(crate) fn set_rate(&mut self, bytes_per_ms: u32) {
        self.rate_bytes_per_ms = bytes_per_ms;
    }

    /// Refill tokens based on elapsed time since last refill.
    pub(crate) fn refill(&mut self, now: Instant) {
        if let Some(last) = self.last_refill {
            if self.rate_bytes_per_ms > 0 {
                let elapsed_ms = now.duration_since(last).as_millis() as u32;
                let tokens = elapsed_ms.saturating_mul(self.rate_bytes_per_ms);
                self.available = self.available.saturating_add(tokens).min(self.max_burst);
            }
        }
        self.last_refill = Some(now);
    }

    /// Try to consume bytes. Returns true if enough tokens available.
    /// Refills first, then checks.
    pub(crate) fn try_consume(&mut self, bytes: u32, now: Instant) -> bool {
        self.refill(now);
        if self.available >= bytes {
            self.available -= bytes;
            true
        } else {
            false
        }
    }

    /// Compute delay until enough tokens are available for `bytes`.
    /// Returns Duration::ZERO if tokens already available or rate is 0.
    pub(crate) fn delay_for(&self, bytes: u32, now: Instant) -> Duration {
        if self.rate_bytes_per_ms == 0 || self.available >= bytes {
            return Duration::ZERO;
        }

        let deficit = bytes - self.available;
        // Account for tokens that will be refilled by now
        let effective_deficit = if let Some(last) = self.last_refill {
            let elapsed_ms = now.duration_since(last).as_millis() as u32;
            let pending_tokens = elapsed_ms.saturating_mul(self.rate_bytes_per_ms);
            deficit.saturating_sub(pending_tokens)
        } else {
            deficit
        };

        if effective_deficit == 0 {
            return Duration::ZERO;
        }

        // Ceiling division: (deficit + rate - 1) / rate
        let delay_ms = effective_deficit.div_ceil(self.rate_bytes_per_ms);
        Duration::from_millis(delay_ms as u64)
    }

    /// Reset the bucket (full burst available).
    pub(crate) fn reset(&mut self, now: Instant) {
        self.available = self.max_burst;
        self.last_refill = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_initial_burst() {
        let tb = TokenBucket::new(1180);
        assert_eq!(tb.available, 2360);
    }

    #[test]
    fn test_try_consume_success() {
        let mut tb = TokenBucket::new(1180);
        let now = Instant::now();

        assert!(tb.try_consume(2360, now));
        assert_eq!(tb.available, 0);
    }

    #[test]
    fn test_try_consume_failure() {
        let mut tb = TokenBucket::new(1180);
        let now = Instant::now();

        assert!(!tb.try_consume(2361, now));
        assert_eq!(tb.available, 2360);
    }

    #[test]
    fn test_set_rate_and_refill() {
        let mut tb = TokenBucket::new(1180);
        let now = Instant::now();
        assert!(tb.try_consume(1000, now));

        tb.set_rate(100);
        let later = now + Duration::from_millis(10);
        tb.refill(later);

        assert_eq!(tb.available, 2360);
    }

    #[test]
    fn test_delay_when_exhausted() {
        let mut tb = TokenBucket::new(1180);
        let now = Instant::now();
        assert!(tb.try_consume(2360, now));

        tb.set_rate(100);
        let delay = tb.delay_for(1180, now);

        assert_eq!(delay, Duration::from_millis(12));
    }

    #[test]
    fn test_delay_zero_when_available() {
        let tb = TokenBucket::new(1180);
        let now = Instant::now();

        assert_eq!(tb.delay_for(1180, now), Duration::ZERO);
    }

    #[test]
    fn test_delay_zero_when_rate_zero() {
        let mut tb = TokenBucket::new(1180);
        let now = Instant::now();
        assert!(tb.try_consume(2360, now));

        assert_eq!(tb.delay_for(1180, now), Duration::ZERO);
    }

    #[test]
    fn test_reset() {
        let mut tb = TokenBucket::new(1180);
        let now = Instant::now();
        assert!(tb.try_consume(500, now));

        tb.reset(now + Duration::from_millis(1));

        assert_eq!(tb.available, 2360);
        assert!(tb.last_refill.is_some());
    }
}
