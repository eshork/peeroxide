//! v3 streaming-stdout reorder buffer.
//!
//! Spec: see *Output Strategies* in `DEADDROP_V2.md (and `docs/src/dd/`)` (stdout case).
//!
//! The receiver maintains an `emit_pos` cursor indexing the next
//! data-chunk-in-DFS-order it will emit to stdout. Out-of-order arrivals
//! are held in a small reorder buffer keyed by file position. When the
//! awaited position arrives, it is emitted along with any contiguous
//! successors held in the buffer.
//!
//! Buffer size is bounded — at the default `PARALLEL_FETCH_CAP = 64`,
//! at most ~64 KB of in-flight data sits in the reorder buffer.

#![allow(dead_code)]

use std::collections::BTreeMap;

/// In-memory reorder buffer that emits data chunks in DFS file order.
pub struct StreamSink {
    /// Next file-order position to emit.
    emit_pos: u64,
    /// Total expected data chunks (so we know when we're done).
    expected: u64,
    /// Out-of-order chunks waiting for their turn, keyed by file position.
    reorder: BTreeMap<u64, Vec<u8>>,
    /// Bytes emitted so far. Useful for caller bookkeeping.
    emitted_bytes: u64,
}

impl StreamSink {
    pub fn new(expected_data_chunks: u64) -> Self {
        Self {
            emit_pos: 0,
            expected: expected_data_chunks,
            reorder: BTreeMap::new(),
            emitted_bytes: 0,
        }
    }

    /// Accept a data chunk arrival. Returns the sequence of payloads (in
    /// file order) that should be written to stdout right now.
    ///
    /// Calls beyond `expected` are silently ignored; calls with a position
    /// already past `emit_pos` are buffered.
    pub fn accept(&mut self, position: u64, payload: Vec<u8>) -> Vec<Vec<u8>> {
        if position >= self.expected || position < self.emit_pos {
            // Already emitted or out of range — drop.
            return Vec::new();
        }

        let mut out = Vec::new();
        if position == self.emit_pos {
            self.emitted_bytes += payload.len() as u64;
            out.push(payload);
            self.emit_pos += 1;
            // Drain any contiguous successors held in the buffer.
            while let Some(next) = self.reorder.remove(&self.emit_pos) {
                self.emitted_bytes += next.len() as u64;
                out.push(next);
                self.emit_pos += 1;
            }
        } else {
            self.reorder.insert(position, payload);
        }
        out
    }

    /// Have we emitted every expected chunk?
    pub fn is_complete(&self) -> bool {
        self.emit_pos >= self.expected
    }

    /// Position of the next chunk we are waiting on (`expected` if done).
    pub fn next_emit_pos(&self) -> u64 {
        self.emit_pos
    }

    /// Number of chunks held in the reorder buffer.
    pub fn buffered_count(&self) -> usize {
        self.reorder.len()
    }

    /// Total bytes emitted so far.
    pub fn emitted_bytes(&self) -> u64 {
        self.emitted_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u8, len: usize) -> Vec<u8> {
        vec![n; len]
    }

    #[test]
    fn empty_sink_is_complete() {
        let s = StreamSink::new(0);
        assert!(s.is_complete());
    }

    #[test]
    fn in_order_emits_immediately() {
        let mut s = StreamSink::new(3);
        let out = s.accept(0, p(1, 10));
        assert_eq!(out, vec![p(1, 10)]);
        let out = s.accept(1, p(2, 20));
        assert_eq!(out, vec![p(2, 20)]);
        let out = s.accept(2, p(3, 30));
        assert_eq!(out, vec![p(3, 30)]);
        assert!(s.is_complete());
        assert_eq!(s.emitted_bytes(), 60);
    }

    #[test]
    fn out_of_order_waits_then_drains() {
        let mut s = StreamSink::new(3);
        // Position 2 arrives first — buffer it.
        let out = s.accept(2, p(3, 30));
        assert!(out.is_empty());
        assert_eq!(s.buffered_count(), 1);

        // Position 1 arrives — buffer it (still waiting on 0).
        let out = s.accept(1, p(2, 20));
        assert!(out.is_empty());
        assert_eq!(s.buffered_count(), 2);

        // Position 0 arrives — drains everything in order.
        let out = s.accept(0, p(1, 10));
        assert_eq!(out, vec![p(1, 10), p(2, 20), p(3, 30)]);
        assert!(s.is_complete());
        assert_eq!(s.emitted_bytes(), 60);
    }

    #[test]
    fn reverse_order_full_drain() {
        let mut s = StreamSink::new(5);
        for pos in (1..5).rev() {
            assert!(s.accept(pos, p(pos as u8, 10)).is_empty());
        }
        assert_eq!(s.buffered_count(), 4);
        let out = s.accept(0, p(0, 10));
        assert_eq!(out.len(), 5);
        assert!(s.is_complete());
    }

    #[test]
    fn duplicate_position_dropped() {
        let mut s = StreamSink::new(2);
        let out = s.accept(0, p(1, 10));
        assert_eq!(out, vec![p(1, 10)]);
        // Replay position 0 (e.g. a duplicate fetch result) — ignored.
        let out2 = s.accept(0, p(99, 10));
        assert!(out2.is_empty());
    }

    #[test]
    fn position_past_expected_dropped() {
        let mut s = StreamSink::new(2);
        let out = s.accept(5, p(1, 10));
        assert!(out.is_empty());
        assert_eq!(s.buffered_count(), 0);
    }
}
