use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};
use rand::Rng;

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::known_users::KnownUser;
use crate::cmd::chat::wire::{self, InviteRecord, INVITE_TYPE_DM};

pub async fn send_dm_invite(
    handle: &HyperDhtHandle,
    invite_feed_keypair: &KeyPair,
    id_keypair: &KeyPair,
    recipient_pubkey: &[u8; 32],
    channel_key: &[u8; 32],
    real_feed_pubkey: &[u8; 32],
    message: &str,
) -> Result<(), String> {
    let ownership = crypto::ownership_proof(
        &id_keypair.secret_key,
        &invite_feed_keypair.public_key,
        channel_key,
    );

    let invite = InviteRecord {
        id_pubkey: id_keypair.public_key,
        ownership_proof: ownership,
        next_feed_pubkey: *real_feed_pubkey,
        invite_type: INVITE_TYPE_DM,
        payload: message.as_bytes().to_vec(),
    };

    let plaintext = invite.serialize().map_err(|e| format!("invite serialize: {e}"))?;

    let invite_x25519_priv = crypto::ed25519_secret_to_x25519(&invite_feed_keypair.secret_key);
    let recipient_x25519 = crypto::ed25519_pubkey_to_x25519(recipient_pubkey)
        .ok_or_else(|| "invalid recipient pubkey".to_string())?;
    let ecdh_secret = crypto::x25519_ecdh(&invite_x25519_priv, &recipient_x25519);
    let inv_key = crypto::invite_key(&ecdh_secret, &invite_feed_keypair.public_key);

    let encrypted = wire::encrypt_invite(&inv_key, &plaintext)
        .map_err(|e| format!("invite encrypt: {e}"))?;

    handle
        .mutable_put(invite_feed_keypair, &encrypted, 0)
        .await
        .map_err(|e| format!("invite mutable_put: {e}"))?;

    debug::log_event(
        "Invite sent",
        "mutable_put",
        &format!(
            "invite_feed_pk={}, sender={}, recipient={}, invite_type=0x{:02x}, payload_len={}",
            debug::short_key(&invite_feed_keypair.public_key),
            debug::short_key(&id_keypair.public_key),
            debug::short_key(recipient_pubkey),
            INVITE_TYPE_DM,
            message.len(),
        ),
    );

    let epoch = crypto::current_epoch();
    let bucket = rand::rng().random_range(0..4u8);
    let topic = crypto::inbox_topic(recipient_pubkey, epoch, bucket);
    let _ = handle.announce(topic, invite_feed_keypair, &[]).await;

    debug::log_event(
        "Inbox announce",
        "announce",
        &format!(
            "invite_feed_pk={}, recipient={}, epoch={epoch}, bucket={bucket}",
            debug::short_key(&invite_feed_keypair.public_key),
            debug::short_key(recipient_pubkey),
        ),
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn send_dm_nudge(
    handle: &HyperDhtHandle,
    invite_feed_keypair: &KeyPair,
    id_keypair: &KeyPair,
    recipient_pubkey: &[u8; 32],
    channel_key: &[u8; 32],
    real_feed_pubkey: &[u8; 32],
    message_text: &str,
    seq: u64,
) -> Result<(), String> {
    let ownership = crypto::ownership_proof(
        &id_keypair.secret_key,
        &invite_feed_keypair.public_key,
        channel_key,
    );

    let payload = if message_text.len() > 800 {
        message_text.as_bytes()[..800].to_vec()
    } else {
        message_text.as_bytes().to_vec()
    };

    let invite = InviteRecord {
        id_pubkey: id_keypair.public_key,
        ownership_proof: ownership,
        next_feed_pubkey: *real_feed_pubkey,
        invite_type: INVITE_TYPE_DM,
        payload,
    };

    let plaintext = invite.serialize().map_err(|e| format!("nudge serialize: {e}"))?;

    let invite_x25519_priv = crypto::ed25519_secret_to_x25519(&invite_feed_keypair.secret_key);
    let recipient_x25519 = crypto::ed25519_pubkey_to_x25519(recipient_pubkey)
        .ok_or_else(|| "invalid recipient pubkey".to_string())?;
    let ecdh_secret = crypto::x25519_ecdh(&invite_x25519_priv, &recipient_x25519);
    let inv_key = crypto::invite_key(&ecdh_secret, &invite_feed_keypair.public_key);

    let encrypted = wire::encrypt_invite(&inv_key, &plaintext)
        .map_err(|e| format!("nudge encrypt: {e}"))?;

    handle
        .mutable_put(invite_feed_keypair, &encrypted, seq + 1)
        .await
        .map_err(|e| format!("nudge mutable_put: {e}"))?;

    debug::log_event(
        "Inbox nudge sent",
        "mutable_put",
        &format!(
            "invite_feed_pk={}, sender={}, recipient={}, seq={}",
            debug::short_key(&invite_feed_keypair.public_key),
            debug::short_key(&id_keypair.public_key),
            debug::short_key(recipient_pubkey),
            seq + 1,
        ),
    );

    let epoch = crypto::current_epoch();
    let bucket = rand::rng().random_range(0..4u8);
    let topic = crypto::inbox_topic(recipient_pubkey, epoch, bucket);
    let _ = handle.announce(topic, invite_feed_keypair, &[]).await;

    debug::log_event(
        "Inbox announce",
        "announce",
        &format!(
            "invite_feed_pk={}, recipient={}, epoch={epoch}, bucket={bucket}",
            debug::short_key(&invite_feed_keypair.public_key),
            debug::short_key(recipient_pubkey),
        ),
    );

    Ok(())
}

pub struct DecodedInvite {
    pub sender_pubkey: [u8; 32],
    pub next_feed_pubkey: [u8; 32],
    pub invite_type: u8,
    pub payload: Vec<u8>,
}

impl std::fmt::Debug for DecodedInvite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedInvite")
            .field("sender_pubkey", &hex::encode(self.sender_pubkey))
            .field("invite_type", &format_args!("0x{:02x}", self.invite_type))
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl Clone for DecodedInvite {
    fn clone(&self) -> Self {
        Self {
            sender_pubkey: self.sender_pubkey,
            next_feed_pubkey: self.next_feed_pubkey,
            invite_type: self.invite_type,
            payload: self.payload.clone(),
        }
    }
}

pub fn decrypt_and_verify_invite(
    encrypted_data: &[u8],
    invite_feed_pubkey: &[u8; 32],
    my_keypair: &KeyPair,
) -> Result<DecodedInvite, String> {
    let invite_x25519_pub = crypto::ed25519_pubkey_to_x25519(invite_feed_pubkey)
        .ok_or_else(|| "invalid invite feed pubkey".to_string())?;
    let my_x25519_priv = crypto::ed25519_secret_to_x25519(&my_keypair.secret_key);
    let ecdh_secret = crypto::x25519_ecdh(&my_x25519_priv, &invite_x25519_pub);
    let inv_key = crypto::invite_key(&ecdh_secret, invite_feed_pubkey);

    let plaintext =
        wire::decrypt_invite(&inv_key, encrypted_data).map_err(|e| format!("decrypt: {e}"))?;

    let record =
        InviteRecord::deserialize(&plaintext).map_err(|e| format!("parse invite: {e}"))?;

    let candidate_dm_key =
        crypto::dm_channel_key(&record.id_pubkey, &my_keypair.public_key);
    if crypto::verify_ownership_proof(
        &record.id_pubkey,
        invite_feed_pubkey,
        &candidate_dm_key,
        &record.ownership_proof,
    ) {
        return Ok(DecodedInvite {
            sender_pubkey: record.id_pubkey,
            next_feed_pubkey: record.next_feed_pubkey,
            invite_type: record.invite_type,
            payload: record.payload,
        });
    }

    if record.invite_type == wire::INVITE_TYPE_PRIVATE && record.payload.len() >= 3 {
        let name_len = record.payload[0] as usize;
        if record.payload.len() >= 1 + name_len + 2 {
            let name = &record.payload[1..1 + name_len];
            let salt_len =
                u16::from_le_bytes([record.payload[1 + name_len], record.payload[2 + name_len]])
                    as usize;
            if record.payload.len() >= 3 + name_len + salt_len {
                let salt = &record.payload[3 + name_len..3 + name_len + salt_len];
                let candidate_key = crypto::channel_key(name, Some(salt));
                if crypto::verify_ownership_proof(
                    &record.id_pubkey,
                    invite_feed_pubkey,
                    &candidate_key,
                    &record.ownership_proof,
                ) {
                    return Ok(DecodedInvite {
                        sender_pubkey: record.id_pubkey,
                        next_feed_pubkey: record.next_feed_pubkey,
                        invite_type: record.invite_type,
                        payload: record.payload,
                    });
                }
            }
        }
    }

    Err("ownership proof verification failed".to_string())
}

pub fn display_invite(
    number: u32,
    invite: &DecodedInvite,
    _my_pubkey: &[u8; 32],
    profile_name: &str,
    known_users: &[KnownUser],
) {
    let sender_hex = hex::encode(invite.sender_pubkey);
    let short = &sender_hex[..8];

    let sender_name = known_users
        .iter()
        .find(|u| u.pubkey == invite.sender_pubkey)
        .map(|u| u.screen_name.as_str())
        .unwrap_or(short);

    if invite.invite_type == INVITE_TYPE_DM {
        let lure = String::from_utf8_lossy(&invite.payload);
        println!("[INVITE #{number}] DM from {sender_name} ({short})");
        if !lure.is_empty() {
            println!("  \"{lure}\"");
        }
        println!("  → peeroxide chat dm {sender_hex} --profile {profile_name}");
    } else {
        if invite.payload.len() >= 3 {
            let name_len = invite.payload[0] as usize;
            if invite.payload.len() >= 1 + name_len + 2 {
                let name = String::from_utf8_lossy(&invite.payload[1..1 + name_len]);
                let salt_len = u16::from_le_bytes([
                    invite.payload[1 + name_len],
                    invite.payload[2 + name_len],
                ]) as usize;
                if invite.payload.len() >= 3 + name_len + salt_len {
                    let salt =
                        String::from_utf8_lossy(&invite.payload[3 + name_len..3 + name_len + salt_len]);
                    println!(
                        "[INVITE #{number}] Channel \"{name}\" from {sender_name} ({short})"
                    );
                    println!(
                        "  → peeroxide chat join \"{name}\" --group \"{salt}\" --profile {profile_name}"
                    );
                    return;
                }
            }
        }
        println!("[INVITE #{number}] Channel invite from {sender_name} ({short})");
    }
}
