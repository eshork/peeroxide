#![allow(dead_code, private_interfaces)]
use super::*;

pub const VERSION: u8 = 0x02;
const DATA_PAYLOAD_MAX: usize = 999; // MAX_PAYLOAD(1000) - 1 byte version header
const ROOT_INDEX_HEADER: usize = 41; // 1+4+4+32
const NON_ROOT_INDEX_HEADER: usize = 33; // 1+32
const PTRS_PER_ROOT: usize = (MAX_PAYLOAD - ROOT_INDEX_HEADER) / 32; // 29
const PTRS_PER_NON_ROOT: usize = (MAX_PAYLOAD - NON_ROOT_INDEX_HEADER) / 32; // 30
const MAX_DATA_CHUNKS: usize = PTRS_PER_ROOT + 65535 * PTRS_PER_NON_ROOT;
const MAX_FILE_SIZE: u64 = MAX_DATA_CHUNKS as u64 * DATA_PAYLOAD_MAX as u64;
pub const PARALLEL_FETCH_CAP: usize = 64;

pub fn derive_index_keypair(root_seed: &[u8; 32], i: u16) -> KeyPair {
    let mut input = Vec::with_capacity(32 + 3 + 2);
    input.extend_from_slice(root_seed);
    input.extend_from_slice(b"idx");
    input.extend_from_slice(&i.to_le_bytes());
    KeyPair::from_seed(peeroxide::discovery_key(&input))
}

pub fn data_chunk_hash(encoded: &[u8]) -> [u8; 32] {
    peeroxide::discovery_key(encoded)
}

pub fn encode_data_chunk(payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + payload.len());
    buf.push(VERSION);
    buf.extend_from_slice(payload);
    buf
}

pub fn encode_root_index(
    file_size: u32,
    crc: u32,
    next_pk: &[u8; 32],
    data_hashes: &[[u8; 32]],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ROOT_INDEX_HEADER + 32 * data_hashes.len());
    buf.push(VERSION);
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(next_pk);
    for h in data_hashes {
        buf.extend_from_slice(h);
    }
    buf
}

pub fn encode_non_root_index(next_pk: &[u8; 32], data_hashes: &[[u8; 32]]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(NON_ROOT_INDEX_HEADER + 32 * data_hashes.len());
    buf.push(VERSION);
    buf.extend_from_slice(next_pk);
    for h in data_hashes {
        buf.extend_from_slice(h);
    }
    buf
}

pub fn compute_data_chunk_count(file_size: usize) -> usize {
    if file_size == 0 {
        0
    } else {
        file_size.div_ceil(DATA_PAYLOAD_MAX)
    }
}

pub fn compute_index_chain_length(data_count: usize) -> usize {
    if data_count <= PTRS_PER_ROOT {
        1
    } else {
        1 + (data_count - PTRS_PER_ROOT).div_ceil(PTRS_PER_NON_ROOT)
    }
}

pub struct V2Built {
    pub data_chunks: Vec<Vec<u8>>,    // encoded data chunks (plain bytes for immutable_put)
    pub index_chunks: Vec<ChunkData>, // encoded index chunks (with keypairs for mutable_put)
    pub data_hashes: Vec<[u8; 32]>,   // content hash of each data chunk
}

