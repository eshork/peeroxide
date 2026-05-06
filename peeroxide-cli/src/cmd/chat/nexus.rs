use clap::Parser;

use peeroxide_dht::hyperdht::{HyperDhtHandle, KeyPair};

use crate::cmd::chat::debug;
use crate::cmd::chat::known_users;
use crate::cmd::chat::profile;
use crate::cmd::chat::wire::NexusRecord;
use crate::cmd::{build_dht_config, sigterm_recv};
use crate::config::ResolvedConfig;

use libudx::UdxRuntime;
use peeroxide_dht::hyperdht;

#[derive(Parser)]
pub struct NexusArgs {
    /// Profile to manage
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Update screen name
    #[arg(long)]
    pub set_name: Option<String>,

    /// Update bio
    #[arg(long)]
    pub set_bio: Option<String>,

    /// Publish nexus to DHT (one-shot)
    #[arg(long)]
    pub publish: bool,

    /// Look up another user's nexus
    #[arg(long)]
    pub lookup: Option<String>,

    /// Run continuously: publish own + refresh friends
    #[arg(long)]
    pub daemon: bool,
}

pub async fn run(args: NexusArgs, cfg: &ResolvedConfig) -> i32 {
    if let Some(ref pubkey_hex) = args.lookup {
        return run_lookup(pubkey_hex, cfg).await;
    }

    let _ = profile::load_or_create_profile(&args.profile);

    if let Some(ref name) = args.set_name {
        let dir = profile::profile_dir(&args.profile);
        if let Err(e) = std::fs::write(dir.join("name"), name.trim()) {
            eprintln!("error: failed to write name: {e}");
            return 1;
        }
        println!("Screen name updated to: {}", name.trim());
        if !args.publish && !args.daemon {
            return 0;
        }
    }

    if let Some(ref bio) = args.set_bio {
        let dir = profile::profile_dir(&args.profile);
        if let Err(e) = std::fs::write(dir.join("bio"), bio.trim()) {
            eprintln!("error: failed to write bio: {e}");
            return 1;
        }
        println!("Bio updated.");
        if !args.publish && !args.daemon {
            return 0;
        }
    }

    let prof = match profile::load_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let id_keypair = KeyPair::from_seed(prof.seed);

    let dht_config = build_dht_config(cfg);
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

    if args.publish {
        publish_nexus_once(&handle, &id_keypair, &args.profile).await;
        let _ = handle.destroy().await;
        let _ = task.await;
        return 0;
    }

    if args.daemon {
        eprintln!("*** nexus daemon started (publish + friend refresh)");
        let profile_name = args.profile.clone();
        let publish_interval = tokio::time::Duration::from_secs(480);
        let friend_interval = tokio::time::Duration::from_secs(600);
        let mut publish_timer = tokio::time::interval(publish_interval);
        let mut friend_timer = tokio::time::interval(friend_interval);
        loop {
            tokio::select! {
                _ = publish_timer.tick() => {
                    publish_nexus_once(&handle, &id_keypair, &profile_name).await;
                }
                _ = friend_timer.tick() => {
                    refresh_friends(&handle, &profile_name).await;
                }
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("\n*** shutting down");
                    break;
                }
                _ = sigterm_recv() => {
                    break;
                }
            }
        }
        let _ = handle.destroy().await;
        let _ = task.await;
        return 0;
    }

    publish_nexus_once(&handle, &id_keypair, &args.profile).await;
    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

async fn publish_nexus_once(handle: &HyperDhtHandle, id_keypair: &KeyPair, profile_name: &str) {
    let prof = match profile::load_profile(profile_name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: failed to load profile for nexus: {e}");
            return;
        }
    };

    let record = NexusRecord {
        name: prof.screen_name.unwrap_or_default(),
        bio: prof.bio.unwrap_or_default(),
    };

    let data = match record.serialize() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("warning: nexus serialize failed: {e}");
            return;
        }
    };

    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match handle.mutable_put(id_keypair, &data, seq).await {
        Ok(_) => {
            eprintln!("  nexus published (seq={seq})");
            debug::log_event(
                "Nexus publish",
                "mutable_put",
                &format!(
                    "id_pubkey={}, seq={seq}, name_len={}, bio_len={}",
                    debug::short_key(&id_keypair.public_key),
                    record.name.len(),
                    record.bio.len(),
                ),
            );
        }
        Err(e) => {
            eprintln!("warning: nexus publish failed: {e}");
        }
    }
}

pub async fn run_nexus_refresh(handle: HyperDhtHandle, id_keypair: KeyPair, profile_name: String) {
    let refresh_interval = tokio::time::Duration::from_secs(480);
    let mut interval = tokio::time::interval(refresh_interval);

    loop {
        interval.tick().await;
        publish_nexus_once(&handle, &id_keypair, &profile_name).await;
    }
}

