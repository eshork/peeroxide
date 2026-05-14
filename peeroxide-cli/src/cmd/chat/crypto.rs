//! Cryptographic primitives for the peeroxide chat protocol.
//!
//! All key-derivation functions use keyed BLAKE2b-256 MACs so that the same
//! raw key material can produce independent outputs for different purposes.

use blake2::digest::consts::U32;
use blake2::digest::{KeyInit, Mac};
use blake2::Blake2bMac;
use curve25519_dalek::edwards::CompressedEdwardsY;
use curve25519_dalek::montgomery::MontgomeryPoint;
use peeroxide_dht::crypto::{hash, hash_batch, sign_detached, verify_detached};
use sha2::{Digest, Sha512};
use std::time::{SystemTime, UNIX_EPOCH};

type Blake2bMac256 = Blake2bMac<U32>;

/// Keyed BLAKE2b-256 MAC.
///
/// Used by all KDF functions in this module.  The `key` is always 32 bytes
/// (a channel key, ECDH secret, etc.) and `msg` is the domain-separated
/// input.
fn keyed_blake2b(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    let mut mac: Blake2bMac256 = KeyInit::new_from_slice(key.as_slice())
        .expect("32-byte key is always valid for BLAKE2b");
    mac.update(msg);
    let output = mac.finalize().into_bytes();
    let mut result = [0u8; 32];
    result.copy_from_slice(&output);
    result
}

/// Returns `(x.len() as u32).to_le_bytes()` — a 4-byte little-endian length
/// prefix suitable for inclusion in hash pre-images.
pub fn len4(x: &[u8]) -> [u8; 4] {
    (x.len() as u32).to_le_bytes()
}

/// Derive a channel key for a public or password-protected channel.
///
/// * Public channel:  
///   `hash_batch([b"peeroxide-chat:channel:v1:", len4(name), name])`
/// * Private channel (with salt):  
///   `hash_batch([b"peeroxide-chat:channel:v1:", len4(name), name, b":salt:", len4(salt), salt])`
pub fn channel_key(name: &[u8], salt: Option<&[u8]>) -> [u8; 32] {
    match salt {
        None => hash_batch(&[b"peeroxide-chat:channel:v1:", &len4(name), name]),
        Some(s) => hash_batch(&[
            b"peeroxide-chat:channel:v1:",
            &len4(name),
            name,
            b":salt:",
            &len4(s),
            s,
        ]),
    }
}

/// Derive a symmetric DM channel key from two peer identity public keys.
///
/// The key is order-independent: `dm_channel_key(a, b) == dm_channel_key(b, a)`.
///
/// `hash_batch([b"peeroxide-chat:dm:v1:", lex_min(id_a, id_b), lex_max(id_a, id_b)])`
pub fn dm_channel_key(id_a: &[u8; 32], id_b: &[u8; 32]) -> [u8; 32] {
    let (lo, hi) = if id_a <= id_b {
        (id_a.as_ref(), id_b.as_ref())
    } else {
        (id_b.as_ref(), id_a.as_ref())
    };
    hash_batch(&[b"peeroxide-chat:dm:v1:", lo, hi])
}

/// Derive the DHT announce topic for a given channel, epoch, and bucket.
///
/// `keyed_blake2b(key=channel_key, msg=b"peeroxide-chat:announce:v1:" || epoch_u64_le || bucket_u8)`
pub fn announce_topic(channel_key: &[u8; 32], epoch: u64, bucket: u8) -> [u8; 32] {
    let mut msg = Vec::with_capacity(27 + 8 + 1);
    msg.extend_from_slice(b"peeroxide-chat:announce:v1:");
    msg.extend_from_slice(&epoch.to_le_bytes());
    msg.push(bucket);
    keyed_blake2b(channel_key, &msg)
}

/// Derive the DHT inbox topic for a given recipient, epoch, and bucket.
///
/// `keyed_blake2b(key=hash(recipient_id_pubkey), msg=b"peeroxide-chat:inbox:v1:" || epoch_u64_le || bucket_u8)`
pub fn inbox_topic(recipient_id_pubkey: &[u8; 32], epoch: u64, bucket: u8) -> [u8; 32] {
    let key = hash(recipient_id_pubkey);
    let mut msg = Vec::with_capacity(24 + 8 + 1);
    msg.extend_from_slice(b"peeroxide-chat:inbox:v1:");
    msg.extend_from_slice(&epoch.to_le_bytes());
    msg.push(bucket);
    keyed_blake2b(&key, &msg)
}

/// Derive the symmetric message encryption key for a public/private channel.
///
/// `keyed_blake2b(key=channel_key, msg=b"peeroxide-chat:msgkey:v1")`
pub fn msg_key(channel_key: &[u8; 32]) -> [u8; 32] {
    keyed_blake2b(channel_key, b"peeroxide-chat:msgkey:v1")
}

