//! v3 sender-side tree construction.
//!
//! Bottom-up greedy. Spec: see *Tree Shape (normative)* section of
//! `DEADDROP_V3.md`. The construction is fully determined by `file_size`;
//! senders MUST produce exactly this shape.

#![allow(dead_code)]

use peeroxide::KeyPair;

use super::keys::{data_chunk_address, derive_index_keypair, salt as compute_salt};
use super::tree::{compute_layout, TreeLayout};
use super::wire::{
    encode_data_chunk, encode_non_root_index, encode_root_index, DATA_PAYLOAD_MAX, HASH_LEN,
    NON_ROOT_INDEX_SLOT_CAP,
};

/// A single index chunk that the sender will publish via `mutable_put`.
#[derive(Clone)]
pub struct IndexChunk {
    /// Sender-assigned linear index in the keypair derivation scheme.
    pub keypair_index: u32,
    /// The keypair used to sign this chunk (`derive_index_keypair(seed, keypair_index)`).
    pub keypair: KeyPair,
    /// Encoded chunk bytes.
    pub encoded: Vec<u8>,
    /// Tree-position metadata: `0` = leaf-index level, higher values = closer to root.
    pub layer: u32,
    /// Position within `layer` (0-indexed; layout traversal order matches build order).
    pub position_in_layer: u64,
}

/// A single data chunk that the sender will publish via `immutable_put`.
#[derive(Clone)]
pub struct DataChunk {
    /// File-order position (0-indexed).
    pub file_position: u64,
    /// Content address: `discovery_key(encoded)`.
    pub address: [u8; HASH_LEN],
    /// Encoded chunk bytes (`[VERSION][salt][payload]`).
    pub encoded: Vec<u8>,
}

/// The fully built v3 tree, ready to publish.
pub struct BuiltTree {
    /// Encoded root chunk bytes.
    pub root_encoded: Vec<u8>,
    /// Root keypair (derived from `root_seed` directly).
    pub root_keypair: KeyPair,
    /// Non-root index chunks, in bottom-up build order matching their `keypair_index`.
    pub index_chunks: Vec<IndexChunk>,
    /// Data chunks in file order.
    pub data_chunks: Vec<DataChunk>,
    /// Layout metadata.
    pub layout: TreeLayout,
    /// CRC-32C of the reassembled file payload.
    pub crc32c: u32,
}