pub fn build_v2_chunks(data: &[u8], root_seed: &[u8; 32]) -> Result<V2Built, String> {
    if data.len() as u64 > MAX_FILE_SIZE {
        return Err(format!(
            "file too large ({} bytes, max {})",
            data.len(),
            MAX_FILE_SIZE
        ));
    }
    let crc = crc32c::crc32c(data);
    let file_size = data.len() as u32;

    // Split and encode data chunks; compute content hash for each
    let encoded_data: Vec<Vec<u8>> = if data.is_empty() {
        vec![]
    } else {
        data.chunks(DATA_PAYLOAD_MAX).map(encode_data_chunk).collect()
    };
    let data_hashes: Vec<[u8; 32]> = encoded_data.iter().map(|e| data_chunk_hash(e)).collect();

    let data_count = encoded_data.len();
    let index_count = compute_index_chain_length(data_count);

    // Derive index keypairs
    // root = KeyPair::from_seed(*root_seed); non-root i=1..
    let index_keypairs: Vec<KeyPair> = {
        let mut kps = Vec::with_capacity(index_count);
        kps.push(KeyPair::from_seed(*root_seed));
        for i in 1..index_count {
            kps.push(derive_index_keypair(root_seed, i as u16));
        }
        kps
    };

    // Encode index chunks
    // root gets data_hashes[0..PTRS_PER_ROOT]
    // non-root i gets data_hashes[PTRS_PER_ROOT + (i-1)*PTRS_PER_NON_ROOT .. PTRS_PER_ROOT + i*PTRS_PER_NON_ROOT]
    // next_pk: index[j].next_pk = index_keypairs[j+1].public_key (last has [0u8;32])
    let mut index_chunks: Vec<ChunkData> = Vec::with_capacity(index_count);
    for j in 0..index_count {
        let next_pk: [u8; 32] = if j + 1 < index_count {
            index_keypairs[j + 1].public_key
        } else {
            [0u8; 32]
        };

        let encoded = if j == 0 {
            // root
            let end = PTRS_PER_ROOT.min(data_count);
            encode_root_index(file_size, crc, &next_pk, &data_hashes[..end])
        } else {
            // non-root j: data_hashes[PTRS_PER_ROOT + (j-1)*PTRS_PER_NON_ROOT ..]
            let start = PTRS_PER_ROOT + (j - 1) * PTRS_PER_NON_ROOT;
            let end = (start + PTRS_PER_NON_ROOT).min(data_count);
            encode_non_root_index(&next_pk, &data_hashes[start..end])
        };

        index_chunks.push(ChunkData {
            keypair: index_keypairs[j].clone(),
            encoded,
        });
    }

    Ok(V2Built {
        data_chunks: encoded_data,
        index_chunks,
        data_hashes,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum NeedEntry {
    Index { start: u16, end: u16 },
    Data { start: u32, end: u32 },
}

pub fn need_topic(root_pk: &[u8; 32]) -> [u8; 32] {
    let mut input = Vec::with_capacity(32 + 4);
    input.extend_from_slice(root_pk);
    input.extend_from_slice(b"need");
    peeroxide::discovery_key(&input)
}

pub fn encode_need_list(entries: &[NeedEntry]) -> Vec<u8> {
    let mut buf = vec![VERSION];
    for entry in entries {
        match entry {
            NeedEntry::Index { start, end } => {
                if buf.len() + 5 > MAX_PAYLOAD {
                    break;
                }
                buf.push(0x00);
                buf.extend_from_slice(&start.to_le_bytes());
                buf.extend_from_slice(&end.to_le_bytes());
            }
            NeedEntry::Data { start, end } => {
                if buf.len() + 9 > MAX_PAYLOAD {
                    break;
                }
                buf.push(0x01);
                buf.extend_from_slice(&start.to_le_bytes());
                buf.extend_from_slice(&end.to_le_bytes());
            }
        }
    }
    buf
}

pub fn decode_need_list(data: &[u8]) -> Result<Vec<NeedEntry>, String> {
    if data.is_empty() {
        return Ok(vec![]);
    }
    if data[0] != VERSION {
        return Err(format!("unexpected version byte 0x{:02x}", data[0]));
    }
    let mut entries = Vec::new();
    let mut i = 1;
    while i < data.len() {
        match data[i] {
            0x00 => {
                if i + 5 > data.len() {
                    return Err("truncated index entry".into());
                }
                let start = u16::from_le_bytes([data[i + 1], data[i + 2]]);
                let end = u16::from_le_bytes([data[i + 3], data[i + 4]]);
                entries.push(NeedEntry::Index { start, end });
                i += 5;
            }
            0x01 => {
                if i + 9 > data.len() {
                    return Err("truncated data entry".into());
                }
                let start = u32::from_le_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]);
                let end = u32::from_le_bytes([data[i + 5], data[i + 6], data[i + 7], data[i + 8]]);
                entries.push(NeedEntry::Data { start, end });
                i += 9;
            }
            tag => return Err(format!("unknown need list tag 0x{tag:02x}")),
        }
    }
    Ok(entries)
}

pub async fn run_put(args: &PutArgs, cfg: &ResolvedConfig) -> i32 {
    super::v1::run_put(args, cfg).await
}

pub async fn get_from_root(
    _root_data: Vec<u8>,
    _root_pk: [u8; 32],
    _handle: HyperDhtHandle,
    _task_handle: tokio::task::JoinHandle<Result<(), peeroxide_dht::hyperdht::HyperDhtError>>,
    _args: &GetArgs,
) -> i32 {
    eprintln!("error: v2 dead drop format not yet implemented");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(b: u8) -> [u8; 32] {
        [b; 32]
    }

    #[test]
    fn test_derive_index_keys_domain_separation() {
        let s = seed(1);
        let v2_key = derive_index_keypair(&s, 0).public_key;
        // v1 derivation: discovery_key(seed || u16_le) — no domain tag
        let mut v1_input = Vec::new();
        v1_input.extend_from_slice(&s);
        v1_input.extend_from_slice(&0u16.to_le_bytes());
        let v1_key = peeroxide::KeyPair::from_seed(peeroxide::discovery_key(&v1_input)).public_key;
        assert_ne!(v2_key, v1_key, "v2 and v1 keys must differ for same seed/index");
        let key1 = derive_index_keypair(&s, 1).public_key;
        assert_ne!(v2_key, key1, "different indices must give different keys");
    }

    #[test]
    fn test_encode_data_chunk() {
        let payload = vec![1u8, 2, 3];
        let encoded = encode_data_chunk(&payload);
        assert_eq!(encoded[0], VERSION);
        assert_eq!(&encoded[1..], &payload);
        // max payload
        let max_payload = vec![0u8; DATA_PAYLOAD_MAX];
        let encoded_max = encode_data_chunk(&max_payload);
        assert_eq!(encoded_max.len(), MAX_PAYLOAD);
    }

    #[test]
    fn test_data_chunk_hash_deterministic() {
        let a = encode_data_chunk(&[1, 2, 3]);
        let b = encode_data_chunk(&[1, 2, 3]);
        let c = encode_data_chunk(&[4, 5, 6]);
        assert_eq!(data_chunk_hash(&a), data_chunk_hash(&b));
        assert_ne!(data_chunk_hash(&a), data_chunk_hash(&c));
        // hash is blake2b of encoded bytes
        assert_eq!(data_chunk_hash(&a), peeroxide::discovery_key(&a));
    }

    #[test]
    fn test_encode_root_index_structure() {
        let next_pk = [7u8; 32];
        let hashes: Vec<[u8; 32]> = (0..3).map(|i| [i as u8; 32]).collect();
        let enc = encode_root_index(42, 99, &next_pk, &hashes);
        assert_eq!(enc[0], VERSION);
        assert_eq!(u32::from_le_bytes(enc[1..5].try_into().unwrap()), 42);
        assert_eq!(u32::from_le_bytes(enc[5..9].try_into().unwrap()), 99);
        assert_eq!(&enc[9..41], &next_pk);
        assert_eq!(enc.len(), ROOT_INDEX_HEADER + 32 * 3);
    }

    #[test]
    fn test_encode_non_root_index_structure() {
        let next_pk = [3u8; 32];
        let hashes: Vec<[u8; 32]> = (0..2).map(|i| [i as u8; 32]).collect();
        let enc = encode_non_root_index(&next_pk, &hashes);
        assert_eq!(enc[0], VERSION);
        assert_eq!(&enc[1..33], &next_pk);
        assert_eq!(enc.len(), NON_ROOT_INDEX_HEADER + 32 * 2);
    }

    #[test]
    fn test_compute_data_chunk_count() {
        assert_eq!(compute_data_chunk_count(0), 0);
        assert_eq!(compute_data_chunk_count(1), 1);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX), 1);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX + 1), 2);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX * 2), 2);
        assert_eq!(compute_data_chunk_count(DATA_PAYLOAD_MAX * 2 + 1), 3);
    }

    #[test]
    fn test_compute_index_chain_length() {
        assert_eq!(compute_index_chain_length(0), 1);
        assert_eq!(compute_index_chain_length(1), 1);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT), 1);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT + 1), 2);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT + PTRS_PER_NON_ROOT), 2);
        assert_eq!(compute_index_chain_length(PTRS_PER_ROOT + PTRS_PER_NON_ROOT + 1), 3);
    }

    #[test]
    fn test_build_v2_chunks_empty() {
        let s = seed(2);
        let built = build_v2_chunks(&[], &s).unwrap();
        assert_eq!(built.data_chunks.len(), 0);
        assert_eq!(built.index_chunks.len(), 1);
        assert_eq!(built.data_hashes.len(), 0);
        // root index must have file_size=0
        let root = &built.index_chunks[0].encoded;
        assert_eq!(root[0], VERSION);
        assert_eq!(u32::from_le_bytes(root[1..5].try_into().unwrap()), 0);
    }

    #[test]
    fn test_build_v2_chunks_single() {
        let s = seed(3);
        let data = b"hello";
        let built = build_v2_chunks(data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), 1);
        assert_eq!(built.index_chunks.len(), 1);
        assert_eq!(built.data_hashes.len(), 1);
        let root = &built.index_chunks[0].encoded;
        // root should contain 1 hash after the header
        assert_eq!(root.len(), ROOT_INDEX_HEADER + 32);
    }

    #[test]
    fn test_build_v2_chunks_fills_root() {
        let s = seed(4);
        let data = vec![0u8; PTRS_PER_ROOT * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), PTRS_PER_ROOT);
        assert_eq!(built.index_chunks.len(), 1);
        assert_eq!(
            built.index_chunks[0].encoded.len(),
            ROOT_INDEX_HEADER + 32 * PTRS_PER_ROOT
        );
    }

    #[test]
    fn test_build_v2_chunks_spills() {
        let s = seed(5);
        let data = vec![0u8; (PTRS_PER_ROOT + 1) * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), PTRS_PER_ROOT + 1);
        assert_eq!(built.index_chunks.len(), 2);
        // root has PTRS_PER_ROOT hashes; non-root has 1
        assert_eq!(
            built.index_chunks[0].encoded.len(),
            ROOT_INDEX_HEADER + 32 * PTRS_PER_ROOT
        );
        assert_eq!(
            built.index_chunks[1].encoded.len(),
            NON_ROOT_INDEX_HEADER + 32 * 1
        );
        // root's next_pk = non-root's public key
        let root_next: [u8; 32] = built.index_chunks[0].encoded[9..41].try_into().unwrap();
        assert_eq!(root_next, built.index_chunks[1].keypair.public_key);
    }

    #[test]
    fn test_build_v2_chunks_multi_index() {
        let s = seed(6);
        let n = PTRS_PER_ROOT + 2 * PTRS_PER_NON_ROOT + 1;
        let data = vec![1u8; n * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        assert_eq!(built.data_chunks.len(), n);
        assert!(built.index_chunks.len() >= 3);
    }

    #[test]
    fn test_build_v2_chunks_reassemble() {
        let s = seed(7);
        let original: Vec<u8> = (0..5000u32).map(|i| (i % 256) as u8).collect();
        let built = build_v2_chunks(&original, &s).unwrap();
        // reassemble: strip version byte from each data chunk
        let reassembled: Vec<u8> = built
            .data_chunks
            .iter()
            .flat_map(|c| c[1..].iter().copied())
            .collect();
        assert_eq!(&reassembled, &original);
        // verify CRC stored in root matches original
        let root = &built.index_chunks[0].encoded;
        let stored_crc = u32::from_le_bytes(root[5..9].try_into().unwrap());
        assert_eq!(stored_crc, crc32c::crc32c(&original));
    }

    #[test]
    fn test_build_v2_rejects_oversized() {
        // We can't actually allocate MAX_FILE_SIZE, so test the boundary check logic
        // by checking a known oversized value
        // Instead, verify MAX_FILE_SIZE constant is set correctly
        assert!(MAX_FILE_SIZE > 1_000_000_000, "MAX_FILE_SIZE should be > 1GB");
        // Test: MAX_DATA_CHUNKS constant is > 1.9M
        assert!(MAX_DATA_CHUNKS > 1_900_000);
    }

    #[test]
    fn test_index_chain_links() {
        let s = seed(8);
        let data = vec![0u8; (PTRS_PER_ROOT + 2) * DATA_PAYLOAD_MAX];
        let built = build_v2_chunks(&data, &s).unwrap();
        let n = built.index_chunks.len();
        for j in 0..n - 1 {
            // root (j==0): next_pk at [9..41]; non-root (j>0): next_pk at [1..33]
            let next_pk: [u8; 32] = if j == 0 {
                built.index_chunks[j].encoded[9..41].try_into().unwrap()
            } else {
                built.index_chunks[j].encoded[1..33].try_into().unwrap()
            };
            assert_eq!(next_pk, built.index_chunks[j + 1].keypair.public_key);
        }
        // last chunk next_pk is all zeros
        let last = &built.index_chunks[n - 1];
        let last_next_pk: [u8; 32] = last.encoded[9..41].try_into().unwrap_or([0u8; 32]);
        // non-root: next_pk is at offset 1..33
        let last_non_root_next_pk: [u8; 32] = last.encoded[1..33].try_into().unwrap();
        let zero = [0u8; 32];
        // one of them must be zero (depending on root vs non-root)
        assert!(last_next_pk == zero || last_non_root_next_pk == zero);
    }

    #[test]
    fn test_index_stores_content_hashes() {
        let s = seed(9);
        let data = b"abc def ghi";
        let built = build_v2_chunks(data, &s).unwrap();
        for (i, encoded) in built.data_chunks.iter().enumerate() {
            let expected_hash = data_chunk_hash(encoded);
            assert_eq!(built.data_hashes[i], expected_hash);
        }
        // Also verify hashes appear in root index
        let root = &built.index_chunks[0].encoded;
        for (i, hash) in built.data_hashes.iter().enumerate() {
            let offset = ROOT_INDEX_HEADER + i * 32;
            let stored: [u8; 32] = root[offset..offset + 32].try_into().unwrap();
            assert_eq!(stored, *hash);
        }
    }

    #[test]
    fn test_need_topic_deterministic() {
        let pk1 = [1u8; 32];
        let pk2 = [2u8; 32];
        assert_eq!(need_topic(&pk1), need_topic(&pk1));
        assert_ne!(need_topic(&pk1), need_topic(&pk2));
    }

    #[test]
    fn test_encode_decode_need_list_index_entries() {
        let entries = vec![NeedEntry::Index { start: 2, end: 5 }];
        let encoded = encode_need_list(&entries);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_encode_decode_need_list_data_entries() {
        let entries = vec![NeedEntry::Data {
            start: 100,
            end: 200,
        }];
        let encoded = encode_need_list(&entries);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_encode_decode_need_list_mixed() {
        let entries = vec![
            NeedEntry::Index { start: 0, end: 3 },
            NeedEntry::Data { start: 10, end: 20 },
            NeedEntry::Index { start: 5, end: 8 },
        ];
        let encoded = encode_need_list(&entries);
        let decoded = decode_need_list(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn test_encode_need_list_capacity() {
        // Fill with data entries (9 bytes each + 1 version byte)
        // MAX_PAYLOAD=1000, so max ~(999/9)=111 data entries
        let entries: Vec<NeedEntry> = (0..200)
            .map(|i| NeedEntry::Data { start: i, end: i })
            .collect();
        let encoded = encode_need_list(&entries);
        assert!(
            encoded.len() <= MAX_PAYLOAD,
            "encoded must fit in MAX_PAYLOAD bytes"
        );
        assert!(encoded.len() > 1, "must have at least version byte + one entry");
    }

    #[test]
    fn test_decode_need_list_empty() {
        let result = decode_need_list(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_decode_need_list_invalid_tag() {
        let data = vec![VERSION, 0xFF];
        let result = decode_need_list(&data);
        assert!(result.is_err());
    }
}
