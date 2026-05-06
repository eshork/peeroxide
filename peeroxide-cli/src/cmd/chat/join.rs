use clap::Parser;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::cmd::chat::crypto;
use crate::cmd::chat::debug;
use crate::cmd::chat::display;
use crate::cmd::chat::feed;
use crate::cmd::chat::post;
use crate::cmd::chat::profile;
use crate::cmd::chat::reader;
use crate::cmd::{build_dht_config, sigterm_recv};
use crate::config::ResolvedConfig;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht::{self, KeyPair};
use tokio::io::{AsyncBufReadExt, BufReader};

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
}

pub async fn run(args: JoinArgs, cfg: &ResolvedConfig) -> i32 {
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

    let feed_keypair = if !read_only {
        Some(KeyPair::generate())
    } else {
        None
    };

    let ownership_proof = feed_keypair.as_ref().map(|fkp| {
        crypto::ownership_proof(&id_keypair.secret_key, &fkp.public_key, &channel_key)
    });

    let mut feed_state = feed_keypair.as_ref().map(|fkp| {
        feed::FeedState::new(
            fkp.clone(),
            id_keypair.clone(),
            channel_key,
            ownership_proof.unwrap(),
            args.feed_lifetime,
        )
    });

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<display::DisplayMessage>();

    let friends = profile::load_friends(&args.profile).unwrap_or_default();
    let mut display_state = display::DisplayState::new(friends);

    eprintln!("*** joining channel '{}'", args.channel);

    let reader_handle = {
        let handle = handle.clone();
        let msg_tx = msg_tx.clone();
        let profile_name = args.profile.clone();
        let self_feed_pubkey = feed_keypair.as_ref().map(|fkp| fkp.public_key);
        let self_id = id_keypair.public_key;
        tokio::spawn(async move {
            reader::run_reader(
                handle,
                channel_key,
                message_key,
                msg_tx,
                profile_name,
                self_feed_pubkey,
                self_id,
            )
            .await;
        })
    };

    let mut feed_state_tx: Option<watch::Sender<(Vec<u8>, u64)>> = None;
    let mut feed_refresh_handle: Option<JoinHandle<()>> = None;

    if let Some(ref fs) = feed_state {
        let initial_data = fs.serialize_feed_record();
        if let Err(e) = handle.mutable_put(&fs.feed_keypair, &initial_data, fs.seq).await {
            eprintln!("warning: initial feed publish failed: {e}");
        }
        let (tx, rx) = watch::channel((initial_data, fs.seq));
        feed_state_tx = Some(tx);

        let h = handle.clone();
        let kp = fs.feed_keypair.clone();
        feed_refresh_handle = Some(tokio::spawn(async move {
                                    feed::run_feed_refresh(h, kp, rx).await;
        }));
    }

    let nexus_handle: Option<JoinHandle<()>> = if !no_nexus {
        let handle = handle.clone();
        let id_kp = id_keypair.clone();
        let profile_name = args.profile.clone();
        Some(tokio::spawn(async move {
            crate::cmd::chat::nexus::run_nexus_refresh(handle, id_kp, profile_name).await;
        }))
    } else {
        None
    };

    let friend_refresh_handle: Option<JoinHandle<()>> = if !no_friends {
        let handle = handle.clone();
        let profile_name = args.profile.clone();
        Some(tokio::spawn(async move {
            crate::cmd::chat::nexus::run_friend_refresh(handle, profile_name).await;
        }))
    } else {
        None
    };

    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();
    let mut stdin_closed = false;
    let mut backlog_done = false;

    let rotation_interval = tokio::time::Duration::from_secs(30);
    let mut rotation_check = tokio::time::interval(rotation_interval);
    let friends_reload_interval = tokio::time::Duration::from_secs(30);
    let mut friends_reload_tick = tokio::time::interval(friends_reload_interval);

    loop {
        tokio::select! {
            line = stdin_reader.next_line(), if !stdin_closed && !read_only => {
                match line {
                    Ok(Some(text)) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        if let Some(ref mut fs) = feed_state {
                            let screen_name = prof.screen_name.clone().unwrap_or_default();
                            if let Err(e) = post::post_message(
                                &handle,
                                fs,
                                &id_keypair,
                                &message_key,
                                &channel_key,
                                &screen_name,
                                &text,
                            ) {
                                eprintln!("error: failed to post: {e}");
                            } else if let Some(ref tx) = feed_state_tx {
                                let _ = tx.send((fs.serialize_feed_record(), fs.seq));
                            }
                        }
                    }
                    Ok(None) => {
                        stdin_closed = true;
                        eprintln!("*** stdin closed, entering read-only mode");
                    }
                    Err(e) => {
                        eprintln!("error reading stdin: {e}");
                        stdin_closed = true;
                    }
                }
            }
            Some(msg) = msg_rx.recv() => {
                if !backlog_done && msg.content.is_empty() && msg.id_pubkey == [0u8; 32] && msg.timestamp == 0 {
                    backlog_done = true;
                    eprintln!("*** — live —");
                    continue;
                }
                display_state.render(&msg);
            }
            _ = rotation_check.tick(), if feed_state.is_some() => {
                if let Some(ref mut fs) = feed_state {
                    if fs.needs_rotation() {
                        let mut new_fs = fs.rotate();

                        // Target-before-pointer: publish NEW feed first so readers
                        // can resolve it, THEN update old feed to point at it.
                        let new_data = new_fs.serialize_feed_record();
                        match handle.mutable_put(&new_fs.feed_keypair, &new_data, new_fs.seq).await {
                            Ok(_) => {
                                debug::log_event(
                                    "Feed rotation (new)",
                                    "mutable_put",
                                    &format!(
                                        "new_feed_pubkey={}, old_feed_pubkey={}",
                                        debug::short_key(&new_fs.feed_keypair.public_key),
                                        debug::short_key(&fs.feed_keypair.public_key),
                                    ),
                                );

                                // New feed is live; now update old feed with next_feed_pubkey pointer
                                let old_record = fs.serialize_feed_record();
                                fs.seq += 1;
                                if let Err(e) = handle.mutable_put(&fs.feed_keypair, &old_record, fs.seq).await {
                                    tracing::warn!("rotation: old feed update failed (non-fatal): {e}");
                                } else {
                                    debug::log_event(
                                        "Feed rotation (old ptr)",
                                        "mutable_put",
                                        &format!(
                                            "old_feed_pubkey={}, seq={}, next_feed={}",
                                            debug::short_key(&fs.feed_keypair.public_key),
                                            fs.seq,
                                            debug::short_key(&new_fs.feed_keypair.public_key),
                                        ),
                                    );
                                }

                                if let Some(h) = feed_refresh_handle.take() {
                                    h.abort();
                                }

                                let overlap_h = handle.clone();
                                let overlap_kp = fs.feed_keypair.clone();
                                let overlap_data = old_record.clone();
                                let overlap_seq = fs.seq;
                                tokio::spawn(async move {
                                    feed::run_rotation_overlap_refresh(
                                        overlap_h, overlap_kp, overlap_data, overlap_seq,
                                    ).await;
                                });

                                let (tx, rx) = watch::channel((new_data, new_fs.seq));
                                feed_state_tx = Some(tx);

                                let h = handle.clone();
                                let kp = new_fs.feed_keypair.clone();
                                feed_refresh_handle = Some(tokio::spawn(async move {
            feed::run_feed_refresh(h, kp, rx).await;
        }));


                                std::mem::swap(fs, &mut new_fs);
                                eprintln!("*** feed keypair rotated");
                            }
                            Err(e) => {
                                eprintln!("warning: feed rotation failed (new feed publish), will retry: {e}");
                                fs.next_feed_pubkey = [0u8; 32];
                            }
                        }
                    }
                }
            }
            _ = friends_reload_tick.tick() => {
                if let Ok(updated_friends) = profile::load_friends(&args.profile) {
                    display_state.reload_friends(updated_friends);
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

    reader_handle.abort();
    if let Some(h) = feed_refresh_handle {
        h.abort();
    }
    if let Some(h) = nexus_handle {
        h.abort();
    }
    if let Some(h) = friend_refresh_handle {
        h.abort();
    }

    let _ = handle.destroy().await;
    let _ = task.await;
    0
}
