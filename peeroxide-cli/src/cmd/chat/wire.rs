//! Wire format serialization/deserialization for all chat protocol record types,
//! plus XSalsa20Poly1305 encryption/decryption wrappers.
//!
//! Record layout specifications documented in `docs/src/chat/wire-format.md`.

use std::fmt;

use peeroxide_dht::crypto::{sign_detached, verify_detached};
use rand::RngCore;
use xsalsa20poly1305::aead::AeadInPlace;
use xsalsa20poly1305::{KeyInit, Nonce, Tag, XSalsa20Poly1305};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const CONTENT_TYPE_TEXT: u8 = 0x01;
pub const INVITE_TYPE_DM: u8 = 0x01;
pub const INVITE_TYPE_PRIVATE: u8 = 0x02;

pub const MAX_RECORD_SIZE: usize = 1000;

pub const MSG_FIXED_OVERHEAD: usize = 180;
pub const MAX_SCREEN_NAME_CONTENT: usize = 820;

const NONCE_SIZE: usize = 24;
const TAG_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum WireError {
    BufferTooShort { need: usize, got: usize },
    RecordTooLarge { size: usize },
    InvalidContentType(u8),
    InvalidInviteType(u8),
    InvalidUtf8(String),
    DecryptionFailed,
    SignatureInvalid,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::BufferTooShort { need, got } => {
                write!(f, "buffer too short: need {need} bytes, got {got}")
            }
            WireError::RecordTooLarge { size } => {
                write!(f, "record too large: {size} bytes exceeds {MAX_RECORD_SIZE} byte limit")
            }
            WireError::InvalidContentType(b) => {
                write!(f, "invalid content type: {b}")
            }
            WireError::InvalidInviteType(b) => {
                write!(f, "invalid invite type: {b}")
            }
            WireError::InvalidUtf8(field) => {
                write!(f, "invalid UTF-8 in field: {field}")
            }
            WireError::DecryptionFailed => write!(f, "decryption failed"),
            WireError::SignatureInvalid => write!(f, "signature verification failed"),
        }
    }
}

impl std::error::Error for WireError {}

// ---------------------------------------------------------------------------
// §7.1 MessageEnvelope
// ---------------------------------------------------------------------------
//
// Plaintext layout:
//   0       32    id_pubkey
//   32      32    prev_msg_hash
//   64       8    timestamp (u64 LE)
//   72       1    content_type
//   73       1    screen_name_len
//   74       N    screen_name (UTF-8)
//   74+N     2    content_len (u16 LE)
//   76+N     M    content (UTF-8)
//   76+N+M  64    signature
//
// Signature covers:
//   b"peeroxide-chat:msg:v1:" || prev_msg_hash(32) || timestamp(8 LE)
//   || content_type(1) || screen_name_len(1) || screen_name(N) || content(M)

/// A signed, encrypted chat message envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEnvelope {
    pub id_pubkey: [u8; 32],
    pub prev_msg_hash: [u8; 32],
    pub timestamp: u64,
    pub content_type: u8,
    pub screen_name: String,
    pub content: String,
    pub signature: [u8; 64],
}

impl MessageEnvelope {
    /// Serialize to plaintext bytes per the §7.1 layout.
    pub fn serialize(&self) -> Vec<u8> {
        let sn = self.screen_name.as_bytes();
        let ct = self.content.as_bytes();
        let total = 32 + 32 + 8 + 1 + 1 + sn.len() + 2 + ct.len() + 64;
        let mut buf = Vec::with_capacity(total);

        buf.extend_from_slice(&self.id_pubkey);
        buf.extend_from_slice(&self.prev_msg_hash);
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.push(self.content_type);
        buf.push(sn.len() as u8);
        buf.extend_from_slice(sn);
        buf.extend_from_slice(&(ct.len() as u16).to_le_bytes());
        buf.extend_from_slice(ct);
        buf.extend_from_slice(&self.signature);
        buf
    }

    /// Deserialize from plaintext bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self, WireError> {
        // Minimum: 32+32+8+1+1+2+64 = 140 bytes (zero-length screen_name + content)
        let min_len = 140;
        if data.len() < min_len {
            return Err(WireError::BufferTooShort { need: min_len, got: data.len() });
        }