/// Errors that can arise while building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    DataCountMismatch { expected: u64, got: u64 },
    EmptyChunkInNonEmptyFile,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::DataCountMismatch { expected, got } => write!(
                f,
                "data chunk count mismatch: expected {expected}, got {got}"
            ),
            BuildError::EmptyChunkInNonEmptyFile => {
                write!(f, "received an empty data chunk for a non-empty file")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Build the v3 tree for a file.
///
/// `data_payloads` is an iterator over the file's data-chunk payloads in
/// file order. Each payload must be ≤ `DATA_PAYLOAD_MAX` bytes and (apart
/// from the last) exactly that size; the iterator must yield `data_chunk_count(file_size)`
/// items.
///
/// `crc32c` is the CRC-32C of the entire reassembled file payload.
pub fn build_tree<I>(
    root_seed: &[u8; 32],
    file_size: u64,
    crc32c: u32,
    data_payloads: I,
) -> Result<BuiltTree, BuildError>
where
    I: IntoIterator,
    I::Item: AsRef<[u8]>,
{
    let salt = compute_salt(root_seed);
    let root_keypair = KeyPair::from_seed(*root_seed);
    let layout = compute_layout(file_size);
    let n = layout.data_chunk_count;

    // Encode all data chunks.
    let mut data_chunks: Vec<DataChunk> = Vec::with_capacity(n as usize);
    for (i, payload) in data_payloads.into_iter().enumerate() {
        let payload = payload.as_ref();
        debug_assert!(
            payload.len() <= DATA_PAYLOAD_MAX,
            "data payload {} exceeds DATA_PAYLOAD_MAX",
            payload.len()
        );
        let encoded = encode_data_chunk(salt, payload);
        let address = data_chunk_address(&encoded);
        data_chunks.push(DataChunk {
            file_position: i as u64,
            address,
            encoded,
        });
    }
    if data_chunks.len() as u64 != n {
        return Err(BuildError::DataCountMismatch {
            expected: n,
            got: data_chunks.len() as u64,
        });
    }

    // Special case: empty file. Root has zero slots, no non-root index chunks.
    if n == 0 {
        let root_encoded = encode_root_index(file_size, crc32c, &[]);
        return Ok(BuiltTree {
            root_encoded,
            root_keypair,
            index_chunks: Vec::new(),
            data_chunks,
            layout,
            crc32c,
        });
    }

    // Special case: N ≤ 30. Root holds data hashes directly; no non-root chunks.
    if layout.depth == 0 {
        let slots: Vec<[u8; HASH_LEN]> = data_chunks.iter().map(|d| d.address).collect();
        let root_encoded = encode_root_index(file_size, crc32c, &slots);
        return Ok(BuiltTree {
            root_encoded,
            root_keypair,
            index_chunks: Vec::new(),
            data_chunks,
            layout,
            crc32c,
        });
    }

    // General case: bottom-up greedy.
    //
    // Layer 0 is leaf-index (each holds up to 31 data hashes from `data_chunks`).
    // Layer L > 0 holds up to 31 child pubkeys from layer L-1.
    // The top layer (`depth - 1`) becomes the root's children.
    let mut index_chunks: Vec<IndexChunk> = Vec::new();
    let mut next_keypair_index: u32 = 0;

    // Build leaf-index layer (layer 0).
    let leaf_count = layout.layer_counts[0];
    let mut leaf_pubkeys: Vec<[u8; HASH_LEN]> = Vec::with_capacity(leaf_count as usize);

    for leaf_pos in 0..leaf_count {
        let start = (leaf_pos * NON_ROOT_INDEX_SLOT_CAP as u64) as usize;
        let end = ((leaf_pos + 1) * NON_ROOT_INDEX_SLOT_CAP as u64).min(n) as usize;
        let slots: Vec<[u8; HASH_LEN]> =
            data_chunks[start..end].iter().map(|d| d.address).collect();
        let encoded = encode_non_root_index(&slots);
        let kp = derive_index_keypair(root_seed, next_keypair_index);
        leaf_pubkeys.push(kp.public_key);
        index_chunks.push(IndexChunk {
            keypair_index: next_keypair_index,
            keypair: kp,
            encoded,
            layer: 0,
            position_in_layer: leaf_pos,
        });
        next_keypair_index += 1;
    }

    // Build higher layers.
    let mut child_pubkeys = leaf_pubkeys;
    for layer_idx in 1..layout.depth {
        let layer_chunk_count = layout.layer_counts[layer_idx as usize];
        let mut layer_pubkeys: Vec<[u8; HASH_LEN]> = Vec::with_capacity(layer_chunk_count as usize);
        let prev_count = child_pubkeys.len();

        for chunk_pos in 0..layer_chunk_count {
            let start = (chunk_pos * NON_ROOT_INDEX_SLOT_CAP as u64) as usize;
            let end = ((chunk_pos + 1) * NON_ROOT_INDEX_SLOT_CAP as u64)
                .min(prev_count as u64) as usize;
            let slots: Vec<[u8; HASH_LEN]> = child_pubkeys[start..end].to_vec();
            let encoded = encode_non_root_index(&slots);
            let kp = derive_index_keypair(root_seed, next_keypair_index);
            layer_pubkeys.push(kp.public_key);
            index_chunks.push(IndexChunk {
                keypair_index: next_keypair_index,
                keypair: kp,
                encoded,
                layer: layer_idx,
                position_in_layer: chunk_pos,
            });
            next_keypair_index += 1;
        }

        child_pubkeys = layer_pubkeys;
    }

    // Root holds the top layer's pubkeys.
    let root_encoded = encode_root_index(file_size, crc32c, &child_pubkeys);

    Ok(BuiltTree {
        root_encoded,
        root_keypair,
        index_chunks,
        data_chunks,
        layout,
        crc32c,
    })
}

/// Convenience wrapper: split an in-memory byte slice into payloads and
/// build the tree in one shot. CRC32C is computed over the whole file.
pub fn build_tree_from_bytes(root_seed: &[u8; 32], file: &[u8]) -> Result<BuiltTree, BuildError> {
    let crc = crc32c::crc32c(file);
    let payloads = file.chunks(DATA_PAYLOAD_MAX);
    build_tree(root_seed, file.len() as u64, crc, payloads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::deaddrop::v2::tree::canonical_depth;
    use crate::cmd::deaddrop::v2::wire::{decode_root_index, ROOT_INDEX_SLOT_CAP};

    fn make_data(n_chunks: usize, last_partial: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..n_chunks {
            let len = if i + 1 == n_chunks && last_partial > 0 {
                last_partial
            } else {
                DATA_PAYLOAD_MAX
            };
            data.extend(std::iter::repeat_n((i % 256) as u8, len));
        }
        data
    }

    #[test]
    fn build_empty_file() {
        let seed = [0u8; 32];
        let tree = build_tree_from_bytes(&seed, &[]).unwrap();
        assert!(tree.data_chunks.is_empty());
        assert!(tree.index_chunks.is_empty());
        let decoded = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded.file_size, 0);
        assert_eq!(decoded.crc32c, 0);
        assert!(decoded.slots.is_empty());
    }

    #[test]
    fn build_tiny_file_n_1() {
        let seed = [1u8; 32];
        let data = make_data(1, 100);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        assert_eq!(tree.data_chunks.len(), 1);
        assert!(tree.index_chunks.is_empty());
        let decoded = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded.slots.len(), 1);
        assert_eq!(decoded.slots[0], tree.data_chunks[0].address);
    }

    #[test]
    fn build_n_eq_30() {
        let seed = [2u8; 32];
        let data = make_data(30, 0);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        assert_eq!(tree.data_chunks.len(), 30);
        assert!(tree.index_chunks.is_empty());
        assert_eq!(tree.layout.depth, 0);
        let decoded = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded.slots.len(), 30);
        for (i, slot) in decoded.slots.iter().enumerate() {
            assert_eq!(*slot, tree.data_chunks[i].address);
        }
    }

    #[test]
    fn build_n_eq_31_depth_1() {
        let seed = [3u8; 32];
        let data = make_data(31, 0);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        assert_eq!(tree.data_chunks.len(), 31);
        assert_eq!(tree.layout.depth, 1);
        // 1 leaf-index chunk holding 31 data hashes; root has 1 child slot.
        assert_eq!(tree.index_chunks.len(), 1);
        let leaf = &tree.index_chunks[0];
        assert_eq!(leaf.layer, 0);
        assert_eq!(leaf.position_in_layer, 0);

        let decoded_root = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded_root.slots.len(), 1);
        assert_eq!(decoded_root.slots[0], leaf.keypair.public_key);
    }

    #[test]
    fn build_n_eq_70_depth_1_three_leaves() {
        let seed = [4u8; 32];
        let data = make_data(70, 0);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        assert_eq!(tree.layout.depth, 1);
        assert_eq!(tree.index_chunks.len(), 3);
        let decoded = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded.slots.len(), 3);
        for (i, leaf) in tree.index_chunks.iter().enumerate() {
            assert_eq!(leaf.layer, 0);
            assert_eq!(leaf.position_in_layer, i as u64);
            assert_eq!(decoded.slots[i], leaf.keypair.public_key);
        }
    }

    #[test]
    fn build_n_eq_931_triggers_depth_2() {
        let seed = [5u8; 32];
        let data = make_data(931, 100);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        assert_eq!(tree.layout.depth, 2);
        // 931 data → 31 leaves → 1 L1 → root holds 1 child.
        let leaves: Vec<_> = tree.index_chunks.iter().filter(|c| c.layer == 0).collect();
        let l1: Vec<_> = tree.index_chunks.iter().filter(|c| c.layer == 1).collect();
        assert_eq!(leaves.len(), 31);
        assert_eq!(l1.len(), 1);
        let decoded = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded.slots.len(), 1);
        assert_eq!(decoded.slots[0], l1[0].keypair.public_key);
    }

    #[test]
    fn keypair_indices_are_dense_in_build_order() {
        let seed = [7u8; 32];
        let data = make_data(70, 0);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        for (i, chunk) in tree.index_chunks.iter().enumerate() {
            assert_eq!(chunk.keypair_index, i as u32);
        }
    }

    #[test]
    fn rejects_too_few_payloads() {
        let seed = [0u8; 32];
        // Claim file_size of 100 (1 chunk) but pass no payloads.
        let result = build_tree(&seed, 100, 0, std::iter::empty::<&[u8]>());
        assert!(matches!(result, Err(BuildError::DataCountMismatch { .. })));
    }

    #[test]
    fn data_chunks_in_file_order() {
        let seed = [9u8; 32];
        let data = make_data(70, 0);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        for (i, dc) in tree.data_chunks.iter().enumerate() {
            assert_eq!(dc.file_position, i as u64);
        }
    }

    #[test]
    fn root_carries_correct_file_size_and_crc() {
        let seed = [11u8; 32];
        let data = b"some content";
        let tree = build_tree_from_bytes(&seed, data).unwrap();
        let decoded = decode_root_index(&tree.root_encoded).unwrap();
        assert_eq!(decoded.file_size, data.len() as u64);
        assert_eq!(decoded.crc32c, crc32c::crc32c(data));
    }

    #[test]
    fn salt_propagates_to_data_chunks() {
        let mut seed = [0u8; 32];
        seed[0] = 0xAA;
        let data = make_data(2, 100);
        let tree = build_tree_from_bytes(&seed, &data).unwrap();
        for dc in &tree.data_chunks {
            assert_eq!(dc.encoded[0], 0x02); // version
            assert_eq!(dc.encoded[1], 0x00); // salt (forced to 0)
        }
    }

    #[test]
    fn root_slot_cap_boundary() {
        // Use ROOT_INDEX_SLOT_CAP to make the boundary explicit.
        assert_eq!(ROOT_INDEX_SLOT_CAP, 30);
        // Just past the boundary triggers depth 1.
        assert_eq!(canonical_depth(31), 1);
    }
}
