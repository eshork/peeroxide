//! v3 tree-shape rules.
//!
//! The shape of the index tree is fully determined by `file_size`. Both
//! senders and receivers compute it deterministically via `canonical_depth`.
//! The wire format encodes neither N (data chunk count) nor tree depth
//! directly; both derive from `file_size`.
//!
//! Slot kind (data hash vs child index pubkey) is determined by a chunk's
//! `remaining_depth` in the tree, which the receiver tracks during BFS:
//!   - remaining_depth == 0 → leaf (slots are data hashes)
//!   - remaining_depth >  0 → non-leaf (slots are child index pubkeys)

#![allow(dead_code)]

use super::wire::{DATA_PAYLOAD_MAX, NON_ROOT_INDEX_SLOT_CAP, ROOT_INDEX_SLOT_CAP};

/// Compute the total number of data chunks for a given file size.
///
/// `0 → 0`. Otherwise `ceil(file_size / DATA_PAYLOAD_MAX)`.
pub fn data_chunk_count(file_size: u64) -> u64 {
    if file_size == 0 {
        0
    } else {
        file_size.div_ceil(DATA_PAYLOAD_MAX as u64)
    }
}

/// The canonical tree depth for `n` data chunks.
///
/// Depth is the number of index layers below the root before reaching the
/// leaf-index level (or before reaching data, if N ≤ 30 and root holds
/// data hashes directly).
///
/// Examples:
///   n = 0   → depth 0 (root holds zero slots)
///   n ≤ 30  → depth 0 (root holds data hashes directly)
///   n ≤ 930 → depth 1 (root → leaf-index → data)
///   n ≤ 28,830 → depth 2
///   ...
pub fn canonical_depth(n: u64) -> u32 {
    if n == 0 || n <= ROOT_INDEX_SLOT_CAP as u64 {
        return 0;
    }
    // n > 30: at least one leaf-index layer.
    let mut layer_count = div_ceil_u64(n, NON_ROOT_INDEX_SLOT_CAP as u64);
    let mut depth = 1u32;
    while layer_count > ROOT_INDEX_SLOT_CAP as u64 {
        layer_count = div_ceil_u64(layer_count, NON_ROOT_INDEX_SLOT_CAP as u64);
        depth += 1;
    }
    depth
}

fn div_ceil_u64(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

/// Maximum number of data chunks that fit in a tree of the given depth.
///
/// `depth = 0 → 30` (root direct).
/// `depth = d → 30 × 31^d`.
pub fn max_data_chunks_for_depth(depth: u32) -> u64 {
    let mut cap = ROOT_INDEX_SLOT_CAP as u64;
    for _ in 0..depth {
        cap = cap.saturating_mul(NON_ROOT_INDEX_SLOT_CAP as u64);
    }
    cap
}

/// Layout description of a fully-built canonical tree.
///
/// `layer_chunk_counts[0]` is the leaf-index layer (or data direct if depth 0).
/// Higher indices are further from the data, ending with the count of
/// children directly under the root (or N data chunks if depth 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLayout {
    /// Total data chunks (`N`).
    pub data_chunk_count: u64,
    /// Tree depth (number of index layers below root).
    pub depth: u32,
    /// Per-layer chunk counts, indexed from leaf-index (`0`) upward.
    /// For depth 0, this is empty (root contains data hashes directly).
    /// For depth d ≥ 1, length is d. The last element is the root-children count.
    pub layer_counts: Vec<u64>,
}

/// Compute the canonical layout for a file of size `file_size`.
pub fn compute_layout(file_size: u64) -> TreeLayout {
    let n = data_chunk_count(file_size);
    let depth = canonical_depth(n);

    let mut layer_counts = Vec::with_capacity(depth as usize);
    if depth >= 1 {
        // Leaf-index layer: ceil(N / 31)
        let mut count = div_ceil_u64(n, NON_ROOT_INDEX_SLOT_CAP as u64);
        layer_counts.push(count);
        // Each higher layer
        for _ in 1..depth {
            count = div_ceil_u64(count, NON_ROOT_INDEX_SLOT_CAP as u64);
            layer_counts.push(count);
        }
    }

    TreeLayout {
        data_chunk_count: n,
        depth,
        layer_counts,
    }
}