        let mut pos = 0usize;

        let mut id_pubkey = [0u8; 32];
        id_pubkey.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let mut prev_msg_hash = [0u8; 32];
        prev_msg_hash.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let timestamp = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;

        let content_type = data[pos];
        pos += 1;
        if content_type != CONTENT_TYPE_TEXT {
            return Err(WireError::InvalidContentType(content_type));
        }

        let sn_len = data[pos] as usize;
        pos += 1;

        if data.len() < pos + sn_len + 2 {
            return Err(WireError::BufferTooShort {
                need: pos + sn_len + 2,
                got: data.len(),
            });
        }
        let screen_name = std::str::from_utf8(&data[pos..pos + sn_len])
            .map_err(|_| WireError::InvalidUtf8("screen_name".into()))?
            .to_owned();
        pos += sn_len;

        let ct_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        if data.len() < pos + ct_len + 64 {
            return Err(WireError::BufferTooShort {
                need: pos + ct_len + 64,
                got: data.len(),
            });
        }
        let content = std::str::from_utf8(&data[pos..pos + ct_len])
            .map_err(|_| WireError::InvalidUtf8("content".into()))?
            .to_owned();
        pos += ct_len;

        let mut signature = [0u8; 64];
        signature.copy_from_slice(&data[pos..pos + 64]);

        Ok(MessageEnvelope {
            id_pubkey,
            prev_msg_hash,
            timestamp,
            content_type,
            screen_name,
            content,
            signature,
        })
    }

    /// Builds and signs a new `MessageEnvelope`.
    ///
    /// `id_secret` is the 64-byte Ed25519 secret key (seed || pubkey as produced
    /// by `ed25519-dalek`); `id_pubkey` is the corresponding 32-byte public key.
    pub fn sign(
        id_secret: &[u8; 64],
        id_pubkey: [u8; 32],
        prev_msg_hash: [u8; 32],
        timestamp: u64,
        content_type: u8,
        screen_name: &str,
        content: &str,
    ) -> Self {
        let sn = screen_name.as_bytes();
        let ct = content.as_bytes();

        let msg = build_msg_signable(&prev_msg_hash, timestamp, content_type, sn, ct);
        let signature = sign_detached(&msg, id_secret);

        MessageEnvelope {
            id_pubkey,
            prev_msg_hash,
            timestamp,
            content_type,
            screen_name: screen_name.to_owned(),
            content: content.to_owned(),
            signature,
        }
    }

    /// Verifies the signature against the contained `id_pubkey`.
    pub fn verify(&self) -> bool {
        let sn = self.screen_name.as_bytes();
        let ct = self.content.as_bytes();
        let msg =
            build_msg_signable(&self.prev_msg_hash, self.timestamp, self.content_type, sn, ct);
        verify_detached(&self.signature, &msg, &self.id_pubkey)
    }
}

/// Build the byte buffer that is signed for a `MessageEnvelope`.
fn build_msg_signable(
    prev_msg_hash: &[u8; 32],
    timestamp: u64,
    content_type: u8,
    screen_name: &[u8],
    content: &[u8],
) -> Vec<u8> {
    let prefix = b"peeroxide-chat:msg:v1:";
    let mut msg = Vec::with_capacity(
        prefix.len() + 32 + 8 + 1 + 1 + screen_name.len() + content.len(),
    );
    msg.extend_from_slice(prefix);
    msg.extend_from_slice(prev_msg_hash);
    msg.extend_from_slice(&timestamp.to_le_bytes());
    msg.push(content_type);
    msg.push(screen_name.len() as u8);
    msg.extend_from_slice(screen_name);
    msg.extend_from_slice(content);
    msg
}

// ---------------------------------------------------------------------------
// §7.2 FeedRecord
// ---------------------------------------------------------------------------
//
// Plaintext layout:
//   0       32    id_pubkey
//   32      64    ownership_proof
//   96      32    next_feed_pubkey (32 zeros if none)
//   128     32    summary_hash (32 zeros if none)
//   160      1    msg_count
//   161    N×32   msg_hashes (newest first)

