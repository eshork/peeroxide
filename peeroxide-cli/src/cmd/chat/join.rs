use clap::Parser;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cmd::chat::crypto;
use crate::cmd::chat::display;
use crate::cmd::chat::known_users::SharedKnownUsers;
use crate::cmd::chat::feed;
use crate::cmd::chat::probe;
use crate::cmd::chat::profile;
use crate::cmd::chat::publisher::{self, PubJob};
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

    let feed_state = feed_keypair.as_ref().map(|fkp| {
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
    let mut display_state = display::DisplayState::new(friends, SharedKnownUsers::load_from_shared());

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

    // --- Publisher worker + stdin reader (only when posting is enabled) ---
    let mut pub_tx: Option<mpsc::Sender<PubJob>> = None;
    let mut publisher_handle: Option<JoinHandle<()>> = None;
    let mut stdin_handle: Option<JoinHandle<()>> = None;
    let mut stdin_eof_rx: Option<tokio::sync::oneshot::Receiver<()>> = None;

    if let Some(fs) = feed_state {
        let (tx, rx) = mpsc::channel::<PubJob>(64);
        pub_tx = Some(tx.clone());

        let screen_name = prof.screen_name.clone().unwrap_or_default();
        let handle_pub = handle.clone();
        let id_kp = id_keypair.clone();
        let batch_size = args.batch_size;
        let batch_wait_ms = args.batch_wait_ms;
        publisher_handle = Some(tokio::spawn(async move {
            publisher::run_publisher(
                handle_pub,
                fs,
                id_kp,
                message_key,
                channel_key,
                screen_name,
                rx,
                batch_size,
                batch_wait_ms,
            )
            .await;
        }));

        // stdin → publisher channel. send().await applies natural backpressure
        // when the publisher cannot keep up. The oneshot signals the main loop
        // on EOF so it can choose whether to exit (default) or remain joined.
        let (eof_tx, eof_rx) = tokio::sync::oneshot::channel::<()>();
        stdin_eof_rx = Some(eof_rx);
        stdin_handle = Some(tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut lines = BufReader::new(stdin).lines();
            let mut stdin_counter: u64 = 0;
            loop {
                match lines.next_line().await {
                    Ok(Some(text)) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        stdin_counter += 1;
                        if probe::is_enabled() {
                            let preview: String = text.chars().take(40).collect();
                            eprintln!("[probe] stdin#{stdin_counter} read={preview:?}");
                        }
                        if tx.send(PubJob::Message(text)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("error reading stdin: {e}");
                        break;
                    }
                }
            }
            // Drop tx so the publisher's rx.recv() returns None once it has
            // drained any in-flight job. Notify the main loop of EOF so it
            // can apply the --stay-after-eof policy.
            drop(tx);
            let _ = eof_tx.send(());
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

    let mut backlog_done = false;
    let friends_reload_interval = tokio::time::Duration::from_secs(30);
    let mut friends_reload_tick = tokio::time::interval(friends_reload_interval);
    let mut eof_handled = false;
    // True when we exit the loop because stdin reached EOF and the user
    // did NOT pass --stay-after-eof. In that case the publish queue must be
    // fully drained before we return; aborting mid-batch would leave the
    // user's messages un-published. False on Ctrl-C / SIGTERM, where the
    // user has explicitly asked to stop and a short drain timeout suffices.
    let mut graceful_eof_exit = false;

    loop {
        tokio::select! {
            Some(msg) = msg_rx.recv() => {
                if !backlog_done && msg.content.is_empty() && msg.id_pubkey == [0u8; 32] && msg.timestamp == 0 {
                    backlog_done = true;
                    eprintln!("*** — live —");
                    continue;
                }
                display_state.render(&msg);
            }
            _ = friends_reload_tick.tick() => {
                if let Ok(updated_friends) = profile::load_friends(&args.profile) {
                    display_state.reload_friends(updated_friends);
                }
            }
            // Fires exactly once when the stdin task reports EOF. The guard
            // disables the arm after first delivery so the oneshot (which
            // returns Pending forever after consumption) is not re-polled.
            () = async {
                if let Some(rx) = stdin_eof_rx.as_mut() {
                    let _ = rx.await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if !eof_handled && stdin_eof_rx.is_some() => {
                eof_handled = true;
                stdin_eof_rx = None;
                if args.stay_after_eof {
                    eprintln!("*** stdin closed, entering read-only mode");
                    // continue running; reader + publisher (idle) stay alive
                } else {
                    graceful_eof_exit = true;
                    break;
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

    // Drop the publisher send half so the publisher's rx.recv() returns None
    // and the worker exits cleanly once it has drained its in-flight batch
    // and any queued jobs.
    drop(pub_tx);

    if let Some(h) = stdin_handle {
        h.abort();
    }
    if let Some(h) = publisher_handle {
        if graceful_eof_exit {
            // EOF-driven exit — the user piped a file and expects every line
            // to land on the wire. Wait for the queue to drain naturally.
            // A second Ctrl-C aborts in case of a stuck DHT.
            eprintln!("*** flushing publish queue (Ctrl-C to abort)…");
            tokio::select! {
                _ = h => {
                    eprintln!("*** publish queue flushed");
                }
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("\n*** abort: outgoing messages may not have reached the network");
                }
            }
        } else {
            // Interrupted exit (Ctrl-C / SIGTERM) — the user asked to stop.
            // Give the in-flight batch a short window to wrap up, then move on.
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(2), h).await;
        }
    }
    reader_handle.abort();
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