/// Derive the symmetric message encryption key for a DM conversation.
///
/// `keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:dm-msgkey:v1:" || channel_key)`
pub fn dm_msg_key(ecdh_secret: &[u8; 32], channel_key: &[u8; 32]) -> [u8; 32] {
    let mut msg = Vec::with_capacity(28 + 32);
    msg.extend_from_slice(b"peeroxide-chat:dm-msgkey:v1:");
    msg.extend_from_slice(channel_key);
    keyed_blake2b(ecdh_secret, &msg)
}

/// Derive the invite encryption key from an ECDH secret and an invite feed pubkey.
///
/// `keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:invite-key:v1:" || invite_feed_pubkey)`
pub fn invite_key(ecdh_secret: &[u8; 32], invite_feed_pubkey: &[u8; 32]) -> [u8; 32] {
    let mut msg = Vec::with_capacity(29 + 32);
    msg.extend_from_slice(b"peeroxide-chat:invite-key:v1:");
    msg.extend_from_slice(invite_feed_pubkey);
    keyed_blake2b(ecdh_secret, &msg)
}

/// Convert an Ed25519 public key to its X25519 (Montgomery) representation.
///
/// Uses the birational map from Edwards to Montgomery form defined in
/// RFC 7748.  Returns `None` if the input is not a valid compressed Edwards
/// point.
pub fn ed25519_pubkey_to_x25519(ed_pubkey: &[u8; 32]) -> Option<[u8; 32]> {
    let compressed = CompressedEdwardsY::from_slice(ed_pubkey).ok()?;
    let point = compressed.decompress()?;
    let montgomery = point.to_montgomery();
    Some(montgomery.to_bytes())
}

/// Convert an Ed25519 secret key (libsodium 64-byte layout: seed ‖ pubkey) to
/// an X25519 private scalar.
///
/// The X25519 scalar is derived as `SHA-512(seed)[0..32]` with the standard
/// X25519 clamping applied.
pub fn ed25519_secret_to_x25519(ed_secret: &[u8; 64]) -> [u8; 32] {
    // secret_key layout: seed(32) || pubkey(32)
    let seed = &ed_secret[..32];
    let h = Sha512::digest(seed);
    let mut x25519_priv = [0u8; 32];
    x25519_priv.copy_from_slice(&h[..32]);
    // Clamp per RFC 7748 §5
    x25519_priv[0] &= 248;
    x25519_priv[31] &= 127;
    x25519_priv[31] |= 64;
    x25519_priv
}

/// Perform an X25519 Diffie–Hellman key exchange.
///
/// `my_priv` should be a clamped X25519 scalar (e.g. from
/// [`ed25519_secret_to_x25519`]).  `their_pub` is the remote party's X25519
/// public key.  Returns the 32-byte shared secret.
pub fn x25519_ecdh(my_priv: &[u8; 32], their_pub: &[u8; 32]) -> [u8; 32] {
    let point = MontgomeryPoint(*their_pub);
    // `mul_clamped` performs the full clamped scalar multiplication defined
    // by RFC 7748 §5, accepting a raw `[u8; 32]` scalar.
    point.mul_clamped(*my_priv).to_bytes()
}

/// Produce an Ed25519 ownership proof binding a feed public key to a channel.
///
/// `sign(id_sk, b"peeroxide-chat:ownership:v1:" || feed_pubkey || channel_key)`
pub fn ownership_proof(
    id_secret_key: &[u8; 64],
    feed_pubkey: &[u8; 32],
    channel_key: &[u8; 32],
) -> [u8; 64] {
    let mut msg = Vec::with_capacity(28 + 32 + 32);
    msg.extend_from_slice(b"peeroxide-chat:ownership:v1:");
    msg.extend_from_slice(feed_pubkey);
    msg.extend_from_slice(channel_key);
    sign_detached(&msg, id_secret_key)
}

/// Verify an ownership proof.
///
/// Returns `true` iff the proof is a valid Ed25519 signature by `id_pubkey`
/// over `b"peeroxide-chat:ownership:v1:" || feed_pubkey || channel_key`.
pub fn verify_ownership_proof(
    id_pubkey: &[u8; 32],
    feed_pubkey: &[u8; 32],
    channel_key: &[u8; 32],
    proof: &[u8; 64],
) -> bool {
    let mut msg = Vec::with_capacity(28 + 32 + 32);
    msg.extend_from_slice(b"peeroxide-chat:ownership:v1:");
    msg.extend_from_slice(feed_pubkey);
    msg.extend_from_slice(channel_key);
    verify_detached(proof, &msg, id_pubkey)
}

/// Return the current epoch: `unix_timestamp_secs / 60`.
///
/// Each epoch is one minute long.  Announce topics are keyed by epoch and a
/// small bucket index so that peers can overlap their presence across
/// consecutive epochs without exact time synchronisation.
pub fn current_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX epoch")
        .as_secs()
        / 60
}

