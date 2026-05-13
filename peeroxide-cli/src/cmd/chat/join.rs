use clap::Parser;

use crate::cmd::chat::crypto;
use crate::cmd::chat::profile;
use crate::cmd::chat::session::{self, SessionConfig};
use crate::config::ResolvedConfig;

use peeroxide_dht::hyperdht::KeyPair;

#[derive(Parser)]
pub struct JoinArgs {
    /// Channel name
    pub channel: String,

    /// Private channel with group name as salt
    #[arg(long, conflicts_with = "keyfile")]
    pub group: Option<String>,

    /// Private channel with keyfile as salt
    #[arg(long, conflicts_with = "group")]
    pub keyfile: Option<String>,

    /// Identity profile to use
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Do not publish personal nexus
    #[arg(long)]
    pub no_nexus: bool,

    /// Do not refresh friend nexus data
    #[arg(long)]
    pub no_friends: bool,

    /// Listen only; no posting, no feed, no announce
    #[arg(long)]
    pub read_only: bool,

    /// Equivalent to --no-nexus --read-only --no-friends
    #[arg(long)]
    pub stealth: bool,

    /// Max feed keypair lifetime before rotation (minutes)
    #[arg(long, default_value = "60")]
    pub feed_lifetime: u64,

    /// Max messages to publish in a single batch.
    /// Each batch performs one mutable_put + one announce regardless of
    /// message count, so larger batches amortize DHT round-trips when
    /// piping a file. Capped well below the 26-hash FeedRecord window.
    #[arg(long, default_value = "16")]
    pub batch_size: usize,

    /// Idle time (ms) the publisher waits to accumulate additional
    /// messages into the current batch before flushing. Interactive
    /// single messages flush after this delay; piped streams typically
    /// fill the batch sooner.
    #[arg(long, default_value = "50")]
    pub batch_wait_ms: u64,

    /// After stdin closes (EOF), remain joined to the channel in read-only
    /// mode instead of exiting. Useful when a script pipes a burst of
    /// messages and the operator wants to keep watching the channel
    /// afterward. Default is to exit cleanly once stdin is exhausted, which
    /// matches the natural shell-pipe lifecycle (`file | peeroxide chat join`
    /// finishes when the file does).
    #[arg(long)]
    pub stay_after_eof: bool,

    /// Disable the background inbox monitor. By default `chat join` polls
    /// the same inbox topics as `chat inbox` so an INBOX indicator can
    /// surface on the status bar; `/inbox` then dumps the unread invites
    /// to the chat region. When disabled, the inbox segment is omitted
    /// from the bar entirely and `/inbox` is a no-op.
    #[arg(long)]
    pub no_inbox: bool,

    /// Inbox polling interval in seconds. Matches the chat inbox CLI
    /// default; the chat protocol docs (`docs/src/chat/protocol.md`) suggest 15-30 s.
    #[arg(long, default_value = "15")]
    pub inbox_poll_interval: u64,
}

pub async fn run(args: JoinArgs, cfg: &ResolvedConfig, line_mode: bool) -> i32 {
    let read_only = args.read_only || args.stealth;
    let no_nexus = args.no_nexus || args.stealth;
    let no_friends = args.no_friends || args.stealth;

    let prof = match profile::load_or_create_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to load profile '{}': {e}", args.profile);
            return 1;
        }
    };

    let id_keypair = KeyPair::from_seed(prof.seed);

    let salt = if let Some(ref group) = args.group {
        Some(group.as_bytes().to_vec())
    } else if let Some(ref keyfile_path) = args.keyfile {
        match std::fs::read(keyfile_path) {
            Ok(data) => Some(data),
            Err(e) => {
                eprintln!("error: failed to read keyfile '{keyfile_path}': {e}");
                return 1;
            }
        }
    } else {
        None
    };

    let channel_key = crypto::channel_key(args.channel.as_bytes(), salt.as_deref());
    let message_key = crypto::msg_key(&channel_key);

    let bar_name = args.channel.clone();
    let greeting = format!("*** joining channel '{}'", args.channel);

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
        dm: None,
    };

    session::run(config, cfg).await
}