/// Mutable-put value for a user's feed head record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRecord {
    pub id_pubkey: [u8; 32],
    pub ownership_proof: [u8; 64],
    pub next_feed_pubkey: [u8; 32], // 32 zeros if none
    pub summary_hash: [u8; 32],     // 32 zeros if none
    pub msg_count: u8,              // 0–26
    pub msg_hashes: Vec<[u8; 32]>,  // newest first
}

impl FeedRecord {
    /// Serialize to bytes, returning `Err` if the result would exceed 1000 bytes.
    pub fn serialize(&self) -> Result<Vec<u8>, WireError> {
        let total = 32 + 64 + 32 + 32 + 1 + self.msg_hashes.len() * 32;
        if total > MAX_RECORD_SIZE {
            return Err(WireError::RecordTooLarge { size: total });
        }
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(&self.id_pubkey);
        buf.extend_from_slice(&self.ownership_proof);
        buf.extend_from_slice(&self.next_feed_pubkey);
        buf.extend_from_slice(&self.summary_hash);
        buf.push(self.msg_count);
        for h in &self.msg_hashes {
            buf.extend_from_slice(h);
        }
        Ok(buf)
    }

    /// Deserialize from bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self, WireError> {
        let min_len = 32 + 64 + 32 + 32 + 1; // 161
        if data.len() < min_len {
            return Err(WireError::BufferTooShort { need: min_len, got: data.len() });
        }

        let mut pos = 0usize;

        let mut id_pubkey = [0u8; 32];
        id_pubkey.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let mut ownership_proof = [0u8; 64];
        ownership_proof.copy_from_slice(&data[pos..pos + 64]);
        pos += 64;

        let mut next_feed_pubkey = [0u8; 32];
        next_feed_pubkey.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let mut summary_hash = [0u8; 32];
        summary_hash.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let msg_count = data[pos] as usize;
        pos += 1;

        if data.len() < pos + msg_count * 32 {
            return Err(WireError::BufferTooShort {
                need: pos + msg_count * 32,
                got: data.len(),
            });
        }

        let mut msg_hashes = Vec::with_capacity(msg_count);
        for _ in 0..msg_count {
            let mut h = [0u8; 32];
            h.copy_from_slice(&data[pos..pos + 32]);
            msg_hashes.push(h);
            pos += 32;
        }

        Ok(FeedRecord {
            id_pubkey,
            ownership_proof,
            next_feed_pubkey,
            summary_hash,
            msg_count: msg_count as u8,
            msg_hashes,
        })
    }
}

// ---------------------------------------------------------------------------
// §7.3 SummaryBlock
// ---------------------------------------------------------------------------
//
// Plaintext layout:
//   0       32    id_pubkey
//   32      32    prev_summary_hash (32 zeros if first)
//   64       1    msg_count
//   65     N×32   msg_hashes (oldest first, max 27)
//   65+N×32 64    signature
//
// Signature covers:
//   b"peeroxide-chat:summary:v1:" || prev_summary_hash(32) || msg_hashes(N×32)

/// Immutable-put value for a historical summary block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryBlock {
    pub id_pubkey: [u8; 32],
    pub prev_summary_hash: [u8; 32], // 32 zeros if first
    pub msg_count: u8,
    pub msg_hashes: Vec<[u8; 32]>, // oldest first, max 27
    pub signature: [u8; 64],
}

impl SummaryBlock {
    /// Serialize to bytes, returning `Err` if the result would exceed 1000 bytes.
    pub fn serialize(&self) -> Result<Vec<u8>, WireError> {
        let total = 32 + 32 + 1 + self.msg_hashes.len() * 32 + 64;
        if total > MAX_RECORD_SIZE {
            return Err(WireError::RecordTooLarge { size: total });
        }
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(&self.id_pubkey);
        buf.extend_from_slice(&self.prev_summary_hash);
        buf.push(self.msg_count);
        for h in &self.msg_hashes {
            buf.extend_from_slice(h);
        }
        buf.extend_from_slice(&self.signature);
        Ok(buf)
    }

    /// Deserialize from bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self, WireError> {
        let min_len = 32 + 32 + 1 + 64; // 129 (zero hashes)
        if data.len() < min_len {
            return Err(WireError::BufferTooShort { need: min_len, got: data.len() });
        }

