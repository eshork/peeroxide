use blake2::digest::consts::U32;
use blake2::digest::{KeyInit, Mac};
use blake2::{Blake2b, Blake2bMac, Digest};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::sync::LazyLock;

type Blake2b256 = Blake2b<U32>;
type Blake2bMac256 = Blake2bMac<U32>;

// ── BLAKE2b primitives ──────────────────────────────────────────────────────

pub fn hash(data: &[u8]) -> [u8; 32] {
    let output = Blake2b256::digest(data);
    let mut result = [0u8; 32];
    result.copy_from_slice(&output);
    result
}

pub fn hash_batch(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Blake2b256::new();
    for part in parts {
        Digest::update(&mut h, part);
    }
    let output = h.finalize();
    let mut result = [0u8; 32];
    result.copy_from_slice(&output);
    result
}

/// BLAKE2b-256 keyed hash: data is `b"hypercore"`, key is the public key.
/// Mirrors `sodium.crypto_generichash(out, HYPERCORE, publicKey)`.
pub fn discovery_key(public_key: &[u8; 32]) -> [u8; 32] {
    let mut mac: Blake2bMac256 = KeyInit::new_from_slice(public_key.as_slice())
        .expect("32-byte key is always valid for BLAKE2b");
    mac.update(b"hypercore");
    let output = mac.finalize().into_bytes();
    let mut result = [0u8; 32];
    result.copy_from_slice(&output);
    result
}

// ── Namespace derivation ────────────────────────────────────────────────────

/// Derives signing namespaces from a string name and a list of command IDs.
/// Mirrors `hypercore-crypto/index.js:namespace(name, ids)`.
pub fn namespace(name: &str, ids: &[u8]) -> Vec<[u8; 32]> {
    let mut ns = [0u8; 33];
    let name_hash = Blake2b256::digest(name.as_bytes());
    ns[..32].copy_from_slice(&name_hash);

    let mut result = Vec::with_capacity(ids.len());
    for &id in ids {
        ns[32] = id;
        let h = Blake2b256::digest(&ns[..]);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&h);
        result.push(arr);
    }
    result
}

pub static NS_ANNOUNCE: LazyLock<[u8; 32]> =
    LazyLock::new(|| namespace("hyperswarm/dht", &[4])[0]);
pub static NS_UNANNOUNCE: LazyLock<[u8; 32]> =
    LazyLock::new(|| namespace("hyperswarm/dht", &[5])[0]);
pub static NS_MUTABLE_PUT: LazyLock<[u8; 32]> =
    LazyLock::new(|| namespace("hyperswarm/dht", &[6])[0]);
pub static NS_PEER_HANDSHAKE: LazyLock<[u8; 32]> =
    LazyLock::new(|| namespace("hyperswarm/dht", &[0])[0]);
pub static NS_PEER_HOLEPUNCH: LazyLock<[u8; 32]> =
    LazyLock::new(|| namespace("hyperswarm/dht", &[1])[0]);

// ── Ed25519 sign / verify ───────────────────────────────────────────────────

/// Sign `message` with `secret_key` (libsodium 64-byte format: seed || pubkey).
pub fn sign_detached(message: &[u8], secret_key: &[u8; 64]) -> [u8; 64] {
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&secret_key[..32]);
    let signing_key = SigningKey::from_bytes(&seed);
    signing_key.sign(message).to_bytes()
}

