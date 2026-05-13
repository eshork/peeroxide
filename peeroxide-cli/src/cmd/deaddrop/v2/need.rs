//! v3 need-list channel.
//!
//! Spec: see *Need-List Feedback Channel* in `DEADDROP_V2.md (and `docs/src/dd/`)`.
//!
//! Wire format:
//!   `[VERSION][count: u16 LE][count × {start: u32 LE, end: u32 LE}]`
//!
//! Each entry expresses a half-open `[start, end)` range of *data chunk
//! indices in DFS file order*. The receiver expresses missing pieces in
//! these terms; the sender translates them into the data chunks plus the
//! full path of index chunks they require.

#![allow(dead_code)]

use super::build::{BuiltTree, IndexChunk};
use super::tree::{compute_layout, TreeLayout};
use super::wire::{
    NEED_ENTRY_SIZE, NEED_LIST_ENTRY_CAP, NEED_LIST_HEADER_SIZE, NON_ROOT_INDEX_SLOT_CAP, VERSION,
    WireError,
};

/// A `[start, end)` range of data chunk indices in DFS file order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeedEntry {
    pub start: u32,
    pub end: u32,
}

impl NeedEntry {
    pub fn new(start: u32, end: u32) -> Self {
        debug_assert!(start < end, "NeedEntry requires start < end");
        Self { start, end }
    }
}

/// Encode a need-list record. Length is `3 + entries.len() * 8` bytes.
///
/// Returns the raw bytes to publish via `mutable_put` to the receiver's
/// ephemeral need-keypair.
pub fn encode_need_list(entries: &[NeedEntry]) -> Vec<u8> {
    let count = entries.len();
    debug_assert!(
        count <= NEED_LIST_ENTRY_CAP,
        "need-list entry count {} exceeds cap {}",
        count,
        NEED_LIST_ENTRY_CAP
    );
    let mut buf = Vec::with_capacity(NEED_LIST_HEADER_SIZE + count * NEED_ENTRY_SIZE);
    buf.push(VERSION);
    buf.extend_from_slice(&(count as u16).to_le_bytes());
    for entry in entries {
        buf.extend_from_slice(&entry.start.to_le_bytes());
        buf.extend_from_slice(&entry.end.to_le_bytes());
    }
    buf
}

/// Decode a need-list record.
///
/// An empty `bytes` slice is the receiver-done sentinel and decodes as an
/// empty entry list (Ok with empty Vec).
pub fn decode_need_list(bytes: &[u8]) -> Result<Vec<NeedEntry>, WireError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes[0] != VERSION {
        return Err(WireError::BadVersion(bytes[0]));
    }
    if bytes.len() < NEED_LIST_HEADER_SIZE {
        return Err(WireError::Truncated {
            needed: NEED_LIST_HEADER_SIZE,
            got: bytes.len(),
        });
    }
    let declared = u16::from_le_bytes(bytes[1..3].try_into().unwrap());
    let entry_bytes = &bytes[NEED_LIST_HEADER_SIZE..];
    if entry_bytes.len() % NEED_ENTRY_SIZE != 0 {
        return Err(WireError::Truncated {
            needed: NEED_LIST_HEADER_SIZE
                + entry_bytes.len().div_ceil(NEED_ENTRY_SIZE) * NEED_ENTRY_SIZE,
            got: bytes.len(),
        });
    }
    let computed = entry_bytes.len() / NEED_ENTRY_SIZE;
    if computed != declared as usize {
        return Err(WireError::BadCount {
            declared,
            computed,
        });
    }

    let mut entries = Vec::with_capacity(computed);
    for chunk in entry_bytes.chunks_exact(NEED_ENTRY_SIZE) {
        let start = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let end = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        if start >= end {
            return Err(WireError::InvalidEntry { start, end });
        }
        entries.push(NeedEntry { start, end });
    }
    Ok(entries)
}

