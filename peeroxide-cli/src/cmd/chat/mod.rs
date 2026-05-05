#![allow(dead_code)]

pub mod crypto;
pub mod debug;
pub mod display;
pub mod dm;
pub mod dm_cmd;
pub mod feed;
pub mod inbox;
pub mod inbox_cmd;
pub mod join;
pub mod nexus;
pub mod post;
pub mod profile;
pub mod reader;
pub mod wire;

use clap::{Parser, Subcommand};

use crate::config::ResolvedConfig;

#[derive(Parser)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub command: ChatCommands,

    /// Enable debug event logging to stderr
    #[arg(long, global = true)]
    pub debug: bool,
}

#[derive(Subcommand)]
pub enum ChatCommands {
    /// Join a channel and participate interactively
    Join(join::JoinArgs),
    /// Start or resume a DM conversation
    Dm(dm_cmd::DmArgs),
    /// Monitor the invite inbox
    Inbox(inbox_cmd::InboxArgs),
    /// Display the current profile's identity
    Whoami(WhoamiArgs),
    /// Manage profiles
    Profiles {
        #[command(subcommand)]
        command: ProfilesCommands,
    },
    /// Manage friends list
    Friends {
        #[command(subcommand)]
        command: Option<FriendsCommands>,
    },
    /// Manage the personal nexus record
    Nexus(nexus::NexusArgs),
}

#[derive(Parser)]
pub struct WhoamiArgs {
    /// Profile to display
    #[arg(long, default_value = "default")]
    pub profile: String,
}

#[derive(Subcommand)]
pub enum ProfilesCommands {
    /// List all profiles
    List,
    /// Create a new profile
    Create {
        /// Profile name
        name: String,
        /// Optional screen name
        #[arg(long)]
        screen_name: Option<String>,
    },
    /// Delete a profile
    Delete {
        /// Profile name to delete
        name: String,
    },
}

#[derive(Subcommand)]
pub enum FriendsCommands {
    /// List all friends
    List,
    /// Add a friend
    Add {
        /// Public key (64-char hex), shortkey (8 hex chars), or name@shortkey
        key: String,
        /// Local alias for this friend
        #[arg(long)]
        alias: Option<String>,
    },
    /// Remove a friend
    Remove {
        /// Public key, shortkey, or name@shortkey
        key: String,
    },
    /// One-shot refresh all friend nexus records
    Refresh,
}

pub async fn run(args: ChatArgs, cfg: &ResolvedConfig) -> i32 {
    if args.debug {
        debug::enable();
    }
    match args.command {
        ChatCommands::Join(join_args) => join::run(join_args, cfg).await,
        ChatCommands::Dm(dm_args) => dm_cmd::run(dm_args, cfg).await,
        ChatCommands::Inbox(inbox_args) => inbox_cmd::run(inbox_args, cfg).await,
        ChatCommands::Whoami(args) => run_whoami(args),
        ChatCommands::Profiles { command } => run_profiles(command),
        ChatCommands::Friends { command } => {
            let command = command.unwrap_or(FriendsCommands::List);
            match command {
                FriendsCommands::Refresh => run_friends_refresh(cfg).await,
                other => run_friends_sync(other),
            }
        }
        ChatCommands::Nexus(nexus_args) => nexus::run(nexus_args, cfg).await,
    }
}

fn run_whoami(args: WhoamiArgs) -> i32 {
    let prof = match profile::load_or_create_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to load profile '{}': {e}", args.profile);
            return 1;
        }
    };

    let kp = peeroxide_dht::hyperdht::KeyPair::from_seed(prof.seed);
    let pubkey_hex = hex::encode(kp.public_key);
    let nexus_topic = hex::encode(peeroxide_dht::crypto::hash(&kp.public_key));

    println!("Profile: {}", prof.name);
    println!("Public key: {pubkey_hex}");
    if let Some(ref name) = prof.screen_name {
        println!("Screen name: {name}");
    } else {
        println!("Screen name: (not set)");
    }
    println!("Nexus topic: {nexus_topic}");
    0
}