pub fn verify_detached(signature: &[u8; 64], message: &[u8], public_key: &[u8; 32]) -> bool {
    let Ok(verifying_key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    verifying_key.verify(message, &sig).is_ok()
}

// ── Signable buffers ────────────────────────────────────────────────────────

/// Build the 64-byte signable for announce / unannounce.
/// `peer_encoded` is the compact-encoding of the HyperPeer.
/// `refresh` is the raw refresh bytes (32-byte token) or `&[]` if absent.
pub fn ann_signable(
    target: &[u8; 32],
    token: &[u8; 32],
    id: &[u8; 32],
    peer_encoded: &[u8],
    refresh: &[u8],
    ns: &[u8; 32],
) -> [u8; 64] {
    let mut signable = [0u8; 64];
    signable[..32].copy_from_slice(ns);
    let h = hash_batch(&[&target[..], &id[..], &token[..], peer_encoded, refresh]);
    signable[32..].copy_from_slice(&h);
    signable
}

/// Build the 64-byte signable for mutable put.
/// Encodes `mutableSignable { seq, value }` then hashes it.
pub fn mutable_signable(ns: &[u8; 32], seq: u64, value: &[u8]) -> [u8; 64] {
    use crate::compact_encoding::{encode_buffer, encode_uint, preencode_buffer, preencode_uint};
    let mut state = crate::compact_encoding::State::new();
    preencode_uint(&mut state, seq);
    preencode_buffer(&mut state, Some(value));
    state.alloc();
    encode_uint(&mut state, seq);
    encode_buffer(&mut state, Some(value));

    let mut signable = [0u8; 64];
    signable[..32].copy_from_slice(ns);
    let h = hash(&state.buffer);
    signable[32..].copy_from_slice(&h);
    signable
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_length() {
        let ns = namespace("hyperswarm/dht", &[0, 1, 2, 3, 4]);
        assert_eq!(ns.len(), 5);
        for arr in &ns {
            assert_eq!(arr.len(), 32);
        }
    }

    #[test]
    fn namespace_deterministic() {
        let a = namespace("hyperswarm/dht", &[4]);
        let b = namespace("hyperswarm/dht", &[4]);
        assert_eq!(a, b);
    }

    #[test]
    fn namespace_different_ids_differ() {
        let ns = namespace("hyperswarm/dht", &[0, 1, 2, 3, 4, 5, 6]);
        for i in 0..ns.len() {
            for j in (i + 1)..ns.len() {
                assert_ne!(ns[i], ns[j], "namespace[{i}] == namespace[{j}]");
            }
        }
    }

    #[test]
    fn namespace_statics_non_zero() {
        assert_ne!(*NS_ANNOUNCE, [0u8; 32]);
        assert_ne!(*NS_UNANNOUNCE, [0u8; 32]);
        assert_ne!(*NS_MUTABLE_PUT, [0u8; 32]);
        assert_ne!(*NS_PEER_HANDSHAKE, [0u8; 32]);
        assert_ne!(*NS_PEER_HOLEPUNCH, [0u8; 32]);
    }

    #[test]
    fn namespace_statics_match_inline() {
        let ns = namespace("hyperswarm/dht", &[4, 5, 6, 0, 1]);
        assert_eq!(*NS_ANNOUNCE, ns[0]);
        assert_eq!(*NS_UNANNOUNCE, ns[1]);
        assert_eq!(*NS_MUTABLE_PUT, ns[2]);
        assert_eq!(*NS_PEER_HANDSHAKE, ns[3]);
        assert_eq!(*NS_PEER_HOLEPUNCH, ns[4]);
    }

    #[test]
    fn hash_is_deterministic() {
        assert_eq!(hash(b"hello"), hash(b"hello"));
        assert_ne!(hash(b"hello"), hash(b"world"));
    }

    #[test]
    fn hash_batch_equals_sequential() {
        let a = hash_batch(&[b"hello", b"world"]);
        let b = hash_batch(&[b"hello", b"world"]);
        assert_eq!(a, b);
        let concat = hash(b"helloworld");
        assert_eq!(a, concat);
        let c = hash_batch(&[b"hell", b"oworld"]);
        assert_eq!(a, c);
    }

    #[test]
    fn discovery_key_deterministic() {
        let pk = [0x42u8; 32];
        assert_eq!(discovery_key(&pk), discovery_key(&pk));
        let pk2 = [0x43u8; 32];
        assert_ne!(discovery_key(&pk), discovery_key(&pk2));
    }

    #[test]
    fn sign_verify_roundtrip() {
        let seed = [0x42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pk: [u8; 32] = signing_key.verifying_key().to_bytes();
        let mut sk = [0u8; 64];
        sk[..32].copy_from_slice(&seed);
        sk[32..].copy_from_slice(&pk);

        let msg = b"test message";
        let sig = sign_detached(msg, &sk);
        assert!(verify_detached(&sig, msg, &pk));
    }

    #[test]
    fn verify_bad_signature_fails() {
        let seed = [0x42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pk: [u8; 32] = signing_key.verifying_key().to_bytes();
        let mut sk = [0u8; 64];
        sk[..32].copy_from_slice(&seed);
        sk[32..].copy_from_slice(&pk);

        let msg = b"test message";
        let mut sig = sign_detached(msg, &sk);
        sig[0] ^= 0xff;
        assert!(!verify_detached(&sig, msg, &pk));
    }

    #[test]
    fn ann_signable_length() {
        let target = [0xaau8; 32];
        let token = [0xbbu8; 32];
        let id = [0xccu8; 32];
        let peer_encoded = [0u8; 33];
        let refresh = [];
        let ns = *NS_ANNOUNCE;
        let s = ann_signable(&target, &token, &id, &peer_encoded, &refresh, &ns);
        assert_eq!(s.len(), 64);
        assert_eq!(&s[..32], ns.as_slice());
    }

    #[test]
    fn mutable_signable_length() {
        let ns = *NS_MUTABLE_PUT;
        let s = mutable_signable(&ns, 42, b"hello");
        assert_eq!(s.len(), 64);
        assert_eq!(&s[..32], ns.as_slice());
    }
}