/// Coalesce a sorted list of missing chunk positions into `[start, end)` ranges.
///
/// Input MUST be sorted ascending and unique. Output is the minimal set of
/// half-open ranges covering the input.
pub fn coalesce_missing_ranges(missing_positions: &[u32]) -> Vec<NeedEntry> {
    if missing_positions.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = missing_positions[0];
    let mut prev = missing_positions[0];
    for &p in &missing_positions[1..] {
        if p == prev + 1 {
            prev = p;
        } else {
            out.push(NeedEntry::new(start, prev + 1));
            start = p;
            prev = p;
        }
    }
    out.push(NeedEntry::new(start, prev + 1));
    out
}

/// Tree-position metadata for a non-root index chunk's *contribution* to a
/// data chunk index range.
///
/// Used by `full_path_chunks_for` to look up which index chunks back which
/// data chunks. Senders precompute this at tree-build time; receivers don't
/// need it.
pub struct ChunkPath<'a> {
    /// All non-root index chunks on the path from root → leaf, in
    /// root-to-leaf order (root itself excluded).
    pub index_chain: Vec<&'a IndexChunk>,
}

/// Compute the set of chunks the sender MUST republish in response to a
/// need-list entry, per the spec's *full-path republish* requirement.
///
/// Returns indices into `tree.data_chunks` and `tree.index_chunks` (NOT
/// the root). Caller fans those out via `mutable_put` / `immutable_put`.
pub struct ResponseChunks {
    pub data_chunk_indices: Vec<usize>,
    pub index_chunk_indices: Vec<usize>,
}

/// Compute the response chunk set for a single need-list entry.
///
/// The full-path republish covers:
///   1. Data chunks `entry.start..entry.end`.
///   2. Every leaf-index chunk that holds any of those data hashes.
///   3. Every ancestor non-root index chunk whose subtree intersects the entry.
pub fn response_chunks_for_entry(tree: &BuiltTree, entry: NeedEntry) -> ResponseChunks {
    let n = tree.data_chunks.len() as u64;
    let start = entry.start as u64;
    let end = (entry.end as u64).min(n);
    if start >= end {
        return ResponseChunks {
            data_chunk_indices: Vec::new(),
            index_chunk_indices: Vec::new(),
        };
    }

    // Data chunks are easy.
    let data_chunk_indices: Vec<usize> = (start as usize..end as usize).collect();

    if tree.layout.depth == 0 {
        // No non-root index chunks exist.
        return ResponseChunks {
            data_chunk_indices,
            index_chunk_indices: Vec::new(),
        };
    }

    // Compute touched chunk ranges per layer, bottom-up.
    let mut touched_at_layer: Vec<(u64, u64)> = Vec::with_capacity(tree.layout.depth as usize);

    // Leaf layer (layer 0): data hashes are packed 31 per chunk in file order,
    // so data position p sits in leaf-index `p / 31`.
    let leaf_lo = start / NON_ROOT_INDEX_SLOT_CAP as u64;
    let leaf_hi_inclusive = (end - 1) / NON_ROOT_INDEX_SLOT_CAP as u64;
    touched_at_layer.push((leaf_lo, leaf_hi_inclusive + 1));

    // Higher layers: each higher chunk holds 31 lower chunks, packed in order.
    for _ in 1..tree.layout.depth {
        let (prev_lo, prev_hi_excl) = *touched_at_layer.last().unwrap();
        let prev_hi_inclusive = prev_hi_excl - 1;
        let lo = prev_lo / NON_ROOT_INDEX_SLOT_CAP as u64;
        let hi_inclusive = prev_hi_inclusive / NON_ROOT_INDEX_SLOT_CAP as u64;
        touched_at_layer.push((lo, hi_inclusive + 1));
    }

    // Translate (layer, position_in_layer) → IndexChunk slice index.
    // `tree.index_chunks` is in bottom-up build order: all of layer 0,
    // then all of layer 1, etc.
    let mut layer_offset: Vec<u64> = Vec::with_capacity(tree.layout.depth as usize + 1);
    let mut acc = 0u64;
    layer_offset.push(0);
    for &count in &tree.layout.layer_counts {
        acc += count;
        layer_offset.push(acc);
    }

    let mut index_chunk_indices: Vec<usize> = Vec::new();
    for (layer, &(lo, hi)) in touched_at_layer.iter().enumerate() {
        let layer_chunks = tree.layout.layer_counts[layer];
        let lo = lo.min(layer_chunks);
        let hi = hi.min(layer_chunks);
        for pos in lo..hi {
            let abs_index = (layer_offset[layer] + pos) as usize;
            index_chunk_indices.push(abs_index);
        }
    }

    ResponseChunks {
        data_chunk_indices,
        index_chunk_indices,
    }
}

