use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cmd::chat::crypto;
use crate::cmd::chat::display;
use crate::cmd::chat::feed;
use crate::cmd::chat::known_users::SharedKnownUsers;
use crate::cmd::chat::profile;
use crate::cmd::chat::publisher::{self, PubJob};
use crate::cmd::chat::reader;
use crate::cmd::chat::tui::{
    self, ChatUi, IgnoreSet, NoticeSink, SlashCommand, StatusState, UiInput, UiOptions, commands,
};
use crate::cmd::{build_dht_config, sigterm_recv};
use crate::config::ResolvedConfig;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht::{self, KeyPair};

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

    let dht_config = build_dht_config(cfg);
    let runtime = match UdxRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to create UDP runtime: {e}");
            return 1;
        }
    };

    // --- ChatUi construction ---
    //
    // Constructed BEFORE the DHT handshake so that all subsequent startup
    // notices ("waiting for bootstrap...", "connection established...") flow
    // through the UI in proper layout instead of landing wherever the cursor
    // happens to be. The factory inspects stdout's TTY status and the
    // `line_mode` opt-out flag to pick `LineUi` (today's behaviour) or
    // `InteractiveUi` (TTY-aware status bar + multi-line input).
    let ui_opts = UiOptions {
        force_line_mode: line_mode,
        channel_name: args.channel.clone(),
        profile_name: args.profile.clone(),
    };
    let mut ui: Box<dyn ChatUi> = tui::make_ui(ui_opts);
    let status: Arc<StatusState> = ui.status();
    let ignore: IgnoreSet = ui.ignore_set();

    // Set up the process-wide notice channel. Background helpers (publisher,
    // reader, post.rs probe traces, nexus refresh, feed rotation) push
    // system-notice lines through this; the main loop drains the receiver
    // below and forwards each line through `ui.render_system`.
    let (notice_tx, mut notice_rx) = NoticeSink::new();
    tui::install_global_notice_sink(notice_tx.clone());

    let (task, handle, _server_rx) = match hyperdht::spawn(&runtime, dht_config).await {
        Ok(v) => v,
        Err(e) => {
            ui.render_system(&format!("error: failed to start DHT: {e}"));
            return 1;
        }
    };

    if let Err(e) = handle.bootstrapped().await {
        ui.render_system(&format!("error: bootstrap failed: {e}"));
        return 1;
    }

    let table_size = handle.table_size().await.unwrap_or(0);
    ui.render_system(&format!(
        "*** connection established with DHT ({table_size} peers in routing table)"
    ));

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

    // Set initial DHT peer count snapshot so the bar isn't empty before the
    // poller's first tick arrives.
    status.set_dht_peers(table_size);

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<display::DisplayMessage>();

    let friends = profile::load_friends(&args.profile).unwrap_or_default();
    let mut display_state =
        display::DisplayState::new(friends, SharedKnownUsers::load_from_shared());

    ui.render_system(&format!("*** joining channel '{}'", args.channel));

    let reader_handle = {
        let handle = handle.clone();
        let msg_tx = msg_tx.clone();
        let profile_name = args.profile.clone();
        let self_feed_pubkey = feed_keypair.as_ref().map(|fkp| fkp.public_key);
        let self_id = id_keypair.public_key;
        let status = status.clone();
        tokio::spawn(async move {
            reader::run_reader(
                handle,
                channel_key,
                message_key,
                msg_tx,
                profile_name,
                self_feed_pubkey,
                self_id,
                status,
            )
            .await;
        })
    };

    // --- Publisher worker (only when posting is enabled) ---
    //
    // Note: the historical stdin BufReader task is gone — every input event
    // (chat messages, slash commands, EOF, Ctrl-C) now arrives through
    // `ui.next_input()`. Messages with no publisher (read-only mode) are
    // surfaced to the user as a system notice and silently dropped.
    let mut pub_tx: Option<mpsc::Sender<PubJob>> = None;
    let mut publisher_handle: Option<JoinHandle<()>> = None;

    if let Some(fs) = feed_state {
        let (tx, rx) = mpsc::channel::<PubJob>(64);
        pub_tx = Some(tx);

        let screen_name = prof.screen_name.clone().unwrap_or_default();
        let handle_pub = handle.clone();
        let id_kp = id_keypair.clone();
        let batch_size = args.batch_size;
        let batch_wait_ms = args.batch_wait_ms;
        let status_pub = status.clone();
        let notices_pub = notice_tx.clone();
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
                status_pub,
                notices_pub,
            )
            .await;
        }));
    }

    let nexus_handle: Option<JoinHandle<()>> = if !no_nexus {
        let handle = handle.clone();
        let id_kp = id_keypair.clone();
        let profile_name = args.profile.clone();
        let notices = notice_tx.clone();
        Some(tokio::spawn(async move {
            crate::cmd::chat::nexus::run_nexus_refresh(handle, id_kp, profile_name, notices).await;
        }))
    } else {
        None
    };

    let friend_refresh_handle: Option<JoinHandle<()>> = if !no_friends {
        let handle = handle.clone();
        let profile_name = args.profile.clone();
        let notices = notice_tx.clone();
        Some(tokio::spawn(async move {
            crate::cmd::chat::nexus::run_friend_refresh(handle, profile_name, notices).await;
        }))
    } else {
        None
    };

    // Periodically poll the DHT table size into the status bar.
    let dht_status_handle: JoinHandle<()> = {
        let handle = handle.clone();
        let status = status.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Burn the immediate first tick (we already populated the initial
            // value above).
            tick.tick().await;
            loop {
                tick.tick().await;
                let n = handle.table_size().await.unwrap_or(0);
                status.set_dht_peers(n);
            }
        })
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
    let mut want_exit = false;

    loop {
        tokio::select! {
            // Drain background system notices first so they reach the UI in
            // order with anything that happens in this iteration. The biased
            // hint isn't strictly needed (each arm is independent) but
            // putting it first keeps the read order predictable for the
            // reader of this code.
            Some(line) = notice_rx.recv() => {
                ui.render_system(&line);
            }
            Some(msg) = msg_rx.recv() => {
                if !backlog_done && msg.content.is_empty() && msg.id_pubkey == [0u8; 32] && msg.timestamp == 0 {
                    backlog_done = true;
                    ui.render_system("*** — live —");
                    continue;
                }
                // Skip messages from ignored users. Read-lock is cheap; the
                // hot path here is "set is empty" which is constant-time.
                let ignored = {
                    let g = ignore.read().await;
                    !g.is_empty() && g.contains(&msg.id_pubkey)
                };
                if ignored {
                    continue;
                }
                let out = display_state.render_to(&msg);
                for notice in &out.system_notices {
                    ui.render_system(notice);
                }
                ui.render_message(&msg);
                // Note: render_message uses the formatted line we already
                // constructed in `out.message_line`, but `ChatUi::render_message`
                // takes the structured `DisplayMessage` so each UI impl can
                // pick its own formatting (line mode prints the line as-is;
                // the interactive UI can colour, prepend cursor moves, etc.).
                // We discard `out.message_line` here intentionally — both
                // implementations re-derive it via `render_message_line` /
                // their own formatting. The state mutations in `render_to`
                // are still important (cooldown tracking).
                let _ = out;
            }
            _ = friends_reload_tick.tick() => {
                if let Ok(updated_friends) = profile::load_friends(&args.profile) {
                    display_state.reload_friends(updated_friends);
                }
            }
            input = ui.next_input() => {
                match input {
                    Some(UiInput::Message(text)) => {
                        if let Some(tx) = pub_tx.as_ref() {
                            status.inc_send_pending();
                            if tx.send(PubJob::Message(text)).await.is_err() {
                                // Publisher dropped — abort send_pending bookkeeping.
                                status.dec_send_pending();
                            }
                        } else {
                            ui.render_system("*** read-only mode; message not sent");
                        }
                    }
                    Some(UiInput::Command(cmd)) => {
                        if dispatch_slash(
                            cmd,
                            &args.profile,
                            ui.as_ref(),
                            &ignore,
                        ).await {
                            // dispatch_slash returns true for /quit etc.
                            ui.render_system("*** shutting down");
                            break;
                        }
                    }
                    Some(UiInput::Eof) => {
                        if !eof_handled {
                            eof_handled = true;
                            if args.stay_after_eof {
                                ui.render_system("*** stdin closed, entering read-only mode");
                                // continue running; reader + publisher (idle) stay alive
                            } else {
                                graceful_eof_exit = true;
                                want_exit = true;
                            }
                        }
                    }
                    Some(UiInput::Interrupt) => {
                        ui.render_system("*** shutting down");
                        break;
                    }
                    None => {
                        // UI shut down on its own — treat as interrupt.
                        break;
                    }
                }
                if want_exit {
                    break;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                ui.render_system("*** shutting down");
                break;
            }
            _ = sigterm_recv() => {
                ui.render_system("*** shutting down (SIGTERM)");
                break;
            }
        }
    }

    // Drop the publisher send half so the publisher's rx.recv() returns None
    // and the worker exits cleanly once it has drained its in-flight batch
    // and any queued jobs.
    drop(pub_tx);

    if let Some(h) = publisher_handle {
        if graceful_eof_exit {
            // EOF-driven exit — the user piped a file and expects every line
            // to land on the wire. Wait for the queue to drain naturally.
            // A second Ctrl-C aborts in case of a stuck DHT.
            ui.render_system("*** flushing publish queue (Ctrl-C to abort)…");
            tokio::select! {
                _ = h => {
                    ui.render_system("*** publish queue flushed");
                }
                _ = tokio::signal::ctrl_c() => {
                    ui.render_system("*** abort: outgoing messages may not have reached the network");
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
    dht_status_handle.abort();

    // Restore terminal before final destroy so any error messages from the
    // shutdown sequence land in a clean cooked-mode terminal.
    ui.shutdown().await;

    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

/// Apply a slash command. Returns `true` if the session should exit.
async fn dispatch_slash(
    cmd: SlashCommand,
    profile_name: &str,
    ui: &dyn ChatUi,
    ignore: &IgnoreSet,
) -> bool {
    use crate::cmd::chat::resolve_recipient as resolve_pubkey;
    match cmd {
        SlashCommand::Quit => return true,
        SlashCommand::Help => {
            ui.render_system(commands::help_text());
        }
        SlashCommand::IgnoreList => {
            let g = ignore.read().await;
            if g.is_empty() {
                ui.render_system("*** ignore list is empty");
            } else {
                let mut lines = vec!["*** ignoring:".to_string()];
                for pk in g.iter() {
                    let short = &hex::encode(pk)[..8];
                    lines.push(format!("    {short}"));
                }
                ui.render_system(&lines.join("\n"));
            }
        }
        SlashCommand::Ignore(arg) => match resolve_pubkey(profile_name, &arg) {
            Ok(pk) => {
                ignore.write().await.insert(pk);
                ui.render_system(&format!("*** ignoring {}", &hex::encode(pk)[..8]));
            }
            Err(e) => ui.render_system(&format!("*** /ignore: {e}")),
        },
        SlashCommand::Unignore(arg) => match resolve_pubkey(profile_name, &arg) {
            Ok(pk) => {
                let removed = ignore.write().await.remove(&pk);
                if removed {
                    ui.render_system(&format!("*** unignored {}", &hex::encode(pk)[..8]));
                } else {
                    ui.render_system("*** not in ignore list");
                }
            }
            Err(e) => ui.render_system(&format!("*** /unignore: {e}")),
        },
        SlashCommand::FriendList => match profile::load_friends(profile_name) {
            Ok(friends) if friends.is_empty() => ui.render_system("*** no friends"),
            Ok(friends) => {
                let mut lines = vec!["*** friends:".to_string()];
                for f in &friends {
                    let short = &hex::encode(f.pubkey)[..8];
                    let alias = f.alias.as_deref().unwrap_or("");
                    if alias.is_empty() {
                        lines.push(format!("    {short}"));
                    } else {
                        lines.push(format!("    {short}  {alias}"));
                    }
                }
                ui.render_system(&lines.join("\n"));
            }
            Err(e) => ui.render_system(&format!("*** /friend: {e}")),
        },
        SlashCommand::Friend(arg) => match resolve_pubkey(profile_name, &arg) {
            Ok(pk) => {
                let friend = profile::Friend {
                    pubkey: pk,
                    alias: None,
                    cached_name: None,
                    cached_bio_line: None,
                };
                match profile::save_friend(profile_name, &friend) {
                    Ok(()) => ui.render_system(&format!("*** added friend {}", &hex::encode(pk)[..8])),
                    Err(e) => ui.render_system(&format!("*** /friend: {e}")),
                }
            }
            Err(e) => ui.render_system(&format!("*** /friend: {e}")),
        },
        SlashCommand::Unfriend(arg) => match resolve_pubkey(profile_name, &arg) {
            Ok(pk) => match profile::remove_friend(profile_name, &pk) {
                Ok(()) => ui.render_system(&format!("*** removed friend {}", &hex::encode(pk)[..8])),
                Err(e) => ui.render_system(&format!("*** /unfriend: {e}")),
            },
            Err(e) => ui.render_system(&format!("*** /unfriend: {e}")),
        },
        SlashCommand::Unknown(s) => {
            ui.render_system(&format!("*** unknown command: /{s}"));
            ui.render_system(commands::help_text());
        }
        SlashCommand::Empty => {
            ui.render_system(commands::help_text());
        }
    }
    false
}