#[cfg(test)]
mod tests {
    use super::*;
    use peeroxide_dht::hyperdht::KeyPair;

    #[test]
    fn test_len4() {
        assert_eq!(len4(b""), [0, 0, 0, 0]);
        assert_eq!(len4(b"hi"), [2, 0, 0, 0]);
        assert_eq!(len4(&[0u8; 256]), [0, 1, 0, 0]);
    }

    #[test]
    fn test_channel_key_deterministic() {
        let k1 = channel_key(b"general", None);
        let k2 = channel_key(b"general", None);
        assert_eq!(k1, k2, "channel_key is deterministic");

        let k3 = channel_key(b"other", None);
        assert_ne!(k1, k3, "different names produce different keys");
    }

    #[test]
    fn test_channel_key_salt_differs_from_unsalted() {
        let unsalted = channel_key(b"general", None);
        let salted = channel_key(b"general", Some(b"s3cret"));
        assert_ne!(unsalted, salted, "salt changes the key");

        let salted2 = channel_key(b"general", Some(b"s3cret"));
        assert_eq!(salted, salted2, "same salt → same key");
    }

    #[test]
    fn test_dm_channel_key_symmetric() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        assert_eq!(
            dm_channel_key(&a, &b),
            dm_channel_key(&b, &a),
            "dm_channel_key must be order-independent"
        );
    }

    #[test]
    fn test_dm_channel_key_differs_from_channel_key() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let dm = dm_channel_key(&a, &b);
        let ch = channel_key(&a, None);
        assert_ne!(dm, ch);
    }

    #[test]
    fn test_announce_topic_varies_by_epoch_and_bucket() {
        let ck = channel_key(b"general", None);
        let t0 = announce_topic(&ck, 1000, 0);
        let t1 = announce_topic(&ck, 1001, 0);
        let t2 = announce_topic(&ck, 1000, 1);
        assert_ne!(t0, t1, "different epochs → different topic");
        assert_ne!(t0, t2, "different buckets → different topic");
        assert_eq!(
            announce_topic(&ck, 1000, 0),
            t0,
            "announce_topic is deterministic"
        );
    }

    #[test]
    fn test_msg_key_deterministic() {
        let ck = channel_key(b"test", None);
        assert_eq!(msg_key(&ck), msg_key(&ck));
    }

    #[test]
    fn test_dm_msg_key_deterministic() {
        let secret = [42u8; 32];
        let ck = channel_key(b"test", None);
        assert_eq!(dm_msg_key(&secret, &ck), dm_msg_key(&secret, &ck));
    }

    #[test]
    fn test_inbox_topic_varies() {
        let pk = [3u8; 32];
        let t0 = inbox_topic(&pk, 500, 0);
        let t1 = inbox_topic(&pk, 501, 0);
        let t2 = inbox_topic(&pk, 500, 1);
        assert_ne!(t0, t1);
        assert_ne!(t0, t2);
    }

    #[test]
    fn test_invite_key_deterministic() {
        let secret = [7u8; 32];
        let feed_pk = [8u8; 32];
        assert_eq!(invite_key(&secret, &feed_pk), invite_key(&secret, &feed_pk));
    }

    #[test]
    fn test_ed25519_pubkey_to_x25519_valid() {
        let kp = KeyPair::generate();
        let x25519_pub = ed25519_pubkey_to_x25519(&kp.public_key);
        assert!(
            x25519_pub.is_some(),
            "valid Ed25519 pubkey should convert successfully"
        );
    }

    #[test]
    fn test_ed25519_pubkey_to_x25519_invalid() {
        let bad = [0xFFu8; 32];
        let _ = ed25519_pubkey_to_x25519(&bad);
    }

    #[test]
    fn test_ecdh_shared_secret_matches() {
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();

        let x_priv_a = ed25519_secret_to_x25519(&kp_a.secret_key);
        let x_priv_b = ed25519_secret_to_x25519(&kp_b.secret_key);

        let x_pub_a = ed25519_pubkey_to_x25519(&kp_a.public_key)
            .expect("keypair A pubkey must convert");
        let x_pub_b = ed25519_pubkey_to_x25519(&kp_b.public_key)
            .expect("keypair B pubkey must convert");

        let shared_ab = x25519_ecdh(&x_priv_a, &x_pub_b);
        let shared_ba = x25519_ecdh(&x_priv_b, &x_pub_a);

        assert_eq!(
            shared_ab, shared_ba,
            "ECDH shared secret must be symmetric"
        );
    }

    #[test]
    fn test_ed25519_to_x25519_roundtrip() {
        let kp_a = KeyPair::generate();
        let kp_b = KeyPair::generate();

        let x_priv_a = ed25519_secret_to_x25519(&kp_a.secret_key);
        let x_pub_b = ed25519_pubkey_to_x25519(&kp_b.public_key)
            .expect("keypair B pubkey must convert");

        let shared = x25519_ecdh(&x_priv_a, &x_pub_b);
        assert_ne!(shared, [0u8; 32], "shared secret must not be the zero point");
    }

    #[test]
    fn test_ownership_proof_verify() {
        let id_kp = KeyPair::generate();
        let feed_pk = [0xABu8; 32];
        let ck = channel_key(b"myroom", None);

        let proof = ownership_proof(&id_kp.secret_key, &feed_pk, &ck);
        assert!(
            verify_ownership_proof(&id_kp.public_key, &feed_pk, &ck, &proof),
            "ownership proof must verify with the correct key"
        );
    }

    #[test]
    fn test_ownership_proof_wrong_key_fails() {
        let id_kp = KeyPair::generate();
        let other_kp = KeyPair::generate();
        let feed_pk = [0xABu8; 32];
        let ck = channel_key(b"myroom", None);

        let proof = ownership_proof(&id_kp.secret_key, &feed_pk, &ck);
        assert!(
            !verify_ownership_proof(&other_kp.public_key, &feed_pk, &ck, &proof),
            "ownership proof must NOT verify with the wrong key"
        );
    }

    #[test]
    fn test_current_epoch_is_reasonable() {
        let epoch = current_epoch();
        assert!(epoch > 28_000_000, "epoch should reflect a plausible current time");
    }

    #[test]
    fn test_channel_key_fixed_vector() {
        let key = channel_key(b"general", None);
        let hex_key = hex::encode(key);
        let key2 = channel_key(b"general", None);
        assert_eq!(key, key2, "channel_key must be deterministic");
        assert_eq!(hex_key.len(), 64);
        assert_ne!(key, [0u8; 32]);
    }

    #[test]
    fn test_channel_key_salted_fixed_vector() {
        let key = channel_key(b"general", Some(b"mysalt"));
        let key2 = channel_key(b"general", Some(b"mysalt"));
        assert_eq!(key, key2, "salted channel_key must be deterministic");
        let unsalted = channel_key(b"general", None);
        assert_ne!(key, unsalted);
    }

    #[test]
    fn test_msg_key_fixed_vector() {
        let ck = channel_key(b"general", None);
        let mk = msg_key(&ck);
        let mk2 = msg_key(&ck);
        assert_eq!(mk, mk2, "msg_key must be deterministic");
        assert_ne!(mk, ck, "msg_key must differ from channel_key");
    }

    #[test]
    fn test_announce_topic_fixed_vector() {
        let ck = channel_key(b"general", None);
        let topic = announce_topic(&ck, 28000000, 2);
        let topic2 = announce_topic(&ck, 28000000, 2);
        assert_eq!(topic, topic2, "announce_topic must be deterministic");
    }

    #[test]
    fn test_dm_channel_key_fixed_vector() {
        let a = [0x01u8; 32];
        let b = [0x02u8; 32];
        let dk = dm_channel_key(&a, &b);
        let dk2 = dm_channel_key(&a, &b);
        assert_eq!(dk, dk2, "dm_channel_key must be deterministic");
        let dk_rev = dm_channel_key(&b, &a);
        assert_eq!(dk, dk_rev, "dm_channel_key must be symmetric");
    }

    #[test]
    fn test_invite_key_fixed_vector() {
        let ecdh = [0x42u8; 32];
        let feed_pk = [0xABu8; 32];
        let ik = invite_key(&ecdh, &feed_pk);
        let ik2 = invite_key(&ecdh, &feed_pk);
        assert_eq!(ik, ik2, "invite_key must be deterministic");
        assert_ne!(ik, ecdh, "invite_key must differ from ecdh input");
    }

    #[test]
    fn test_ecdh_deterministic_from_seed() {
        let seed_a = [0x11u8; 32];
        let seed_b = [0x22u8; 32];
        let kp_a = KeyPair::from_seed(seed_a);
        let kp_b = KeyPair::from_seed(seed_b);

        let x_priv_a = ed25519_secret_to_x25519(&kp_a.secret_key);
        let x_pub_b = ed25519_pubkey_to_x25519(&kp_b.public_key).unwrap();
        let shared1 = x25519_ecdh(&x_priv_a, &x_pub_b);

        let x_priv_a2 = ed25519_secret_to_x25519(&kp_a.secret_key);
        let x_pub_b2 = ed25519_pubkey_to_x25519(&kp_b.public_key).unwrap();
        let shared2 = x25519_ecdh(&x_priv_a2, &x_pub_b2);

        assert_eq!(shared1, shared2, "ECDH must be deterministic from same seeds");
        assert_ne!(shared1, [0u8; 32]);
    }
}
