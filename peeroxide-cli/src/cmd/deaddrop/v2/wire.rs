//! v3 wire-format encoders and decoders.
//!
//! Spec: see *Frame Formats* section of `DEADDROP_V3.md`.
//!
//! Layouts:
//!   data chunk:        `[ver: 0x02][salt: u8][payload: ≤998 B]`
//!   non-root index:    `[ver: 0x02][N × 32 B slots]`        with N ≤ 31
//!   root index:        `[ver: 0x02][file_size: u64 LE][crc32c: u32 LE][N × 32 B slots]`  with N ≤ 30
//!   need-list record:  `[ver: 0x02][count: u16 LE][count × {start: u32 LE, end: u32 LE}]`
//!
//! Slot kind (data hash vs child index pubkey) is derived from the chunk's
//! tree position, not encoded in the chunk. See `tree.rs`.

#![allow(dead_code)]

/// All v3 frames begin with this version byte.
pub const VERSION: u8 = 0x02;

/// DHT max-record size (set by hyperdht). Every encoded chunk must fit.
pub const MAX_CHUNK_SIZE: usize = 1000;

/// Data chunk header: version + salt.
pub const DATA_HEADER_SIZE: usize = 2;

/// Maximum payload bytes per data chunk (998 B).
pub const DATA_PAYLOAD_MAX: usize = MAX_CHUNK_SIZE - DATA_HEADER_SIZE;

/// Non-root index chunk header: version only.
pub const NON_ROOT_INDEX_HEADER_SIZE: usize = 1;

/// Maximum slots per non-root index chunk (31).
pub const NON_ROOT_INDEX_SLOT_CAP: usize = (MAX_CHUNK_SIZE - NON_ROOT_INDEX_HEADER_SIZE) / 32;

/// Root index chunk header: version + file_size (u64) + crc32c (u32).
pub const ROOT_INDEX_HEADER_SIZE: usize = 1 + 8 + 4;

/// Maximum slots per root index chunk (30).
pub const ROOT_INDEX_SLOT_CAP: usize = (MAX_CHUNK_SIZE - ROOT_INDEX_HEADER_SIZE) / 32;

/// Need-list header: version + u16 count.
pub const NEED_LIST_HEADER_SIZE: usize = 1 + 2;

/// Bytes per `NeedEntry`: u32 start + u32 end.
pub const NEED_ENTRY_SIZE: usize = 8;

/// Maximum entries per need-list record (124).
pub const NEED_LIST_ENTRY_CAP: usize =
    (MAX_CHUNK_SIZE - NEED_LIST_HEADER_SIZE) / NEED_ENTRY_SIZE;

/// SHA/BLAKE-256 size in bytes (for slot entries).
pub const HASH_LEN: usize = 32;

/// Errors that can arise when decoding v3 chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Empty,
    BadVersion(u8),
    Truncated { needed: usize, got: usize },
    BadSlotByteLength(usize),
    OversizedChunk(usize),
    BadCount { declared: u16, computed: usize },
    InvalidEntry { start: u32, end: u32 },
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Empty => write!(f, "empty chunk"),
            WireError::BadVersion(b) => write!(f, "bad version byte 0x{b:02x}, expected 0x02"),
            WireError::Truncated { needed, got } => {
                write!(f, "truncated chunk: need {needed} bytes, got {got}")
            }
            WireError::BadSlotByteLength(n) => {
                write!(f, "slot byte length {n} not a multiple of 32")
            }
            WireError::OversizedChunk(n) => {
                write!(f, "chunk size {n} exceeds MAX_CHUNK_SIZE ({MAX_CHUNK_SIZE})")
            }
            WireError::BadCount { declared, computed } => write!(
                f,
                "need-list count mismatch: declared {declared}, computed from length {computed}"
            ),
            WireError::InvalidEntry { start, end } => {
                write!(f, "invalid need-list entry: start={start} end={end} (need start < end)")
            }
        }
    }
}

impl std::error::Error for WireError {}

// ── Data chunks ─────────────────────────────────────────────────────────────

/// Encode a data chunk: `[VERSION][salt][payload]`.
pub fn encode_data_chunk(salt: u8, payload: &[u8]) -> Vec<u8> {
    debug_assert!(
        payload.len() <= DATA_PAYLOAD_MAX,
        "data payload {} exceeds DATA_PAYLOAD_MAX ({})",
        payload.len(),
        DATA_PAYLOAD_MAX
    );
    let mut buf = Vec::with_capacity(DATA_HEADER_SIZE + payload.len());
    buf.push(VERSION);
    buf.push(salt);
    buf.extend_from_slice(payload);
    buf
}

/// Verify a fetched data chunk and extract its payload bytes.
///
/// The DHT validates `discovery_key(chunk_bytes) == expected_address` before
/// returning, so the bytes here are already content-verified. We just check
/// that they have the right shape.
pub fn decode_data_chunk(bytes: &[u8]) -> Result<&[u8], WireError> {
    if bytes.is_empty() {
        return Err(WireError::Empty);
    }
    if bytes[0] != VERSION {
        return Err(WireError::BadVersion(bytes[0]));
    }
    if bytes.len() < DATA_HEADER_SIZE {
        return Err(WireError::Truncated {
            needed: DATA_HEADER_SIZE,
            got: bytes.len(),
        });
    }
    Ok(&bytes[DATA_HEADER_SIZE..])
}

