#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct RateCalculator {
    window: VecDeque<(Instant, u64)>,
    window_secs: f64,
    max_samples: usize,
}

impl RateCalculator {
    pub fn new() -> Self {
        Self::new_with_window(5.0, 200)
    }

    pub fn new_with_window(window_secs: f64, max_samples: usize) -> Self {
        Self {
            window: VecDeque::new(),
            window_secs: window_secs.max(0.0),
            max_samples,
        }
    }

    pub fn record(&mut self, now: Instant, bytes_so_far: u64) {
        self.window.push_back((now, bytes_so_far));

        let window_secs = self.window_secs;
        while let Some((instant, _)) = self.window.front() {
            let Some(age) = now.checked_duration_since(*instant) else {
                break;
            };
            if age > Duration::from_secs_f64(window_secs) {
                self.window.pop_front();
            } else {
                break;
            }
        }

        while self.window.len() > self.max_samples {
            self.window.pop_front();
        }
    }

    pub fn rate_bps(&self) -> f64 {
        if self.window_secs == 0.0 || self.window.len() < 2 {
            return 0.0;
        }

        let Some((latest_instant, latest_bytes)) = self.window.back() else {
            return 0.0;
        };
        let Some((oldest_instant, oldest_bytes)) = self.window.front() else {
            return 0.0;
        };

        let Some(window) = latest_instant.checked_duration_since(*oldest_instant) else {
            return 0.0;
        };
        if window.is_zero() || latest_bytes < oldest_bytes {
            return 0.0;
        }

        let bytes = latest_bytes - oldest_bytes;
        bytes as f64 / window.as_secs_f64()
    }

    pub fn eta_secs(&self, total: u64, done: u64) -> Option<f64> {
        if total == 0 || done >= total {
            return None;
        }

        let rate = self.rate_bps();
        if rate < 1e-3 {
            return None;
        }

        Some((total - done) as f64 / rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Instant {
        Instant::now()
    }

    #[test]
    fn constant_rate() {
        let start = base();
        let mut rate = RateCalculator::new();

        for i in 0..51_u64 {
            rate.record(start + Duration::from_millis(i * 100), i * 100_000);
        }

        let bps = rate.rate_bps();
        assert!((950_000.0..=1_050_000.0).contains(&bps), "rate={bps}");
    }

    #[test]
    fn burst_then_idle() {
        let start = base();
        let mut rate = RateCalculator::new();

        for i in 0..10_u64 {
            rate.record(start + Duration::from_millis(i * 100), (i + 1) * 1_000_000);
        }
        rate.record(start + Duration::from_secs(6), 10_000_000);

        assert!(rate.rate_bps() < 500_000.0, "rate={}", rate.rate_bps());
    }

    #[test]
    fn single_sample() {
        let mut rate = RateCalculator::new();
        rate.record(base(), 123);
        assert_eq!(rate.rate_bps(), 0.0);
    }

    #[test]
    fn zero_rate_eta() {
        let mut rate = RateCalculator::new();
        rate.record(base(), 123);
        assert_eq!(rate.eta_secs(100, 0), None);
    }

    #[test]
    fn done_equals_total() {
        let mut rate = RateCalculator::new();
        rate.record(base(), 123);
        assert_eq!(rate.eta_secs(100, 100), None);
    }

    #[test]
    fn done_greater_total() {
        let mut rate = RateCalculator::new();
        rate.record(base(), 123);
        assert_eq!(rate.eta_secs(100, 150), None);
    }

    #[test]
    fn reversed_samples() {
        let t = base();
        let mut rate = RateCalculator::new();
        rate.record(t, 100);
        rate.record(t, 200);
        assert_eq!(rate.rate_bps(), 0.0);
    }

    #[test]
    fn sample_cap() {
        let t = base();
        let mut rate = RateCalculator::new();

        for i in 0..300_u64 {
            rate.record(t, i);
        }

        assert!(rate.window.len() <= 200, "len={}", rate.window.len());
    }

    #[test]
    fn eviction_by_age() {
        let start = base();
        let mut rate = RateCalculator::new();

        for i in 0..100_u64 {
            rate.record(start + Duration::from_millis(i * 100), i);
        }
        rate.record(start + Duration::from_secs(10), 100);

        assert!((50..=51).contains(&rate.window.len()), "len={}", rate.window.len());
        assert!(
            rate.window.front().is_some_and(|(instant, _)| {
                instant
                    .checked_duration_since(start + Duration::from_secs(5))
                    .is_none_or(|age| age <= Duration::from_millis(100))
            }),
            "front={:?}",
            rate.window.front()
        );
    }
}
