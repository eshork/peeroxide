use clap::Parser;

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::inbox;
use crate::cmd::chat::known_users;
use crate::cmd::chat::profile;
use crate::cmd::{build_dht_config, sigterm_recv};
use crate::config::ResolvedConfig;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht::{self, KeyPair};

#[derive(Parser)]
pub struct InboxArgs {
    /// Identity profile to use
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Inbox polling interval in seconds
    #[arg(long, default_value = "15")]
    pub poll_interval: u64,

    /// Do not publish personal nexus
    #[arg(long)]
    pub no_nexus: bool,

    /// Do not refresh friend nexus data
    #[arg(long)]
    pub no_friends: bool,
}

pub async fn run(args: InboxArgs, cfg: &ResolvedConfig) -> i32 {
    let prof = match profile::load_or_create_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to load profile '{}': {e}", args.profile);
            return 1;
        }
    };

    let id_keypair = KeyPair::from_seed(prof.seed);

    let dht_config = build_dht_config(cfg);
    let runtime = match UdxRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to create UDP runtime: {e}");
            return 1;
        }
    };

    let (task, handle, _server_rx) = match hyperdht::spawn(&runtime, dht_config).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: failed to start DHT: {e}");
            return 1;
        }
    };

    if let Err(e) = handle.bootstrapped().await {
        eprintln!("error: bootstrap failed: {e}");
        return 1;
    }

    let table_size = handle.table_size().await.unwrap_or(0);
    eprintln!("*** connection established with DHT ({table_size} peers in routing table)");
    eprintln!("*** monitoring inbox (polling every {}s)", args.poll_interval);

    let poll_interval = tokio::time::Duration::from_secs(args.poll_interval);
    let mut seen_invite_feeds: std::collections::HashMap<[u8; 32], u64> =
        std::collections::HashMap::new();
    let mut invite_count = 0u32;

    let cached_users: Vec<profile::KnownUser> = known_users::load_shared_users()
        .unwrap_or_default()
        .into_iter()
        .map(|u| profile::KnownUser {
            pubkey: u.pubkey,
            screen_name: u.screen_name,
        })
        .collect();

    let mut interval = tokio::time::interval(poll_interval);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let current_epoch = crypto::current_epoch();
                for epoch in [current_epoch, current_epoch.saturating_sub(1)] {
                    for bucket in 0..4u8 {
                        let topic = crypto::inbox_topic(&id_keypair.public_key, epoch, bucket);
                        if let Ok(results) = handle.lookup(topic).await {
                            let peer_count: usize = results.iter().map(|r| r.peers.len()).sum();
                            debug::log_event(
                                "Inbox check",
                                "lookup",
                                &format!(
                                    "epoch={epoch}, bucket={bucket}, results={peer_count}",
                                ),
                            );
                            for result in &results {
                                for peer in &result.peers {
                                    let feed_pk = peer.public_key;
                                    let prev_seq = seen_invite_feeds.get(&feed_pk).copied();
                                    if let Ok(Some(mget)) = handle.mutable_get(&feed_pk, 0).await {
                                        let dominated = match prev_seq {
                                            Some(s) => mget.seq <= s,
                                            None => false,
                                        };
                                        if dominated {
                                            continue;
                                        }
                                        if let Ok(invite) = inbox::decrypt_and_verify_invite(
                                            &mget.value,
                                            &feed_pk,
                                            &id_keypair,
                                        ) {
                                            seen_invite_feeds.insert(feed_pk, mget.seq);
                                            invite_count += 1;
                                            debug::log_event(
                                                "Invite received",
                                                "mutable_get",
                                                &format!(
                                                    "invite_feed_pk={}, sender={}, invite_type=0x{:02x}, payload_len={}",
                                                    debug::short_key(&feed_pk),
                                                    debug::short_key(&invite.sender_pubkey),
                                                    invite.invite_type,
                                                    invite.payload.len(),
                                                ),
                                            );
                                            inbox::display_invite(
                                                invite_count,
                                                &invite,
                                                &id_keypair.public_key,
                                                &args.profile,
                                                &cached_users,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n*** shutting down");
                break;
            }
            _ = sigterm_recv() => {
                eprintln!("\n*** shutting down (SIGTERM)");
                break;
            }
        }
    }

    let _ = handle.destroy().await;
    let _ = task.await;
    0
}