// ── Index chunks ────────────────────────────────────────────────────────────

/// Encode the root index chunk: `[VERSION][file_size_u64_le][crc32c_u32_le][slots]`.
///
/// Slots are 32 bytes each. Their kind (data hash vs child index pubkey) is
/// determined by the canonical tree shape derived from `file_size`; the wire
/// format does not encode it.
pub fn encode_root_index(file_size: u64, crc32c: u32, slots: &[[u8; HASH_LEN]]) -> Vec<u8> {
    debug_assert!(
        slots.len() <= ROOT_INDEX_SLOT_CAP,
        "root slot count {} exceeds ROOT_INDEX_SLOT_CAP ({})",
        slots.len(),
        ROOT_INDEX_SLOT_CAP
    );
    let mut buf = Vec::with_capacity(ROOT_INDEX_HEADER_SIZE + slots.len() * HASH_LEN);
    buf.push(VERSION);
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(&crc32c.to_le_bytes());
    for slot in slots {
        buf.extend_from_slice(slot);
    }
    buf
}

/// Encode a non-root index chunk: `[VERSION][slots]`.
pub fn encode_non_root_index(slots: &[[u8; HASH_LEN]]) -> Vec<u8> {
    debug_assert!(
        slots.len() <= NON_ROOT_INDEX_SLOT_CAP,
        "non-root slot count {} exceeds NON_ROOT_INDEX_SLOT_CAP ({})",
        slots.len(),
        NON_ROOT_INDEX_SLOT_CAP
    );
    let mut buf = Vec::with_capacity(NON_ROOT_INDEX_HEADER_SIZE + slots.len() * HASH_LEN);
    buf.push(VERSION);
    for slot in slots {
        buf.extend_from_slice(slot);
    }
    buf
}

/// Parsed root index chunk.
#[derive(Debug, Clone)]
pub struct RootIndex {
    pub file_size: u64,
    pub crc32c: u32,
    pub slots: Vec<[u8; HASH_LEN]>,
}

/// Decode a root index chunk into its fields plus slot vector.
pub fn decode_root_index(bytes: &[u8]) -> Result<RootIndex, WireError> {
    if bytes.is_empty() {
        return Err(WireError::Empty);
    }
    if bytes[0] != VERSION {
        return Err(WireError::BadVersion(bytes[0]));
    }
    if bytes.len() < ROOT_INDEX_HEADER_SIZE {
        return Err(WireError::Truncated {
            needed: ROOT_INDEX_HEADER_SIZE,
            got: bytes.len(),
        });
    }
    if bytes.len() > MAX_CHUNK_SIZE {
        return Err(WireError::OversizedChunk(bytes.len()));
    }

    let file_size = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
    let crc32c = u32::from_le_bytes(bytes[9..13].try_into().unwrap());
    let slot_bytes = &bytes[ROOT_INDEX_HEADER_SIZE..];
    if slot_bytes.len() % HASH_LEN != 0 {
        return Err(WireError::BadSlotByteLength(slot_bytes.len()));
    }
    let slots: Vec<[u8; HASH_LEN]> = slot_bytes
        .chunks_exact(HASH_LEN)
        .map(|c| {
            let mut h = [0u8; HASH_LEN];
            h.copy_from_slice(c);
            h
        })
        .collect();
    Ok(RootIndex {
        file_size,
        crc32c,
        slots,
    })
}