        let mut pos = 0usize;

        let mut id_pubkey = [0u8; 32];
        id_pubkey.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let mut prev_summary_hash = [0u8; 32];
        prev_summary_hash.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let msg_count = data[pos] as usize;
        pos += 1;

        if data.len() < pos + msg_count * 32 + 64 {
            return Err(WireError::BufferTooShort {
                need: pos + msg_count * 32 + 64,
                got: data.len(),
            });
        }

        let mut msg_hashes = Vec::with_capacity(msg_count);
        for _ in 0..msg_count {
            let mut h = [0u8; 32];
            h.copy_from_slice(&data[pos..pos + 32]);
            msg_hashes.push(h);
            pos += 32;
        }

        let mut signature = [0u8; 64];
        signature.copy_from_slice(&data[pos..pos + 64]);

        Ok(SummaryBlock {
            id_pubkey,
            prev_summary_hash,
            msg_count: msg_count as u8,
            msg_hashes,
            signature,
        })
    }

    /// Build and sign a new `SummaryBlock`.
    pub fn sign(
        id_secret: &[u8; 64],
        id_pubkey: [u8; 32],
        prev_summary_hash: [u8; 32],
        msg_hashes: Vec<[u8; 32]>,
    ) -> Self {
        let msg = build_summary_signable(&prev_summary_hash, &msg_hashes);
        let signature = sign_detached(&msg, id_secret);
        let msg_count = msg_hashes.len() as u8;
        SummaryBlock {
            id_pubkey,
            prev_summary_hash,
            msg_count,
            msg_hashes,
            signature,
        }
    }

    /// Verify the signature against the contained `id_pubkey`.
    pub fn verify(&self) -> bool {
        let msg = build_summary_signable(&self.prev_summary_hash, &self.msg_hashes);
        verify_detached(&self.signature, &msg, &self.id_pubkey)
    }
}

/// Build the byte buffer that is signed for a `SummaryBlock`.
fn build_summary_signable(prev_summary_hash: &[u8; 32], msg_hashes: &[[u8; 32]]) -> Vec<u8> {
    let prefix = b"peeroxide-chat:summary:v1:";
    let mut msg = Vec::with_capacity(prefix.len() + 32 + msg_hashes.len() * 32);
    msg.extend_from_slice(prefix);
    msg.extend_from_slice(prev_summary_hash);
    for h in msg_hashes {
        msg.extend_from_slice(h);
    }
    msg
}

// ---------------------------------------------------------------------------
// §7.4 NexusRecord
// ---------------------------------------------------------------------------
//
// Plaintext layout:
//   0       1    name_len
//   1       N    name (UTF-8)
//   1+N     2    bio_len (u16 LE)
//   3+N     M    bio (UTF-8)

/// Mutable-put value for a user's public profile (name + bio).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NexusRecord {
    pub name: String,
    pub bio: String,
}

impl NexusRecord {
    /// Serialize to bytes, returning `Err` if the result would exceed 1000 bytes.
    pub fn serialize(&self) -> Result<Vec<u8>, WireError> {
        let name_bytes = self.name.as_bytes();
        let bio_bytes = self.bio.as_bytes();
        let total = 1 + name_bytes.len() + 2 + bio_bytes.len();
        if total > MAX_RECORD_SIZE {
            return Err(WireError::RecordTooLarge { size: total });
        }
        let mut buf = Vec::with_capacity(total);
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(&(bio_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(bio_bytes);
        Ok(buf)
    }

    /// Deserialize from bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self, WireError> {
        if data.is_empty() {
            return Err(WireError::BufferTooShort { need: 1, got: 0 });
        }

        let mut pos = 0usize;
        let name_len = data[pos] as usize;
        pos += 1;

        if data.len() < pos + name_len + 2 {
            return Err(WireError::BufferTooShort {
                need: pos + name_len + 2,
                got: data.len(),
            });
        }
        let name = std::str::from_utf8(&data[pos..pos + name_len])
            .map_err(|_| WireError::InvalidUtf8("name".into()))?
            .to_owned();
        pos += name_len;

        let bio_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        if data.len() < pos + bio_len {
            return Err(WireError::BufferTooShort {
                need: pos + bio_len,
                got: data.len(),
            });
        }
        let bio = std::str::from_utf8(&data[pos..pos + bio_len])
            .map_err(|_| WireError::InvalidUtf8("bio".into()))?
            .to_owned();

        Ok(NexusRecord { name, bio })
    }
}

// ---------------------------------------------------------------------------
// §7.5 InviteRecord
// ---------------------------------------------------------------------------
//
// Plaintext layout:
//   0       32    id_pubkey
//   32      64    ownership_proof
//   96      32    next_feed_pubkey
//   128      1    invite_type
//   129      2    payload_len (u16 LE)
//   131      N    payload

/// Encrypted invite record, carried inside an encrypted envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRecord {
    pub id_pubkey: [u8; 32],
    pub ownership_proof: [u8; 64],
    pub next_feed_pubkey: [u8; 32],
    pub invite_type: u8,
    pub payload: Vec<u8>,
}