async fn run_lookup(pubkey_hex: &str, cfg: &ResolvedConfig) -> i32 {
    let pk_bytes = match hex::decode(pubkey_hex) {
        Ok(b) if b.len() == 32 => {
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&b);
            pk
        }
        _ => {
            eprintln!("error: invalid pubkey (expected 64-char hex)");
            return 1;
        }
    };

    let dht_config = build_dht_config(cfg);
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
            eprintln!("error: {e}");
            return 1;
        }
    };

    if let Err(e) = handle.bootstrapped().await {
        eprintln!("error: bootstrap failed: {e}");
        return 1;
    }

    match handle.mutable_get(&pk_bytes, 0).await {
        Ok(Some(result)) => match NexusRecord::deserialize(&result.value) {
            Ok(nexus) => {
                println!("Pubkey: {pubkey_hex}");
                if nexus.name.is_empty() {
                    println!("Name: (not set)");
                } else {
                    println!("Name: {}", nexus.name);
                }
                if !nexus.bio.is_empty() {
                    println!("Bio: {}", nexus.bio);
                }
                println!("Seq: {}", result.seq);
            }
            Err(e) => {
                eprintln!("error: failed to parse nexus record: {e}");
            }
        },
        Ok(None) => {
            println!("No nexus record found for {pubkey_hex}");
        }
        Err(e) => {
            eprintln!("error: mutable_get failed: {e}");
        }
    }

    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

pub async fn run_friend_refresh(handle: HyperDhtHandle, profile_name: String) {
    let refresh_interval = tokio::time::Duration::from_secs(600);
    let mut interval = tokio::time::interval(refresh_interval);
    let mut friend_index: usize = 0;

    loop {
        interval.tick().await;
        refresh_one_friend(&handle, &profile_name, &mut friend_index).await;
    }
}

async fn refresh_one_friend(handle: &HyperDhtHandle, profile_name: &str, index: &mut usize) {
    let friends = match profile::load_friends(profile_name) {
        Ok(f) => f,
        Err(_) => return,
    };

    if friends.is_empty() {
        return;
    }

    *index %= friends.len();
    let friend = &friends[*index];
    *index += 1;

    if let Ok(Some(result)) = handle.mutable_get(&friend.pubkey, 0).await {
        if let Ok(nexus) = NexusRecord::deserialize(&result.value) {
            let mut updated = friend.clone();
            let mut changed = false;
            let name = nexus.name.clone();
            let name_len = nexus.name.len();
            let bio_len = nexus.bio.len();
            if !name.is_empty() && updated.cached_name.as_deref() != Some(&name) {
                updated.cached_name = Some(name.clone());
                let _ = known_users::update_shared(&friend.pubkey, &name);
                changed = true;
            }
            if !nexus.bio.is_empty() {
                let first_line = nexus.bio.lines().next().unwrap_or("").to_owned();
                if updated.cached_bio_line.as_deref() != Some(&first_line) {
                    updated.cached_bio_line = Some(first_line);
                    changed = true;
                }
            }
            if changed {
                debug::log_event(
                    "Friend nexus update",
                    "mutable_get",
                    &format!(
                        "friend_pubkey={}, seq={}, name_len={name_len}, bio_len={bio_len}",
                        debug::short_key(&friend.pubkey),
                        result.seq,
                    ),
                );
                let _ = profile::remove_friend(profile_name, &friend.pubkey);
                let _ = profile::save_friend(profile_name, &updated);
            }
        }
    }
}

pub async fn refresh_friends(handle: &HyperDhtHandle, profile_name: &str) {
    let friends = match profile::load_friends(profile_name) {
        Ok(f) => f,
        Err(_) => return,
    };

    for friend in &friends {
        if let Ok(Some(result)) = handle.mutable_get(&friend.pubkey, 0).await {
            if let Ok(nexus) = NexusRecord::deserialize(&result.value) {
                let mut updated = friend.clone();
                let mut changed = false;
                let name = nexus.name.clone();
                let name_len = nexus.name.len();
                let bio_len = nexus.bio.len();
                if !name.is_empty() && updated.cached_name.as_deref() != Some(&name) {
                    updated.cached_name = Some(name.clone());
                    let _ = known_users::update_shared(&updated.pubkey, &name);
                    changed = true;
                }
                if !nexus.bio.is_empty() {
                    let first_line = nexus.bio.lines().next().unwrap_or("").to_owned();
                    if updated.cached_bio_line.as_deref() != Some(&first_line) {
                        updated.cached_bio_line = Some(first_line);
                        changed = true;
                    }
                }
                if changed {
                    debug::log_event(
                        "Friend nexus update",
                        "mutable_get",
                        &format!(
                            "friend_pubkey={}, seq={}, name_len={name_len}, bio_len={bio_len}",
                            debug::short_key(&friend.pubkey),
                            result.seq,
                        ),
                    );
                    let _ = profile::remove_friend(profile_name, &friend.pubkey);
                    let _ = profile::save_friend(profile_name, &updated);
                }
            }
        }
    }
}