/// Decode a non-root index chunk into its slot vector.
pub fn decode_non_root_index(bytes: &[u8]) -> Result<Vec<[u8; HASH_LEN]>, WireError> {
    if bytes.is_empty() {
        return Err(WireError::Empty);
    }
    if bytes[0] != VERSION {
        return Err(WireError::BadVersion(bytes[0]));
    }
    if bytes.len() > MAX_CHUNK_SIZE {
        return Err(WireError::OversizedChunk(bytes.len()));
    }
    let slot_bytes = &bytes[NON_ROOT_INDEX_HEADER_SIZE..];
    if slot_bytes.len() % HASH_LEN != 0 {
        return Err(WireError::BadSlotByteLength(slot_bytes.len()));
    }
    let slots: Vec<[u8; HASH_LEN]> = slot_bytes
        .chunks_exact(HASH_LEN)
        .map(|c| {
            let mut h = [0u8; HASH_LEN];
            h.copy_from_slice(c);
            h
        })
        .collect();
    Ok(slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_capacities_match_spec() {
        assert_eq!(ROOT_INDEX_SLOT_CAP, 30);
        assert_eq!(NON_ROOT_INDEX_SLOT_CAP, 31);
        assert_eq!(DATA_PAYLOAD_MAX, 998);
        assert_eq!(NEED_LIST_ENTRY_CAP, 124);
    }

    #[test]
    fn data_chunk_roundtrip() {
        let payload = b"hello world";
        let encoded = encode_data_chunk(0xAB, payload);
        assert_eq!(encoded[0], VERSION);
        assert_eq!(encoded[1], 0xAB);
        assert_eq!(&encoded[2..], payload);
        assert_eq!(decode_data_chunk(&encoded).unwrap(), payload);
    }

    #[test]
    fn data_chunk_max_payload() {
        let payload = vec![0xFFu8; DATA_PAYLOAD_MAX];
        let encoded = encode_data_chunk(0, &payload);
        assert_eq!(encoded.len(), MAX_CHUNK_SIZE);
        assert_eq!(decode_data_chunk(&encoded).unwrap(), &payload[..]);
    }

    #[test]
    fn data_chunk_empty_payload() {
        // Theoretically allowed by the format but never produced by the canonical
        // sender (data chunks always carry at least 1 byte).
        let encoded = encode_data_chunk(0, b"");
        assert_eq!(encoded.len(), DATA_HEADER_SIZE);
        assert_eq!(decode_data_chunk(&encoded).unwrap(), b"");
    }

    #[test]
    fn data_chunk_rejects_bad_version() {
        assert_eq!(
            decode_data_chunk(&[0x01, 0xAA, 0xBB]),
            Err(WireError::BadVersion(0x01))
        );
    }

    #[test]
    fn data_chunk_rejects_empty() {
        assert_eq!(decode_data_chunk(&[]), Err(WireError::Empty));
    }

    #[test]
    fn data_chunk_rejects_truncated_header() {
        assert_eq!(
            decode_data_chunk(&[VERSION]),
            Err(WireError::Truncated {
                needed: DATA_HEADER_SIZE,
                got: 1
            })
        );
    }

    #[test]
    fn root_index_roundtrip_with_slots() {
        let slots: Vec<[u8; 32]> = (0..30).map(|i| [i as u8; 32]).collect();
        let encoded = encode_root_index(123_456_789_u64, 0xDEAD_BEEF_u32, &slots);
        assert_eq!(encoded.len(), ROOT_INDEX_HEADER_SIZE + 30 * 32);
        let decoded = decode_root_index(&encoded).unwrap();
        assert_eq!(decoded.file_size, 123_456_789);
        assert_eq!(decoded.crc32c, 0xDEAD_BEEF);
        assert_eq!(decoded.slots, slots);
    }

    #[test]
    fn root_index_empty_slots() {
        let encoded = encode_root_index(0, 0, &[]);
        assert_eq!(encoded.len(), ROOT_INDEX_HEADER_SIZE);
        let decoded = decode_root_index(&encoded).unwrap();
        assert_eq!(decoded.file_size, 0);
        assert_eq!(decoded.crc32c, 0);
        assert!(decoded.slots.is_empty());
    }

    #[test]
    fn root_index_rejects_bad_slot_alignment() {
        let mut bytes = encode_root_index(100, 0, &[[0u8; 32]; 1]);
        bytes.pop(); // drop one byte → slot bytes are 31, not multiple of 32
        assert!(matches!(
            decode_root_index(&bytes),
            Err(WireError::BadSlotByteLength(_))
        ));
    }

    #[test]
    fn root_index_rejects_truncated_header() {
        let bytes = vec![VERSION, 0, 0, 0]; // less than 13 bytes
        assert!(matches!(
            decode_root_index(&bytes),
            Err(WireError::Truncated { .. })
        ));
    }

    #[test]
    fn root_index_rejects_oversized() {
        let bytes = vec![VERSION; MAX_CHUNK_SIZE + 1];
        assert!(matches!(
            decode_root_index(&bytes),
            Err(WireError::OversizedChunk(_))
        ));
    }

    #[test]
    fn non_root_index_roundtrip() {
        let slots: Vec<[u8; 32]> = (0..31).map(|i| [(i * 3) as u8; 32]).collect();
        let encoded = encode_non_root_index(&slots);
        assert_eq!(encoded.len(), NON_ROOT_INDEX_HEADER_SIZE + 31 * 32);
        let decoded = decode_non_root_index(&encoded).unwrap();
        assert_eq!(decoded, slots);
    }

    #[test]
    fn non_root_index_partial_slots() {
        let slots: Vec<[u8; 32]> = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
        let encoded = encode_non_root_index(&slots);
        assert_eq!(encoded.len(), NON_ROOT_INDEX_HEADER_SIZE + 3 * 32);
        let decoded = decode_non_root_index(&encoded).unwrap();
        assert_eq!(decoded, slots);
    }

    #[test]
    fn non_root_index_rejects_bad_alignment() {
        let mut bytes = encode_non_root_index(&[[0u8; 32]]);
        bytes.pop();
        assert!(matches!(
            decode_non_root_index(&bytes),
            Err(WireError::BadSlotByteLength(_))
        ));
    }

    #[test]
    fn non_root_index_rejects_bad_version() {
        let mut bytes = vec![0u8; 33];
        bytes[0] = 0x01;
        assert_eq!(
            decode_non_root_index(&bytes),
            Err(WireError::BadVersion(0x01))
        );
    }
}