/// Total non-root index chunk count for the canonical tree of a given file size.
pub fn total_non_root_index_chunks(file_size: u64) -> u64 {
    compute_layout(file_size).layer_counts.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_chunk_count_basic() {
        assert_eq!(data_chunk_count(0), 0);
        assert_eq!(data_chunk_count(1), 1);
        assert_eq!(data_chunk_count(998), 1);
        assert_eq!(data_chunk_count(999), 2);
        assert_eq!(data_chunk_count(1996), 2);
        assert_eq!(data_chunk_count(1997), 3);
    }

    #[test]
    fn canonical_depth_boundaries() {
        assert_eq!(canonical_depth(0), 0);
        assert_eq!(canonical_depth(1), 0);
        assert_eq!(canonical_depth(29), 0);
        assert_eq!(canonical_depth(30), 0);
        assert_eq!(canonical_depth(31), 1);
        assert_eq!(canonical_depth(930), 1); // 30 * 31
        assert_eq!(canonical_depth(931), 2);
        assert_eq!(canonical_depth(28_830), 2); // 30 * 31^2
        assert_eq!(canonical_depth(28_831), 3);
        assert_eq!(canonical_depth(893_730), 3); // 30 * 31^3
        assert_eq!(canonical_depth(893_731), 4);
        assert_eq!(canonical_depth(27_705_630), 4); // 30 * 31^4
        assert_eq!(canonical_depth(27_705_631), 5);
    }

    #[test]
    fn max_data_chunks_matches_spec() {
        assert_eq!(max_data_chunks_for_depth(0), 30);
        assert_eq!(max_data_chunks_for_depth(1), 930);
        assert_eq!(max_data_chunks_for_depth(2), 28_830);
        assert_eq!(max_data_chunks_for_depth(3), 893_730);
        assert_eq!(max_data_chunks_for_depth(4), 27_705_630);
        assert_eq!(max_data_chunks_for_depth(5), 858_874_530);
        assert_eq!(max_data_chunks_for_depth(6), 26_625_110_430);
    }

    #[test]
    fn layout_empty_file() {
        let layout = compute_layout(0);
        assert_eq!(layout.data_chunk_count, 0);
        assert_eq!(layout.depth, 0);
        assert!(layout.layer_counts.is_empty());
    }

    #[test]
    fn layout_small_file_n_eq_1() {
        let layout = compute_layout(100);
        assert_eq!(layout.data_chunk_count, 1);
        assert_eq!(layout.depth, 0);
        assert!(layout.layer_counts.is_empty());
    }

    #[test]
    fn layout_n_eq_30() {
        let layout = compute_layout(30 * DATA_PAYLOAD_MAX as u64);
        assert_eq!(layout.data_chunk_count, 30);
        assert_eq!(layout.depth, 0);
        assert!(layout.layer_counts.is_empty());
    }

    #[test]
    fn layout_n_eq_31() {
        let layout = compute_layout(31 * DATA_PAYLOAD_MAX as u64);
        assert_eq!(layout.data_chunk_count, 31);
        assert_eq!(layout.depth, 1);
        // 31 data → 1 leaf-index node → 1 root child.
        assert_eq!(layout.layer_counts, vec![1]);
    }

    #[test]
    fn layout_n_eq_70() {
        let layout = compute_layout(70 * DATA_PAYLOAD_MAX as u64);
        assert_eq!(layout.data_chunk_count, 70);
        assert_eq!(layout.depth, 1);
        // 70 data → 3 leaf-index nodes (31 + 31 + 8) → 3 root children.
        assert_eq!(layout.layer_counts, vec![3]);
    }

    #[test]
    fn layout_n_eq_930() {
        // 930 = 30 * 31, exactly fills depth 1.
        let layout = compute_layout(930 * DATA_PAYLOAD_MAX as u64);
        assert_eq!(layout.data_chunk_count, 930);
        assert_eq!(layout.depth, 1);
        // 930 data → 30 leaf-index → 30 root children.
        assert_eq!(layout.layer_counts, vec![30]);
    }

    #[test]
    fn layout_n_eq_931_triggers_depth_2() {
        let layout = compute_layout(931 * DATA_PAYLOAD_MAX as u64);
        assert_eq!(layout.data_chunk_count, 931);
        assert_eq!(layout.depth, 2);
        // 931 data → ceil(931/31) = 31 leaves → ceil(31/31) = 1 L1 node → 1 root child.
        assert_eq!(layout.layer_counts, vec![31, 1]);
    }

    #[test]
    fn layout_1gb() {
        let layout = compute_layout(1_073_741_824);
        assert_eq!(layout.data_chunk_count, 1_075_894);
        assert_eq!(layout.depth, 4);
        // 1,075,894 data → 34,707 leaves → 1,120 L1 → 37 L2 → 2 L3 → root with K=2.
        assert_eq!(layout.layer_counts, vec![34_707, 1_120, 37, 2]);
    }

    #[test]
    fn total_non_root_index_chunks_1gb() {
        let total = total_non_root_index_chunks(1_073_741_824);
        assert_eq!(total, 34_707 + 1_120 + 37 + 2);
        assert_eq!(total, 35_866);
    }
}
