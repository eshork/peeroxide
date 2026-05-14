use clap::Parser;

use crate::cmd::chat::crypto;
use crate::cmd::chat::known_users;
use crate::cmd::chat::name_resolver::NameResolver;
use crate::cmd::chat::profile;
use crate::cmd::chat::session::{self, DmExtras, SessionConfig};
use crate::config::ResolvedConfig;

use peeroxide_dht::hyperdht::KeyPair;

#[derive(Parser)]
pub struct DmArgs {
    /// Recipient: alias, pubkey hex (64 chars), @shortkey, name@shortkey, or screen name
    pub recipient: String,

    /// Identity profile to use
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Do not publish personal nexus
    #[arg(long)]
    pub no_nexus: bool,

    /// Do not refresh friend nexus data
    #[arg(long)]
    pub no_friends: bool,

    /// Listen only
    #[arg(long)]
    pub read_only: bool,

    /// Equivalent to --no-nexus --read-only --no-friends
    #[arg(long)]
    pub stealth: bool,

    /// Message to include in the startup inbox nudge
    #[arg(long)]
    pub message: Option<String>,

    /// Max feed keypair lifetime before rotation (minutes)
    #[arg(long, default_value = "60")]
    pub feed_lifetime: u64,

    /// Max messages to publish in a single batch.
    #[arg(long, default_value = "16")]
    pub batch_size: usize,

    /// Idle time (ms) the publisher waits to accumulate additional
    /// messages into the current batch before flushing.
    #[arg(long, default_value = "50")]
    pub batch_wait_ms: u64,

    /// After stdin closes (EOF), remain joined to the channel in
    /// read-only mode instead of exiting. Default is to exit cleanly
    /// once stdin is exhausted.
    #[arg(long)]
    pub stay_after_eof: bool,

    /// Disable the background inbox monitor + INBOX status bar segment
    /// + /inbox slash command. Default is enabled.
    #[arg(long)]
    pub no_inbox: bool,

    /// Inbox polling interval in seconds.
    #[arg(long, default_value = "15")]
    pub inbox_poll_interval: u64,
}

pub async fn run(args: DmArgs, cfg: &ResolvedConfig, line_mode: bool) -> i32 {
    let read_only = args.read_only || args.stealth;
    let no_nexus = args.no_nexus || args.stealth;
    let no_friends = args.no_friends || args.stealth;

    let recipient_pubkey = match super::resolve_recipient(&args.profile, &args.recipient) {
        Ok(pk) => pk,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let prof = match profile::load_or_create_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to load profile '{}': {e}", args.profile);
            return 1;
        }
    };

    let id_keypair = KeyPair::from_seed(prof.seed);

    // Channel + message keys for a DM are derived deterministically from
    // the two identity pubkeys (channel) plus the X25519-ECDH shared
    // secret (message). The session is then a normal "private channel"
    // with these specialized keys; everything downstream
    // (announce topics, feed records, encryption, ordering) treats them
    // identically to a non-DM private channel.
    let channel_key = crypto::dm_channel_key(&id_keypair.public_key, &recipient_pubkey);
    let ecdh_secret = {
        let my_x25519 = crypto::ed25519_secret_to_x25519(&id_keypair.secret_key);
        let Some(their_x25519) = crypto::ed25519_pubkey_to_x25519(&recipient_pubkey) else {
            eprintln!("error: invalid recipient public key (cannot convert to X25519)");
            return 1;
        };
        crypto::x25519_ecdh(&my_x25519, &their_x25519)
    };
    let message_key = crypto::dm_msg_key(&ecdh_secret, &channel_key);

    // Resolve the recipient's display name for the bar / greeting via
    // the canonical name resolver (friend alias > known screen name >
    // vendor name fallback).
    let friends = profile::load_friends(&args.profile).unwrap_or_default();
    let known = known_users::load_shared_users().unwrap_or_default();
    let resolved = NameResolver::new(&friends, &known).resolve(&recipient_pubkey);

    let bar_name = format!("DM:{}", resolved.bar_label());
    let greeting = format!("*** DM with {}", resolved.formal());

    let config = SessionConfig {
        bar_name,
        greeting,
        channel_key,
        message_key,
        profile: args.profile,
        prof,
        id_keypair,
        read_only,
        no_nexus,
        no_friends,
        no_inbox: args.no_inbox,
        feed_lifetime: args.feed_lifetime,
        batch_size: args.batch_size,
        batch_wait_ms: args.batch_wait_ms,
        inbox_poll_interval: args.inbox_poll_interval,
        stay_after_eof: args.stay_after_eof,
        line_mode,
        dm: Some(DmExtras {
            recipient_pubkey,
            initial_message: args.message,
        }),
    };

    session::run(config, cfg).await
}
