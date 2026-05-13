use clap::Parser;

use crate::cmd::chat::inbox_monitor::{format_invite_lines, InboxMonitor};
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

    let cached_users = known_users::load_shared_users().unwrap_or_default();
    let monitor = InboxMonitor::new(cached_users);

    let poll_interval = tokio::time::Duration::from_secs(args.poll_interval);
    let mut interval = tokio::time::interval(poll_interval);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let new_invites = monitor.poll_once(&handle, &id_keypair).await;
                // Live-print and drain at once so the unread buffer doesn't
                // grow unboundedly; the CLI's whole purpose is to surface
                // new invites as they arrive.
                let _ = monitor.take_unread();
                for inv in &new_invites {
                    for line in format_invite_lines(inv, &args.profile, monitor.known_users()) {
                        println!("{line}");
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