fn run_profiles(command: ProfilesCommands) -> i32 {
    match command {
        ProfilesCommands::List => {
            let profiles = match profile::list_profiles() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            };
            if profiles.is_empty() {
                println!("No profiles found. Create one with: peeroxide chat profiles create <name>");
                return 0;
            }
            for name in profiles {
                match profile::load_profile(&name) {
                    Ok(prof) => {
                        let kp = peeroxide_dht::hyperdht::KeyPair::from_seed(prof.seed);
                        let short = &hex::encode(kp.public_key)[..8];
                        let screen = prof
                            .screen_name
                            .as_deref()
                            .map(|s| format!("({s})"))
                            .unwrap_or_else(|| "(no screen name)".to_string());
                        println!("  {name:16} {short}...  {screen}");
                    }
                    Err(e) => {
                        println!("  {name:16} (error: {e})");
                    }
                }
            }
            0
        }
        ProfilesCommands::Create { name, screen_name } => {
            match profile::create_profile(&name, screen_name.as_deref()) {
                Ok(prof) => {
                    let kp = peeroxide_dht::hyperdht::KeyPair::from_seed(prof.seed);
                    let pubkey_hex = hex::encode(kp.public_key);
                    println!("Created profile '{name}'");
                    println!("Public key: {pubkey_hex}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        ProfilesCommands::Delete { name } => {
            if name == "default" {
                eprintln!("error: cannot delete the default profile");
                return 1;
            }
            match profile::delete_profile(&name) {
                Ok(()) => {
                    println!("Deleted profile '{name}'");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
    }
}

fn run_friends_sync(command: FriendsCommands) -> i32 {
    match command {
        FriendsCommands::List => {
            let friends = match profile::load_friends("default") {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            };
            if friends.is_empty() {
                println!("No friends. Add one with: peeroxide chat friends add <pubkey>");
                return 0;
            }
            for f in &friends {
                let pk_hex = hex::encode(f.pubkey);
                let short = &pk_hex[..8];
                let alias_str = f.alias.as_deref().unwrap_or("");
                let name_str = f.cached_name.as_deref().unwrap_or("(unknown)");
                if alias_str.is_empty() {
                    println!("  {short}  {name_str}");
                } else {
                    println!("  {short}  {alias_str} ({name_str})");
                }
            }
            0
        }
        FriendsCommands::Add { key, alias } => {
            // Resolve key: could be full 64-char hex, 8-char shortkey, or name@shortkey
            let pubkey = match resolve_friend_key("default", &key) {
                Ok(pk) => pk,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            };
            let friend = profile::Friend {
                pubkey,
                alias,
                cached_name: None,
                cached_bio_line: None,
            };
            if let Err(e) = profile::save_friend("default", &friend) {
                eprintln!("error: {e}");
                return 1;
            }
            println!("Added friend {}", hex::encode(pubkey));
            0
        }
        FriendsCommands::Remove { key } => {
            let pubkey = match resolve_friend_key("default", &key) {
                Ok(pk) => pk,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            };
            if let Err(e) = profile::remove_friend("default", &pubkey) {
                eprintln!("error: {e}");
                return 1;
            }
            println!("Removed friend {}", &hex::encode(pubkey)[..8]);
            0
        }
        FriendsCommands::Refresh => unreachable!(),
    }
}

async fn run_friends_refresh(cfg: &ResolvedConfig) -> i32 {
    use libudx::UdxRuntime;
    use peeroxide_dht::hyperdht;

    let dht_config = crate::cmd::build_dht_config(cfg);
    let runtime = match UdxRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let (task, handle, _) = match hyperdht::spawn(&runtime, dht_config).await {
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

    eprintln!("*** refreshing friend nexus records...");
    nexus::refresh_friends(&handle, "default").await;
    eprintln!("*** done");

    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

/// Resolve a friend key from various formats:
/// - 64-char hex pubkey
/// - 8-char shortkey (looked up in known_users)
/// - name@shortkey (shortkey portion used for lookup)
fn resolve_friend_key(profile_name: &str, input: &str) -> Result<[u8; 32], String> {
    // Full 64-char hex
    if input.len() == 64 {
        if let Ok(bytes) = hex::decode(input) {
            if bytes.len() == 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                return Ok(pk);
            }
        }
    }

    // Extract shortkey portion (after @ if present)
    let shortkey = if let Some(pos) = input.rfind('@') {
        &input[pos + 1..]
    } else {
        input
    };

    if shortkey.len() != 8 {
        return Err(format!(
            "invalid key format: expected 64-char hex, 8-char shortkey, or name@shortkey, got '{input}'"
        ));
    }

    match profile::resolve_shortkey(profile_name, shortkey) {
        Ok(Some(pk)) => Ok(pk),
        Ok(None) => Err(format!("shortkey '{shortkey}' not found in known users")),
        Err(e) => Err(format!("failed to search known users: {e}")),
    }
}