impl InviteRecord {
    /// Serialize to bytes, returning `Err` if the result would exceed 1000 bytes.
    pub fn serialize(&self) -> Result<Vec<u8>, WireError> {
        let total = 32 + 64 + 32 + 1 + 2 + self.payload.len();
        if total > MAX_RECORD_SIZE {
            return Err(WireError::RecordTooLarge { size: total });
        }
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(&self.id_pubkey);
        buf.extend_from_slice(&self.ownership_proof);
        buf.extend_from_slice(&self.next_feed_pubkey);
        buf.push(self.invite_type);
        buf.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        buf.extend_from_slice(&self.payload);
        Ok(buf)
    }

    /// Deserialize from bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self, WireError> {
        let min_len = 32 + 64 + 32 + 1 + 2; // 131
        if data.len() < min_len {
            return Err(WireError::BufferTooShort { need: min_len, got: data.len() });
        }

        let mut pos = 0usize;

        let mut id_pubkey = [0u8; 32];
        id_pubkey.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let mut ownership_proof = [0u8; 64];
        ownership_proof.copy_from_slice(&data[pos..pos + 64]);
        pos += 64;

        let mut next_feed_pubkey = [0u8; 32];
        next_feed_pubkey.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        let invite_type = data[pos];
        pos += 1;
        if invite_type != INVITE_TYPE_DM && invite_type != INVITE_TYPE_PRIVATE {
            return Err(WireError::InvalidInviteType(invite_type));
        }

        let payload_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        if data.len() < pos + payload_len {
            return Err(WireError::BufferTooShort {
                need: pos + payload_len,
                got: data.len(),
            });
        }
        let payload = data[pos..pos + payload_len].to_vec();

        Ok(InviteRecord {
            id_pubkey,
            ownership_proof,
            next_feed_pubkey,
            invite_type,
            payload,
        })
    }
}

// ---------------------------------------------------------------------------
// Encryption wrappers
// ---------------------------------------------------------------------------

