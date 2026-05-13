//! Generic chat session orchestration shared by `chat join` and `chat dm`.
//!
//! A "chat session" is the long-running attached state for a single
//! channel: the spawned reader / publisher / nexus / friend-refresh /
//! inbox-monitor / dht-status tasks, the main `select!` loop that turns
//! incoming messages, system notices, and UI events into actions, and
//! the orderly shutdown sequence.
//!
//! Both `chat join` and `chat dm` build a [`SessionConfig`] and call
//! [`run`]. The DM-specific behaviour (initial inbox invite, per-post
//! nudge, optional invite retraction on shutdown) is gated behind
//! `config.dm.is_some()` and runs in a small dedicated `dm_nudge` task
//! so the rest of the orchestration stays channel-agnostic.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cmd::chat::crypto;
use crate::cmd::chat::display;
use crate::cmd::chat::feed;
use crate::cmd::chat::inbox;
use crate::cmd::chat::known_users::SharedKnownUsers;
use crate::cmd::chat::profile::{self, Profile};
use crate::cmd::chat::publisher::{self, PubJob};
use crate::cmd::chat::reader;
use crate::cmd::chat::tui::{
    self, ChatUi, IgnoreSet, NoticeSink, SlashCommand, StatusState, UiInput, UiOptions, commands,
};
use crate::cmd::{build_dht_config, sigterm_recv};
use crate::config::ResolvedConfig;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht::{self, KeyPair};

/// All inputs needed to run one chat-session lifecycle. `chat join`
/// builds this from a channel name + optional salt; `chat dm` builds it
/// from a recipient pubkey + DM-specific extras.
pub struct SessionConfig {
    /// Compact human-readable label for the status bar's channel-name
    /// field (e.g. `"#room"` or `"DM:alice@abc12345"`). Distinct from
    /// any topic / key value.
    pub bar_name: String,
    /// One-line system notice shown after the DHT is bootstrapped to
    /// announce the user has joined this session. e.g.
    /// `"*** joining channel '#room'"` or `"*** DM with alice (abc12345)"`.
    pub greeting: String,
    /// 32-byte channel key (`channel_key(name, salt)` or
    /// `dm_channel_key(me, them)`). Drives the announce topic schedule
    /// and (for non-DM channels) the message encryption key.
    pub channel_key: [u8; 32],
    /// 32-byte symmetric message-envelope encryption key
    /// (`msg_key(channel_key)` for plain channels, `dm_msg_key(ecdh,
    /// channel_key)` for DMs).
    pub message_key: [u8; 32],
    /// Profile name used for friends file, slash command resolution,
    /// nexus refresh.
    pub profile: String,
    /// Already-loaded profile (avoids the session having to reload).
    pub prof: Profile,
    /// Identity keypair derived from `prof.seed`.
    pub id_keypair: KeyPair,

    pub read_only: bool,
    pub no_nexus: bool,
    pub no_friends: bool,
    pub no_inbox: bool,
    pub feed_lifetime: u64,
    pub batch_size: usize,
    pub batch_wait_ms: u64,
    pub inbox_poll_interval: u64,
    pub stay_after_eof: bool,
    pub line_mode: bool,

    /// DM-specific extras. `Some(_)` activates the inbox-invite send,
    /// per-post nudge, and best-effort invite retraction on shutdown.
    pub dm: Option<DmExtras>,
}

/// DM-specific session config, carried inside [`SessionConfig::dm`].
pub struct DmExtras {
    /// Recipient identity public key.
    pub recipient_pubkey: [u8; 32],
    /// Optional initial-message lure included in the first inbox invite
    /// sent on session startup. None = silent invite.
    pub initial_message: Option<String>,
}

