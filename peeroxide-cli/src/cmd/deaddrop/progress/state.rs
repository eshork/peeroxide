#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Put,
    Get,
}

pub struct ProgressState {
    pub phase: Phase,
    pub version: u8,
    pub filename: Arc<str>,
    pub bytes_total: AtomicU64,
    pub bytes_done: AtomicU64,
    pub indexes_total: AtomicU32,
    pub indexes_done: AtomicU32,
    pub data_total: AtomicU32,
    pub data_done: AtomicU32,
    pub start_instant: Instant,
}

impl ProgressState {
    pub fn new(phase: Phase, version: u8, filename: Arc<str>) -> Arc<Self> {
        Arc::new(Self {
            phase,
            version,
            filename,
            bytes_total: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            indexes_total: AtomicU32::new(0),
            indexes_done: AtomicU32::new(0),
            data_total: AtomicU32::new(0),
            data_done: AtomicU32::new(0),
            start_instant: Instant::now(),
        })
    }

    pub fn set_length(&self, bytes_total: u64, indexes_total: u32, data_total: u32) {
        self.bytes_total.store(bytes_total, Ordering::Relaxed);
        self.indexes_total.store(indexes_total, Ordering::Relaxed);
        self.data_total.store(data_total, Ordering::Relaxed);
    }

    pub fn inc_index(&self) {
        self.indexes_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_data(&self, chunk_bytes: u64) {
        self.data_done.fetch_add(1, Ordering::Relaxed);
        self.bytes_done.fetch_add(chunk_bytes, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_serde() {
        assert_eq!(serde_json::to_string(&Phase::Put).unwrap(), "\"put\"");
        assert_eq!(serde_json::to_string(&Phase::Get).unwrap(), "\"get\"");
    }

    #[test]
    fn set_length_after_start() {
        let state = ProgressState::new(Phase::Put, 2, Arc::<str>::from("file.txt"));
        assert_eq!(state.bytes_total.load(Ordering::Relaxed), 0);
        state.set_length(1000, 3, 5);
        assert_eq!(state.bytes_total.load(Ordering::Relaxed), 1000);
        assert_eq!(state.indexes_total.load(Ordering::Relaxed), 3);
        assert_eq!(state.data_total.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn no_panic_on_zero_bytes() {
        let state = ProgressState::new(Phase::Get, 2, Arc::<str>::from("file.txt"));
        state.inc_data(0);
        assert_eq!(state.data_done.load(Ordering::Relaxed), 1);
        assert_eq!(state.bytes_done.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_inc() {
        let state = ProgressState::new(Phase::Put, 2, Arc::<str>::from("file.txt"));
        let mut tasks = Vec::with_capacity(64);
        for _ in 0..64 {
            let state = Arc::clone(&state);
            tasks.push(tokio::spawn(async move {
                state.inc_data(65536);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(state.data_done.load(Ordering::Relaxed), 64);
        assert_eq!(state.bytes_done.load(Ordering::Relaxed), 64 * 65536);
    }
}
