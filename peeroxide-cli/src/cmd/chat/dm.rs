use crate::cmd::chat::crypto;

pub fn dm_channel_key(my_pubkey: &[u8; 32], their_pubkey: &[u8; 32]) -> [u8; 32] {
    crypto::dm_channel_key(my_pubkey, their_pubkey)
}

pub fn dm_msg_key(my_secret: &[u8; 64], their_pubkey: &[u8; 32], channel_key: &[u8; 32]) -> [u8; 32] {
    let my_x25519 = crypto::ed25519_secret_to_x25519(my_secret);
    let their_x25519 = match crypto::ed25519_pubkey_to_x25519(their_pubkey) {
        Some(pk) => pk,
        None => return [0u8; 32],
    };
    let ecdh_secret = crypto::x25519_ecdh(&my_x25519, &their_x25519);
    crypto::dm_msg_key(&ecdh_secret, channel_key)
}