/// Encrypt `plaintext` using XSalsa20Poly1305 with a random nonce.
///
/// Wire format: `nonce(24) || tag(16) || ciphertext`
pub fn encrypt_message(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, WireError> {
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from(nonce_bytes);
    let cipher = XSalsa20Poly1305::new(key.into());

    let mut ciphertext = plaintext.to_vec();
    let tag = cipher
        .encrypt_in_place_detached(&nonce, b"", &mut ciphertext)
        .map_err(|_| WireError::DecryptionFailed)?;

    let mut result = Vec::with_capacity(NONCE_SIZE + TAG_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(tag.as_slice());
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data in wire format `nonce(24) || tag(16) || ciphertext`.
///
/// Returns the plaintext on success.
pub fn decrypt_message(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, WireError> {
    if data.len() < NONCE_SIZE + TAG_SIZE {
        return Err(WireError::BufferTooShort {
            need: NONCE_SIZE + TAG_SIZE,
            got: data.len(),
        });
    }

    let nonce = Nonce::from_slice(&data[..NONCE_SIZE]);
    let tag = Tag::from_slice(&data[NONCE_SIZE..NONCE_SIZE + TAG_SIZE]);
    let mut plaintext = data[NONCE_SIZE + TAG_SIZE..].to_vec();

    let cipher = XSalsa20Poly1305::new(key.into());
    cipher
        .decrypt_in_place_detached(nonce, b"", &mut plaintext, tag)
        .map_err(|_| WireError::DecryptionFailed)?;

    Ok(plaintext)
}

pub fn encrypt_invite(invite_key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, WireError> {
    let encrypted = encrypt_message(invite_key, plaintext)?;
    if encrypted.len() > MAX_RECORD_SIZE {
        return Err(WireError::RecordTooLarge {
            size: encrypted.len(),
        });
    }
    Ok(encrypted)
}

pub fn decrypt_invite(invite_key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, WireError> {
    decrypt_message(invite_key, data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    fn random_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        rand::rng().fill_bytes(&mut k);
        k
    }

    fn random_bytes<const N: usize>() -> [u8; N] {
        let mut b = [0u8; N];
        rand::rng().fill_bytes(&mut b);
        b
    }

    /// Generate a deterministic-ish Ed25519 keypair using ed25519-dalek for tests.
    fn make_keypair() -> ([u8; 64], [u8; 32]) {
        use ed25519_dalek::{SigningKey, VerifyingKey};
        let seed: [u8; 32] = random_bytes();
        let sk = SigningKey::from_bytes(&seed);
        let pk: VerifyingKey = (&sk).into();
        // ed25519-dalek's to_keypair_bytes() gives seed||pubkey
        let mut secret = [0u8; 64];
        secret[..32].copy_from_slice(&seed);
        secret[32..].copy_from_slice(pk.as_bytes());
        let pubkey: [u8; 32] = *pk.as_bytes();
        (secret, pubkey)
    }

    // --- MessageEnvelope ---

    #[test]
    fn message_envelope_round_trip() {
        let prev = random_bytes::<32>();
        let ts = 1_700_000_000u64;
        let (secret, pubkey) = make_keypair();

        let env = MessageEnvelope::sign(&secret, pubkey, prev, ts, CONTENT_TYPE_TEXT, "alice", "hello");
        let bytes = env.serialize();
        let env2 = MessageEnvelope::deserialize(&bytes).expect("deserialize");
        assert_eq!(env, env2);
    }

    #[test]
    fn message_envelope_sign_verify() {
        let prev = random_bytes::<32>();
        let (secret, pubkey) = make_keypair();
        let env = MessageEnvelope::sign(&secret, pubkey, prev, 42, CONTENT_TYPE_TEXT, "bob", "world");
        assert!(env.verify(), "signature must be valid");
    }

    #[test]
    fn message_envelope_verify_rejects_tampered() {
        let prev = random_bytes::<32>();
        let (secret, pubkey) = make_keypair();
        let mut env =
            MessageEnvelope::sign(&secret, pubkey, prev, 42, CONTENT_TYPE_TEXT, "carol", "secret");
        env.content = "tampered".to_owned();
        assert!(!env.verify(), "tampered content must fail verification");
    }

    #[test]
    fn message_envelope_max_content_fits() {
        let prev = random_bytes::<32>();
        let (secret, pubkey) = make_keypair();
        let content = "x".repeat(819);
        let env =
            MessageEnvelope::sign(&secret, pubkey, prev, 0, CONTENT_TYPE_TEXT, "a", &content);
        let bytes = env.serialize();
        assert!(bytes.len() <= MAX_RECORD_SIZE - 40, "plaintext must fit");
    }

    #[test]
    fn message_envelope_bad_content_type() {
        let prev = random_bytes::<32>();
        let (secret, pubkey) = make_keypair();
        let mut env = MessageEnvelope::sign(&secret, pubkey, prev, 0, CONTENT_TYPE_TEXT, "a", "b");
        env.content_type = 0xFF;
        let bytes = env.serialize();
        let result = MessageEnvelope::deserialize(&bytes);
        assert!(matches!(result, Err(WireError::InvalidContentType(0xFF))));
    }

    #[test]
    fn message_envelope_buffer_too_short() {
        let result = MessageEnvelope::deserialize(&[0u8; 10]);
        assert!(matches!(result, Err(WireError::BufferTooShort { .. })));
    }

    // --- FeedRecord ---

    #[test]
    fn feed_record_round_trip() {
        let rec = FeedRecord {
            id_pubkey: random_bytes::<32>(),
            ownership_proof: random_bytes::<64>(),
            next_feed_pubkey: [0u8; 32],
            summary_hash: [0u8; 32],
            msg_count: 3,
            msg_hashes: vec![random_bytes::<32>(), random_bytes::<32>(), random_bytes::<32>()],
        };
        let bytes = rec.serialize().expect("serialize");
        let rec2 = FeedRecord::deserialize(&bytes).expect("deserialize");
        assert_eq!(rec, rec2);
    }

    #[test]
    fn feed_record_empty_hashes() {
        let rec = FeedRecord {
            id_pubkey: [1u8; 32],
            ownership_proof: [2u8; 64],
            next_feed_pubkey: [0u8; 32],
            summary_hash: [0u8; 32],
            msg_count: 0,
            msg_hashes: vec![],
        };
        let bytes = rec.serialize().expect("serialize");
        let rec2 = FeedRecord::deserialize(&bytes).expect("deserialize");
        assert_eq!(rec, rec2);
    }

    #[test]
    fn feed_record_too_large() {
        // 27 hashes → 32+64+32+32+1+27*32 = 1025 > 1000
        let rec = FeedRecord {
            id_pubkey: [0u8; 32],
            ownership_proof: [0u8; 64],
            next_feed_pubkey: [0u8; 32],
            summary_hash: [0u8; 32],
            msg_count: 27,
            msg_hashes: vec![[0u8; 32]; 27],
        };
        assert!(matches!(rec.serialize(), Err(WireError::RecordTooLarge { .. })));
    }

    #[test]
    fn feed_record_buffer_too_short() {
        let result = FeedRecord::deserialize(&[0u8; 10]);
        assert!(matches!(result, Err(WireError::BufferTooShort { .. })));
    }

    // --- SummaryBlock ---

    #[test]
    fn summary_block_round_trip() {
        let (secret, pubkey) = make_keypair();
        let prev = random_bytes::<32>();
        let hashes: Vec<[u8; 32]> = (0..5).map(|_| random_bytes::<32>()).collect();
        let blk = SummaryBlock::sign(&secret, pubkey, prev, hashes);
        let bytes = blk.serialize().expect("serialize");
        let blk2 = SummaryBlock::deserialize(&bytes).expect("deserialize");
        assert_eq!(blk, blk2);
    }

    #[test]
    fn summary_block_sign_verify() {
        let (secret, pubkey) = make_keypair();
        let prev = random_bytes::<32>();
        let hashes: Vec<[u8; 32]> = (0..3).map(|_| random_bytes::<32>()).collect();
        let blk = SummaryBlock::sign(&secret, pubkey, prev, hashes);
        assert!(blk.verify());
    }

    #[test]
    fn summary_block_verify_rejects_tampered() {
        let (secret, pubkey) = make_keypair();
        let prev = random_bytes::<32>();
        let hashes: Vec<[u8; 32]> = (0..3).map(|_| random_bytes::<32>()).collect();
        let mut blk = SummaryBlock::sign(&secret, pubkey, prev, hashes);
        blk.msg_hashes[0] = [0xFF; 32];
        assert!(!blk.verify());
    }

    #[test]
    fn summary_block_buffer_too_short() {
        assert!(matches!(
            SummaryBlock::deserialize(&[0u8; 5]),
            Err(WireError::BufferTooShort { .. })
        ));
    }

    // --- NexusRecord ---

    #[test]
    fn nexus_record_round_trip() {
        let rec = NexusRecord {
            name: "Alice".to_owned(),
            bio: "Hello, world!".to_owned(),
        };
        let bytes = rec.serialize().expect("serialize");
        let rec2 = NexusRecord::deserialize(&bytes).expect("deserialize");
        assert_eq!(rec, rec2);
    }

    #[test]
    fn nexus_record_empty_fields() {
        let rec = NexusRecord { name: "".to_owned(), bio: "".to_owned() };
        let bytes = rec.serialize().expect("serialize");
        let rec2 = NexusRecord::deserialize(&bytes).expect("deserialize");
        assert_eq!(rec, rec2);
    }

    #[test]
    fn nexus_record_too_large() {
        let rec = NexusRecord {
            name: "a".repeat(255),
            bio: "b".repeat(750),
        };
        assert!(matches!(rec.serialize(), Err(WireError::RecordTooLarge { .. })));
    }

    #[test]
    fn nexus_record_buffer_too_short() {
        assert!(matches!(
            NexusRecord::deserialize(&[]),
            Err(WireError::BufferTooShort { .. })
        ));
    }

    // --- InviteRecord ---

    #[test]
    fn invite_record_dm_round_trip() {
        let rec = InviteRecord {
            id_pubkey: random_bytes::<32>(),
            ownership_proof: random_bytes::<64>(),
            next_feed_pubkey: random_bytes::<32>(),
            invite_type: INVITE_TYPE_DM,
            payload: b"some dm payload".to_vec(),
        };
        let bytes = rec.serialize().expect("serialize");
        let rec2 = InviteRecord::deserialize(&bytes).expect("deserialize");
        assert_eq!(rec, rec2);
    }

    #[test]
    fn invite_record_private_round_trip() {
        let rec = InviteRecord {
            id_pubkey: random_bytes::<32>(),
            ownership_proof: random_bytes::<64>(),
            next_feed_pubkey: random_bytes::<32>(),
            invite_type: INVITE_TYPE_PRIVATE,
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = rec.serialize().expect("serialize");
        let rec2 = InviteRecord::deserialize(&bytes).expect("deserialize");
        assert_eq!(rec, rec2);
    }

    #[test]
    fn invite_record_invalid_type() {
        let rec = InviteRecord {
            id_pubkey: [0u8; 32],
            ownership_proof: [0u8; 64],
            next_feed_pubkey: [0u8; 32],
            invite_type: INVITE_TYPE_DM,
            payload: vec![],
        };
        let mut bytes = rec.serialize().expect("serialize");
        // corrupt the invite_type byte (offset 128)
        bytes[128] = 0x99;
        assert!(matches!(
            InviteRecord::deserialize(&bytes),
            Err(WireError::InvalidInviteType(0x99))
        ));
    }

    #[test]
    fn invite_record_buffer_too_short() {
        assert!(matches!(
            InviteRecord::deserialize(&[0u8; 10]),
            Err(WireError::BufferTooShort { .. })
        ));
    }

    // --- Encryption ---

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = random_key();
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let ciphertext = encrypt_message(&key, plaintext).expect("encrypt");
        let decrypted = decrypt_message(&key, &ciphertext).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key1 = random_key();
        let key2 = random_key();
        let plaintext = b"secret message";
        let ciphertext = encrypt_message(&key1, plaintext).expect("encrypt");
        let result = decrypt_message(&key2, &ciphertext);
        assert!(matches!(result, Err(WireError::DecryptionFailed)));
    }

    #[test]
    fn decrypt_too_short_fails() {
        let key = random_key();
        let result = decrypt_message(&key, &[0u8; 10]);
        assert!(matches!(result, Err(WireError::BufferTooShort { .. })));
    }

    #[test]
    fn encrypt_empty_plaintext() {
        let key = random_key();
        let ct = encrypt_message(&key, b"").expect("encrypt");
        let pt = decrypt_message(&key, &ct).expect("decrypt");
        assert_eq!(pt, b"");
    }

    #[test]
    fn encrypt_invite_roundtrip() {
        let key = random_key();
        let plaintext = b"invite data here";
        let ct = encrypt_invite(&key, plaintext).expect("encrypt_invite");
        let pt = decrypt_invite(&key, &ct).expect("decrypt_invite");
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn encrypt_invite_rejects_oversized() {
        let key = random_key();
        let plaintext = vec![0xAB; 980];
        let result = encrypt_invite(&key, &plaintext);
        assert!(matches!(result, Err(WireError::RecordTooLarge { .. })));
    }

    #[test]
    fn encrypt_invite_max_size_boundary() {
        let key = random_key();
        let max_plaintext_size = MAX_RECORD_SIZE - NONCE_SIZE - TAG_SIZE;
        let plaintext = vec![0u8; max_plaintext_size];
        let result = encrypt_invite(&key, &plaintext);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), MAX_RECORD_SIZE);
    }
}