/// Compute response chunks for an entire need-list record.
///
/// Deduplicates so that overlapping ranges don't produce duplicate
/// republishes. Output indices are sorted ascending.
pub fn response_chunks_for_list(tree: &BuiltTree, entries: &[NeedEntry]) -> ResponseChunks {
    use std::collections::BTreeSet;
    let mut data_set: BTreeSet<usize> = BTreeSet::new();
    let mut index_set: BTreeSet<usize> = BTreeSet::new();

    for &entry in entries {
        let r = response_chunks_for_entry(tree, entry);
        data_set.extend(r.data_chunk_indices);
        index_set.extend(r.index_chunk_indices);
    }

    ResponseChunks {
        data_chunk_indices: data_set.into_iter().collect(),
        index_chunk_indices: index_set.into_iter().collect(),
    }
}

/// Helper: data-chunk-index range that a tree of given layout covers.
pub fn full_data_range(layout: &TreeLayout) -> NeedEntry {
    NeedEntry {
        start: 0,
        end: layout.data_chunk_count as u32,
    }
}

/// Convenience: build a need-list covering all chunks (for "send me everything"
/// initial requests if a receiver wants to bootstrap fully). Not normally used.
pub fn full_need_list(file_size: u64) -> Vec<NeedEntry> {
    let layout = compute_layout(file_size);
    if layout.data_chunk_count == 0 {
        Vec::new()
    } else {
        vec![NeedEntry {
            start: 0,
            end: layout.data_chunk_count as u32,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::deaddrop::v2::build::build_tree_from_bytes;
    use crate::cmd::deaddrop::v2::wire::{DATA_PAYLOAD_MAX, NEED_LIST_ENTRY_CAP};

    #[test]
    fn need_list_roundtrip_empty() {
        let encoded = encode_need_list(&[]);
        assert_eq!(encoded.len(), NEED_LIST_HEADER_SIZE);
        let decoded = decode_need_list(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn need_list_roundtrip_single_entry() {
        let entries = vec![NeedEntry::new(10, 20)];
        let encoded = encode_need_list(&entries);
        assert_eq!(encoded.len(), NEED_LIST_HEADER_SIZE + 8);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn need_list_roundtrip_many_entries() {
        let entries: Vec<NeedEntry> = (0..50)
            .map(|i| NeedEntry::new(i * 100, i * 100 + 50))
            .collect();
        let encoded = encode_need_list(&entries);
        assert_eq!(encoded.len(), NEED_LIST_HEADER_SIZE + 50 * 8);
        assert_eq!(decode_need_list(&encoded).unwrap(), entries);
    }

    #[test]
    fn need_list_at_capacity() {
        let entries: Vec<NeedEntry> = (0..NEED_LIST_ENTRY_CAP as u32)
            .map(|i| NeedEntry::new(i, i + 1))
            .collect();
        let encoded = encode_need_list(&entries);
        assert!(encoded.len() <= 1000);
        assert_eq!(decode_need_list(&encoded).unwrap(), entries);
    }

    #[test]
    fn empty_bytes_is_done_sentinel() {
        let decoded = decode_need_list(&[]).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn rejects_bad_version() {
        let bad = vec![0x01, 0x00, 0x00];
        assert_eq!(decode_need_list(&bad), Err(WireError::BadVersion(0x01)));
    }

    #[test]
    fn rejects_count_mismatch() {
        let mut bytes = vec![VERSION, 0x05, 0x00]; // says 5 entries
        bytes.extend_from_slice(&[0u8; 8]); // but only 1
        assert!(matches!(
            decode_need_list(&bytes),
            Err(WireError::BadCount { declared: 5, computed: 1 })
        ));
    }

    #[test]
    fn rejects_invalid_entry() {
        let entries = [
            5u32.to_le_bytes(),
            5u32.to_le_bytes(), // start == end
        ];
        let mut bytes = vec![VERSION, 0x01, 0x00];
        bytes.extend_from_slice(&entries[0]);
        bytes.extend_from_slice(&entries[1]);
        assert!(matches!(
            decode_need_list(&bytes),
            Err(WireError::InvalidEntry { start: 5, end: 5 })
        ));
    }

    #[test]
    fn coalesce_empty() {
        assert!(coalesce_missing_ranges(&[]).is_empty());
    }

    #[test]
    fn coalesce_single() {
        assert_eq!(coalesce_missing_ranges(&[5]), vec![NeedEntry::new(5, 6)]);
    }

    #[test]
    fn coalesce_contiguous() {
        assert_eq!(
            coalesce_missing_ranges(&[1, 2, 3, 4, 5]),
            vec![NeedEntry::new(1, 6)]
        );
    }

    #[test]
    fn coalesce_gaps() {
        assert_eq!(
            coalesce_missing_ranges(&[1, 2, 5, 6, 7, 10]),
            vec![
                NeedEntry::new(1, 3),
                NeedEntry::new(5, 8),
                NeedEntry::new(10, 11),
            ]
        );
    }

    fn make_data(n_chunks: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..n_chunks {
            data.extend(std::iter::repeat_n((i % 256) as u8, DATA_PAYLOAD_MAX));
        }
        data
    }

    #[test]
    fn response_chunks_depth_0_no_index() {
        let seed = [1u8; 32];
        let data = make_data(20);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        let r = response_chunks_for_entry(&tree, NeedEntry::new(5, 10));
        assert_eq!(r.data_chunk_indices, vec![5, 6, 7, 8, 9]);
        assert!(r.index_chunk_indices.is_empty());
    }

    #[test]
    fn response_chunks_depth_1_one_leaf() {
        let seed = [2u8; 32];
        let data = make_data(70);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        // Need data chunks 5..10 — all in leaf 0 (data 0..30).
        let r = response_chunks_for_entry(&tree, NeedEntry::new(5, 10));
        assert_eq!(r.data_chunk_indices, vec![5, 6, 7, 8, 9]);
        // Should include exactly leaf 0 (index_chunks[0]).
        assert_eq!(r.index_chunk_indices, vec![0]);
    }

    #[test]
    fn response_chunks_depth_1_spans_leaves() {
        let seed = [3u8; 32];
        let data = make_data(70);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        // Need data 25..40 — spans leaf 0 (data 0..31) and leaf 1 (data 31..62).
        let r = response_chunks_for_entry(&tree, NeedEntry::new(25, 40));
        assert_eq!(r.data_chunk_indices, (25..40).collect::<Vec<usize>>());
        assert_eq!(r.index_chunk_indices, vec![0, 1]);
    }

    #[test]
    fn response_chunks_depth_2_full_path() {
        let seed = [4u8; 32];
        let data = make_data(931); // depth 2: 31 leaves + 1 L1
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        // Need just data position 0 — should pull leaf 0 + L1 0.
        let r = response_chunks_for_entry(&tree, NeedEntry::new(0, 1));
        assert_eq!(r.data_chunk_indices, vec![0]);
        // Layer offsets: leaves at 0..31, L1 at 31..32.
        // Leaf 0 → index_chunks[0]; L1 0 → index_chunks[31].
        assert_eq!(r.index_chunk_indices, vec![0, 31]);
    }

    #[test]
    fn response_chunks_dedup_across_entries() {
        let seed = [5u8; 32];
        let data = make_data(70);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        // Two entries that both touch leaf 0.
        let entries = vec![NeedEntry::new(0, 5), NeedEntry::new(10, 15)];
        let r = response_chunks_for_list(&tree, &entries);
        // Leaf 0 should only appear once.
        assert_eq!(r.index_chunk_indices, vec![0]);
        let mut expected_data: Vec<usize> = (0..5).chain(10..15).collect();
        expected_data.sort();
        assert_eq!(r.data_chunk_indices, expected_data);
    }

    #[test]
    fn response_clamps_to_file_size() {
        let seed = [6u8; 32];
        let data = make_data(20);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        // Request bytes past the end.
        let r = response_chunks_for_entry(&tree, NeedEntry::new(15, 100));
        assert_eq!(r.data_chunk_indices, (15..20).collect::<Vec<usize>>());
    }
}