/// Run one chat session to completion. Returns the process exit code
/// (typically 0 on a clean shutdown, non-zero on a fatal startup error).
pub async fn run(config: SessionConfig, cfg: &ResolvedConfig) -> i32 {
    let SessionConfig {
        bar_name,
        greeting,
        channel_key,
        message_key,
        profile: profile_name,
        prof,
        id_keypair,
        read_only,
        no_nexus,
        no_friends,
        no_inbox,
        feed_lifetime,
        batch_size,
        batch_wait_ms,
        inbox_poll_interval,
        stay_after_eof,
        line_mode,
        dm,
    } = config;

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
    // Constructed BEFORE the DHT handshake so all subsequent startup
    // notices flow through the UI in proper layout instead of landing
    // wherever the cursor happens to be.
    let ui_opts = UiOptions {
        force_line_mode: line_mode,
        channel_name: bar_name.clone(),
        profile_name: profile_name.clone(),
    };
    let mut ui: Box<dyn ChatUi> = tui::make_ui(ui_opts);
    let status: Arc<StatusState> = ui.status();
    let ignore: IgnoreSet = ui.ignore_set();

    // Process-wide notice channel for background helpers.
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
            feed_lifetime,
        )
    });

    status.set_dht_peers(table_size);

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<display::DisplayMessage>();

    let friends = profile::load_friends(&profile_name).unwrap_or_default();
    let mut display_state =
        display::DisplayState::new(friends, SharedKnownUsers::load_from_shared());

    ui.render_system(&greeting);

    // --- DM-specific: invite-feed-keypair + initial invite ---
    //
    // For DM sessions we generate an ephemeral invite-feed-keypair (used
    // for the inbox invite + per-epoch nudges) here, after the DHT is up.
    // If an initial message was provided we send the first invite now
    // before the rest of the session machinery starts so the recipient's
    // inbox monitor has the earliest possible opportunity to discover us.
    let invite_feed_keypair = if dm.is_some() && !read_only {
        Some(KeyPair::generate())
    } else {
        None
    };
    if let (Some(dm_extras), Some(inv_kp), Some(fs)) =
        (dm.as_ref(), invite_feed_keypair.as_ref(), feed_state.as_ref())
        && let Some(msg_text) = dm_extras.initial_message.as_ref()
    {
        if let Err(e) = inbox::send_dm_invite(
            &handle,
            inv_kp,
            &id_keypair,
            &dm_extras.recipient_pubkey,
            &channel_key,
            &fs.feed_keypair.public_key,
            msg_text,
        )
        .await
        {
            ui.render_system(&format!("warning: invite send failed: {e}"));
        }
    }

    // --- Reader task ---
    let self_id = id_keypair.public_key;
    let reader_handle = {
        let handle = handle.clone();
        let msg_tx = msg_tx.clone();
        let profile_name = profile_name.clone();
        let self_feed_pubkey = feed_keypair.as_ref().map(|fkp| fkp.public_key);
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

    // --- Publisher worker ---
    let mut pub_tx: Option<mpsc::Sender<PubJob>> = None;
    let mut publisher_handle: Option<JoinHandle<()>> = None;
    if let Some(fs) = feed_state {
        let (tx, rx) = mpsc::channel::<PubJob>(64);
        pub_tx = Some(tx);

        let screen_name = prof.screen_name.clone().unwrap_or_default();
        let handle_pub = handle.clone();
        let id_kp = id_keypair.clone();
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

    // --- DM nudge task ---
    //
    // For DM sessions, after each user-typed message we forward the text
    // into a small dedicated task that checks if the current epoch is
    // later than the last nudged one and, if so, fires a `send_dm_nudge`
    // (a fresh mutable_put on the invite_feed_keypair + an inbox-topic
    // announce). Once-per-epoch throttling matches the original
    // dm_cmd.rs behavior; the publisher knows nothing about DM.
    let mut nudge_tx: Option<mpsc::UnboundedSender<String>> = None;
    let mut nudge_handle: Option<JoinHandle<()>> = None;
    if let (Some(dm_extras), Some(inv_kp), Some(real_feed_pk)) = (
        dm.as_ref(),
        invite_feed_keypair.as_ref(),
        feed_keypair.as_ref().map(|f| f.public_key),
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        nudge_tx = Some(tx);
        let handle = handle.clone();
        let inv_kp = inv_kp.clone();
        let id_kp = id_keypair.clone();
        let recipient = dm_extras.recipient_pubkey;
        nudge_handle = Some(tokio::spawn(async move {
            let mut last_nudged_epoch = 0u64;
            let mut nudge_seq = 0u64;
            while let Some(text) = rx.recv().await {
                let current = crypto::current_epoch();
                if current == last_nudged_epoch {
                    continue;
                }
                let _ = inbox::send_dm_nudge(
                    &handle,
                    &inv_kp,
                    &id_kp,
                    &recipient,
                    &channel_key,
                    &real_feed_pk,
                    &text,
                    nudge_seq,
                )
                .await;
                nudge_seq += 1;
                last_nudged_epoch = current;
            }
        }));
    }

    // --- Nexus refresh ---
    let nexus_handle: Option<JoinHandle<()>> = if !no_nexus {
        let handle = handle.clone();
        let id_kp = id_keypair.clone();
        let profile_name = profile_name.clone();
        let notices = notice_tx.clone();
        Some(tokio::spawn(async move {
            crate::cmd::chat::nexus::run_nexus_refresh(handle, id_kp, profile_name, notices).await;
        }))
    } else {
        None
    };

    // --- Friend refresh ---
    let friend_refresh_handle: Option<JoinHandle<()>> = if !no_friends {
        let handle = handle.clone();
        let profile_name = profile_name.clone();
        let notices = notice_tx.clone();
        Some(tokio::spawn(async move {
            crate::cmd::chat::nexus::run_friend_refresh(handle, profile_name, notices).await;
        }))
    } else {
        None
    };

    // --- DHT peer-count poller ---
    let dht_status_handle: JoinHandle<()> = {
        let handle = handle.clone();
        let status = status.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                let n = handle.table_size().await.unwrap_or(0);
                status.set_dht_peers(n);
            }
        })
    };

    // --- Inbox monitor ---
    let inbox_state: Option<Arc<crate::cmd::chat::inbox_monitor::InboxMonitor>> = if !no_inbox {
        let cached_users = crate::cmd::chat::known_users::load_shared_users().unwrap_or_default();
        Some(Arc::new(
            crate::cmd::chat::inbox_monitor::InboxMonitor::new(cached_users),
        ))
    } else {
        None
    };
    status.set_inbox_enabled(inbox_state.is_some());
    let inbox_handle: Option<JoinHandle<()>> = inbox_state.as_ref().map(|m| {
        let handle = handle.clone();
        let id_kp = id_keypair.clone();
        let status = status.clone();
        let monitor = m.clone();
        let interval_secs = inbox_poll_interval.max(1);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                let _ = monitor.poll_once(&handle, &id_kp).await;
                status.set_inbox_unread(monitor.unread_count());
            }
        })
    });

    // --- Main loop ---
    let mut backlog_done = false;
    let friends_reload_interval = Duration::from_secs(30);
    let mut friends_reload_tick = tokio::time::interval(friends_reload_interval);
    let mut eof_handled = false;
    let mut graceful_eof_exit = false;
    let mut want_exit = false;

    loop {
        tokio::select! {
            Some(line) = notice_rx.recv() => {
                ui.render_system(&line);
            }
            Some(msg) = msg_rx.recv() => {
                if !backlog_done && msg.content.is_empty() && msg.id_pubkey == [0u8; 32] && msg.timestamp == 0 {
                    backlog_done = true;
                    ui.render_system("*** — live —");
                    continue;
                }
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
                let _ = out;
            }
            _ = friends_reload_tick.tick() => {
                if let Ok(updated_friends) = profile::load_friends(&profile_name) {
                    display_state.reload_friends(updated_friends);
                }
            }
            input = ui.next_input() => {
                match input {
                    Some(UiInput::Message(text)) => {
                        if let Some(tx) = pub_tx.as_ref() {
                            status.inc_send_pending();
                            // Forward text to the DM nudge task too (if
                            // active). Throttling to one nudge per epoch
                            // happens inside the task. Ignore send errors —
                            // the task is gone, the user is mid-shutdown.
                            if let Some(ntx) = nudge_tx.as_ref() {
                                let _ = ntx.send(text.clone());
                            }
                            // Bounded mpsc(64); on backpressure, watch for
                            // ctrl_c so the outer select! can react.
                            tokio::select! {
                                biased;
                                _ = tokio::signal::ctrl_c() => {
                                    status.dec_send_pending();
                                    ui.render_system("*** shutting down");
                                    break;
                                }
                                send_res = tx.send(PubJob::Message(text)) => {
                                    if send_res.is_err() {
                                        status.dec_send_pending();
                                    }
                                }
                            }
                        } else {
                            ui.render_system("*** read-only mode; message not sent");
                        }
                    }
                    Some(UiInput::Command(cmd)) => {
                        if dispatch_slash(
                            cmd,
                            &profile_name,
                            ui.as_ref(),
                            &ignore,
                            &status,
                            inbox_state.as_ref(),
                        ).await {
                            ui.render_system("*** shutting down");
                            break;
                        }
                    }
                    Some(UiInput::Eof) => {
                        if !eof_handled {
                            eof_handled = true;
                            if stay_after_eof {
                                ui.render_system("*** stdin closed, entering read-only mode");
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

    // --- Shutdown ---
    drop(pub_tx);

    if let Some(h) = publisher_handle {
        if graceful_eof_exit {
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
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
    }
    reader_handle.abort();
    if let Some(h) = nexus_handle {
        h.abort();
    }
    if let Some(h) = friend_refresh_handle {
        h.abort();
    }
    if let Some(h) = inbox_handle {
        h.abort();
    }
    // Drop nudge_tx so the nudge task's rx.recv() returns None and it
    // exits cleanly, then await briefly. Skips when DM mode wasn't
    // active.
    drop(nudge_tx);
    if let Some(h) = nudge_handle {
        let _ = tokio::time::timeout(Duration::from_secs(1), h).await;
    }
    dht_status_handle.abort();

    // Best-effort: for DM sessions, retract the invite-feed by writing an
    // empty payload at the next seq. Bounded to 1 s so a stuck DHT can't
    // hang shutdown. Failure is silent — TTL on the DHT will eventually
    // expire the announce regardless.
    if let (Some(_dm_extras), Some(inv_kp)) = (dm.as_ref(), invite_feed_keypair.as_ref()) {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            handle.mutable_put(inv_kp, b"", u64::MAX / 2),
        )
        .await;
    }

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
    status: &StatusState,
    inbox_state: Option<&Arc<crate::cmd::chat::inbox_monitor::InboxMonitor>>,
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
                    Ok(()) => {
                        ui.render_system(&format!("*** added friend {}", &hex::encode(pk)[..8]))
                    }
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
        SlashCommand::Inbox => match inbox_state {
            None => ui.render_system(
                "*** inbox monitoring disabled (started with --no-inbox); restart without that flag to enable",
            ),
            Some(monitor) => {
                let drained = monitor.take_unread();
                let known = monitor.known_users().to_vec();
                status.set_inbox_unread(0);
                if drained.is_empty() {
                    ui.render_system("*** inbox: no new invites");
                } else {
                    let n = drained.len();
                    ui.render_system(&format!("*** inbox: {n} new invite(s)"));
                    for inv in &drained {
                        for line in crate::cmd::chat::inbox_monitor::format_invite_lines(
                            inv,
                            profile_name,
                            &known,
                        ) {
                            ui.render_system(&line);
                        }
                    }
                }
            }
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
