use super::*;
use crate::cmd::sigterm_recv;
use crate::cmd::chat::{known_users, nexus, profile};

const MAX_CHUNKS: usize = 65535;
const ROOT_HEADER_SIZE: usize = 39;
const NON_ROOT_HEADER_SIZE: usize = 33;
const ROOT_PAYLOAD_MAX: usize = MAX_PAYLOAD - ROOT_HEADER_SIZE;
const NON_ROOT_PAYLOAD_MAX: usize = MAX_PAYLOAD - NON_ROOT_HEADER_SIZE;
const VERSION: u8 = 0x01;

fn derive_chunk_keypair(root_seed: &[u8; 32], chunk_index: u16) -> KeyPair {
    let mut input = Vec::with_capacity(34);
    input.extend_from_slice(root_seed);
    input.extend_from_slice(&chunk_index.to_le_bytes());
    let hash = peeroxide::discovery_key(&input);
    KeyPair::from_seed(hash)
}

fn encode_root_chunk(total_chunks: u16, crc: u32, next_pk: &[u8; 32], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ROOT_HEADER_SIZE + payload.len());
    buf.push(VERSION);
    buf.extend_from_slice(&total_chunks.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(next_pk);
    buf.extend_from_slice(payload);
    buf
}

fn encode_non_root_chunk(next_pk: &[u8; 32], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(NON_ROOT_HEADER_SIZE + payload.len());
    buf.push(VERSION);
    buf.extend_from_slice(next_pk);
    buf.extend_from_slice(payload);
    buf
}

pub async fn run_put(args: &PutArgs, cfg: &ResolvedConfig) -> i32 {
    if args.refresh_interval == 0 {
        eprintln!("error: --refresh-interval must be greater than 0");
        return 1;
    }
    if args.ttl == Some(0) {
        eprintln!("error: --ttl must be greater than 0");
        return 1;
    }
    if args.max_pickups == Some(0) {
        eprintln!("error: --max-pickups must be greater than 0");
        return 1;
    }

    let data = if args.file == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
            eprintln!("error: failed to read stdin: {e}");
            return 1;
        }
        buf
    } else {
        match std::fs::read(&args.file) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: failed to read file: {e}");
                return 1;
            }
        }
    };

    let total_chunks = compute_chunk_count(data.len());
    if total_chunks > MAX_CHUNKS {
        eprintln!("error: file too large ({} chunks exceeds max {})", total_chunks, MAX_CHUNKS);
        return 1;
    }

    let root_seed: [u8; 32] = if let Some(ref phrase) = args.passphrase {
        if phrase.is_empty() {
            eprintln!("error: passphrase cannot be empty");
            return 1;
        }
        peeroxide::discovery_key(phrase.as_bytes())
    } else if args.interactive_passphrase {
        eprintln!("Enter passphrase: ");
        let passphrase = rpassword_read();
        if passphrase.is_empty() {
            eprintln!("error: passphrase cannot be empty");
            return 1;
        }
        peeroxide::discovery_key(passphrase.as_bytes())
    } else {
        let mut seed = [0u8; 32];
        use rand::RngCore;
        rand::rng().fill_bytes(&mut seed);
        seed
    };

    let root_kp = KeyPair::from_seed(root_seed);
    let crc = compute_crc32c(&data);

    let chunks = split_into_chunks(&data, total_chunks as u16, crc, &root_seed);

    let dht_config = build_dht_config(cfg);
    let runtime = match UdxRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to create UDP runtime: {e}");
            return 1;
        }
    };

    let (task, handle, _rx) = match hyperdht::spawn(&runtime, dht_config).await {
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

    let (max_concurrency, dispatch_delay): (Option<usize>, Option<Duration>) = if let Some(ref speed_str) = args.max_speed {
        match parse_max_speed(speed_str) {
            Ok(speed) => {
                let cap = ((speed / 22000) as usize).max(1);
                let delay = Duration::from_secs_f64(22000.0 / speed as f64);
                (Some(cap), Some(delay))
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    } else {
        (None, None)
    };

    eprintln!("DD PUT {} chunks ({} bytes)", total_chunks, data.len());

    if let Err(e) = publish_chunks(&handle, &chunks, max_concurrency, dispatch_delay, true).await {
        eprintln!("error: publish failed: {e}");
        let _ = handle.destroy().await;
        let _ = task.await;
        return 1;
    }

    let pickup_key = to_hex(&root_kp.public_key);
    println!("{pickup_key}");

    eprintln!("  published to DHT (best-effort)");
    eprintln!("  pickup key printed to stdout");
    eprintln!("  refreshing every {}s, monitoring for acks...", args.refresh_interval);

    let ack_topic = peeroxide::discovery_key(&[root_kp.public_key.as_slice(), b"ack"].concat());
    let mut seen_acks: HashSet<[u8; 32]> = HashSet::new();
    let mut pickup_count: u64 = 0;

    let ttl_deadline = args.ttl.map(|t| tokio::time::Instant::now() + Duration::from_secs(t));
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(args.refresh_interval));
    refresh_interval.tick().await;
    let mut ack_interval = tokio::time::interval(Duration::from_secs(30));
    ack_interval.tick().await;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => break,
            _ = sigterm_recv() => break,
            _ = async {
                if let Some(deadline) = ttl_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => break,
            _ = refresh_interval.tick() => {
                eprintln!("  refreshing {} chunks...", chunks.len());
                if let Err(e) = publish_chunks(&handle, &chunks, max_concurrency, dispatch_delay, true).await {
                    eprintln!("  warning: refresh failed: {e}");
                }
            }
            _ = ack_interval.tick() => {
                if let Ok(results) = handle.lookup(ack_topic).await {
                    for result in &results {
                        for peer in &result.peers {
                            if seen_acks.insert(peer.public_key) {
                                pickup_count += 1;
                                eprintln!("  [ack] pickup #{pickup_count} detected");
                                if let Some(max) = args.max_pickups {
                                    if pickup_count >= max {
                                        eprintln!("  max pickups reached, stopping");
                                        let _ = handle.destroy().await;
                                        let _ = task.await;
                                        return 0;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    eprintln!("  stopped refreshing; records expire in ~20m");
    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

fn compute_chunk_count(data_len: usize) -> usize {
    if data_len <= ROOT_PAYLOAD_MAX {
        1
    } else {
        let remaining = data_len - ROOT_PAYLOAD_MAX;
        1 + remaining.div_ceil(NON_ROOT_PAYLOAD_MAX)
    }
}

fn split_into_chunks(data: &[u8], total: u16, crc: u32, root_seed: &[u8; 32]) -> Vec<ChunkData> {
    let mut chunks = Vec::new();
    let root_kp = KeyPair::from_seed(*root_seed);

    let root_payload_len = data.len().min(ROOT_PAYLOAD_MAX);
    let root_payload = &data[..root_payload_len];
    let mut offset = root_payload_len;

    let mut keypairs: Vec<KeyPair> = Vec::with_capacity(total as usize);
    keypairs.push(root_kp.clone());
    for i in 1..total {
        keypairs.push(derive_chunk_keypair(root_seed, i));
    }

    let next_pk = if total > 1 {
        keypairs[1].public_key
    } else {
        [0u8; 32]
    };

    chunks.push(ChunkData {
        keypair: root_kp,
        encoded: encode_root_chunk(total, crc, &next_pk, root_payload),
    });

    for i in 1..total as usize {
        let payload_len = (data.len() - offset).min(NON_ROOT_PAYLOAD_MAX);
        let payload = &data[offset..offset + payload_len];
        offset += payload_len;

        let next_pk = if i + 1 < total as usize {
            keypairs[i + 1].public_key
        } else {
            [0u8; 32]
        };

        chunks.push(ChunkData {
            keypair: keypairs[i].clone(),
            encoded: encode_non_root_chunk(&next_pk, payload),
        });
    }

    chunks
}

#[allow(dead_code)]
async fn run_friends_refresh(cfg: &ResolvedConfig) -> i32 {
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

    eprintln!("*** refreshing friend nexus records...");
    nexus::refresh_friends(&handle, "default").await;
    eprintln!("*** done");

    let _ = handle.destroy().await;
    let _ = task.await;
    0
}

/// Resolve a recipient identifier to a 32-byte Ed25519 public key.
#[allow(dead_code)]
pub fn resolve_recipient(profile_name: &str, input: &str) -> Result<[u8; 32], String> {
    let resolved = if input.len() == 64 {
        match hex::decode(input) {
            Ok(bytes) if bytes.len() == 32 => {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                Ok(pk)
            }
            _ => Err(format!("invalid 64-char hex pubkey: '{input}'")),
        }
    } else if let Some(shortkey) = input.strip_prefix('@') {
        resolve_shortkey_input(shortkey)
    } else if let Some(pos) = input.rfind('@') {
        let name_part = &input[..pos];
        let shortkey_part = &input[pos + 1..];
        let pk = resolve_shortkey_input(shortkey_part)?;

        let users = known_users::load_shared_users()
            .map_err(|e| format!("failed to load known users: {e}"))?;
        if let Some(user) = users.iter().find(|u| u.pubkey == pk) {
            if user.screen_name == name_part {
                Ok(pk)
            } else {
                Err("name mismatch".to_string())
            }
        } else {
            Ok(pk)
        }
    } else if input.len() == 8 && input.chars().all(|c| c.is_ascii_hexdigit()) {
        resolve_shortkey_input(input)
    } else {
        let friends = profile::load_friends(profile_name).unwrap_or_default();
        let mut matched_pubkeys: Vec<[u8; 32]> = Vec::new();
        for f in &friends {
            if f.alias.as_deref() == Some(input) {
                matched_pubkeys.push(f.pubkey);
            }
        }

        if matched_pubkeys.is_empty() {
            let users = known_users::load_shared_users().unwrap_or_default();
            for u in &users {
                if u.screen_name == input {
                    matched_pubkeys.push(u.pubkey);
                }
            }
        }

        matched_pubkeys.sort();
        matched_pubkeys.dedup();
        match matched_pubkeys.len() {
            1 => Ok(matched_pubkeys[0]),
            0 => Err(format!("recipient '{input}' not found")),
            n => Err(format!("recipient '{input}' is ambiguous ({n} matches)")),
        }
    };

    let resolved = resolved?;
    if let Ok(own_prof) = profile::load_profile(profile_name) {
        let own_kp = KeyPair::from_seed(own_prof.seed);
        if resolved == own_kp.public_key {
            return Err("cannot send a DM to yourself".to_string());
        }
    }
    Ok(resolved)
}

#[allow(dead_code)]
fn resolve_shortkey_input(shortkey: &str) -> Result<[u8; 32], String> {
    let mut cache = known_users::SharedKnownUsers::load_from_shared();
    match cache.resolve_shortkey(shortkey) {
        Ok(Some(pk)) => Ok(pk),
        Ok(None) => Err(format!("shortkey '{shortkey}' not found in known users")),
        Err(e) => Err(format!("failed to search known users: {e}")),
    }
}

pub async fn get_from_root(
    root_data: Vec<u8>,
    root_pk: [u8; 32],
    handle: HyperDhtHandle,
    task_handle: tokio::task::JoinHandle<Result<(), peeroxide_dht::hyperdht::HyperDhtError>>,
    args: &GetArgs,
) -> i32 {
    let chunk_timeout = Duration::from_secs(args.timeout);

    if root_data.len() < ROOT_HEADER_SIZE {
        eprintln!("error: root chunk too small");
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    let total_chunks = u16::from_le_bytes([root_data[1], root_data[2]]) as usize;
    let stored_crc = u32::from_le_bytes([root_data[3], root_data[4], root_data[5], root_data[6]]);
    let mut next_pk = [0u8; 32];
    next_pk.copy_from_slice(&root_data[7..39]);
    let root_payload = &root_data[39..];

    if total_chunks == 0 || total_chunks > MAX_CHUNKS {
        eprintln!("error: invalid chunk count: {total_chunks}");
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    eprintln!("  fetching chunk 1/{total_chunks}...");

    let mut payload_data = Vec::new();
    payload_data.extend_from_slice(root_payload);

    let mut seen_keys: HashSet<[u8; 32]> = HashSet::new();
    seen_keys.insert(root_pk);

    for i in 1..total_chunks {
        eprintln!("  fetching chunk {}/{}...", i + 1, total_chunks);

        if next_pk == [0u8; 32] {
            if i == total_chunks - 1 {
                break;
            }
            eprintln!("error: chain ended prematurely at chunk {i}");
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        if !seen_keys.insert(next_pk) {
            eprintln!("error: loop detected in chunk chain");
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        let chunk_data = match fetch_with_retry(&handle, &next_pk, chunk_timeout).await {
            Some(d) => d,
            None => {
                eprintln!("error: chunk {} not found (timeout)", i + 1);
                let _ = handle.destroy().await;
                let _ = task_handle.await;
                return 1;
            }
        };

        if chunk_data.is_empty() || chunk_data[0] != VERSION {
            eprintln!("error: invalid chunk {} (bad version)", i + 1);
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        if chunk_data.len() < NON_ROOT_HEADER_SIZE {
            eprintln!("error: chunk {} too small", i + 1);
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        next_pk.copy_from_slice(&chunk_data[1..33]);
        let chunk_payload = &chunk_data[33..];
        payload_data.extend_from_slice(chunk_payload);
    }

    if total_chunks > 1 && next_pk != [0u8; 32] {
        eprintln!("error: final chunk does not terminate chain (next != zeros)");
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    let computed_crc = compute_crc32c(&payload_data);
    if computed_crc != stored_crc {
        eprintln!("error: CRC mismatch (expected {stored_crc:08x}, got {computed_crc:08x})");
        let _ = handle.destroy().await;
        let _ = task_handle.await;
        return 1;
    }

    eprintln!("  reassembled {} bytes", payload_data.len());

    if let Some(ref output_path) = args.output {
        let dir = std::path::Path::new(output_path)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let temp_path = dir.join(format!(".peeroxide-pickup-{}", std::process::id()));

        if let Err(e) = tokio::fs::write(&temp_path, &payload_data).await {
            eprintln!("error: failed to write temp file: {e}");
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        if let Err(e) = tokio::fs::rename(&temp_path, output_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            eprintln!("error: failed to rename: {e}");
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }

        eprintln!("  written to {output_path}");
    } else {
        use std::io::Write;
        if let Err(e) = std::io::stdout().write_all(&payload_data) {
            eprintln!("error: failed to write to stdout: {e}");
            let _ = handle.destroy().await;
            let _ = task_handle.await;
            return 1;
        }
    }

    if !args.no_ack {
        let ack_topic =
            peeroxide::discovery_key(&[root_pk.as_slice(), b"ack"].concat());
        let ack_kp = KeyPair::generate();
        let _ = handle.announce(ack_topic, &ack_kp, &[]).await;
        eprintln!("  ack sent (ephemeral identity)");
    } else {
        eprintln!("  done (no ack sent)");
    }

    eprintln!("  done");
    let _ = handle.destroy().await;
    let _ = task_handle.await;
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::chat::names;
    use std::fs;
    use std::io::{self, Write};
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn pk(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn profile_root(home: &Path) -> std::path::PathBuf {
        home.join(".config/peeroxide/chat/profiles")
    }

    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            unsafe { std::env::set_var("HOME", home) };
            Self(prev)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(prev) => unsafe { std::env::set_var("HOME", prev) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }

    fn write_profile(home: &Path, name: &str, seed: [u8; 32]) -> io::Result<()> {
        let dir = profile_root(home).join(name);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("seed"), seed)
    }

    fn write_known_users(home: &Path, rows: &[([u8; 32], &str)]) -> io::Result<()> {
        let dir = home.join(".config").join("peeroxide").join("chat");
        fs::create_dir_all(&dir)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("known_users"))?;
        for (pubkey, name) in rows {
            writeln!(file, "{}\t{}", hex::encode(pubkey), name)?;
        }
        Ok(())
    }

    fn write_friends(home: &Path, profile_name: &str, rows: &[([u8; 32], Option<&str>)]) -> io::Result<()> {
        let dir = profile_root(home).join(profile_name);
        fs::create_dir_all(&dir)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("friends"))?;
        for (pubkey, alias) in rows {
            writeln!(file, "{}\t{}\t\t", hex::encode(pubkey), alias.unwrap_or(""))?;
        }
        Ok(())
    }

    fn prepare_profile(home: &Path, profile_name: &str) -> io::Result<()> {
        fs::create_dir_all(profile_root(home).join(profile_name))
    }

    fn friend_output(friend: &profile::Friend) -> String {
        let pk_hex = hex::encode(friend.pubkey);
        let short = &pk_hex[..8];
        let alias_str = friend.alias.as_deref().unwrap_or("");
        let name_str = friend
            .cached_name
            .clone()
            .unwrap_or_else(|| names::generate_name_from_seed(&friend.pubkey));
        if alias_str.is_empty() {
            format!("  {short}  {name_str}")
        } else {
            format!("  {short}  {alias_str} ({name_str})")
        }
    }

    fn current_test_binary() -> std::path::PathBuf {
        std::env::current_exe().unwrap()
    }

    fn run_child_case(home: &Path, case: &str, profile_name: &str, input: &str) {
        let output = Command::new(current_test_binary())
            .args(["--exact", "resolve_recipient_sandbox", "--nocapture"])
            .env("HOME", home)
            .env("RESOLVE_CASE", case)
            .env("RESOLVE_PROFILE", profile_name)
            .env("RESOLVE_INPUT", input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_friends_child_case(home: &Path, case: &str) {
        let output = Command::new(current_test_binary())
            .args(["--exact", "friends_sandbox", "--nocapture"])
            .env("HOME", home)
            .env("FRIENDS_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_resolve_64char_valid_hex() {
        let tmp = TempDir::new().unwrap();
        let input = hex::encode([0x11u8; 32]);
        run_child_case(tmp.path(), "valid_hex", "default", &input);
    }

    #[test]
    fn test_resolve_64char_invalid_hex() {
        let tmp = TempDir::new().unwrap();
        let input = "g".repeat(64);
        run_child_case(tmp.path(), "invalid_hex", "default", &input);
    }

    #[test]
    fn test_resolve_at_shortkey() {
        let tmp = TempDir::new().unwrap();
        write_known_users(tmp.path(), &[(pk(1), "Alice")]).unwrap();
        let shortkey = &hex::encode(pk(1))[..8];
        run_child_case(tmp.path(), "at_shortkey", "default", &format!("@{shortkey}"));
    }

    #[test]
    fn test_resolve_name_at_shortkey() {
        let tmp = TempDir::new().unwrap();
        write_known_users(tmp.path(), &[(pk(2), "alice")]).unwrap();
        let shortkey = &hex::encode(pk(2))[..8];
        run_child_case(tmp.path(), "name_at_shortkey", "default", &format!("alice@{shortkey}"));
    }

    #[test]
    fn test_resolve_bare_shortkey() {
        let tmp = TempDir::new().unwrap();
        write_known_users(tmp.path(), &[(pk(3), "Bob")]).unwrap();
        let shortkey = &hex::encode(pk(3))[..8];
        run_child_case(tmp.path(), "bare_shortkey", "default", shortkey);
    }

    #[test]
    fn test_resolve_friend_alias() {
        let tmp = TempDir::new().unwrap();
        write_friends(tmp.path(), "default", &[(pk(4), Some("carol"))]).unwrap();
        run_child_case(tmp.path(), "friend_alias", "default", "carol");
    }

    #[test]
    fn test_resolve_known_user_screen_name() {
        let tmp = TempDir::new().unwrap();
        write_known_users(tmp.path(), &[(pk(5), "dave")]).unwrap();
        run_child_case(tmp.path(), "known_user", "default", "dave");
    }

    #[test]
    fn test_resolve_friend_alias_priority() {
        let tmp = TempDir::new().unwrap();
        write_friends(tmp.path(), "default", &[(pk(6), Some("erin"))]).unwrap();
        write_known_users(tmp.path(), &[(pk(7), "erin")]).unwrap();
        run_child_case(tmp.path(), "friend_priority", "default", "erin");
    }

    #[test]
    fn test_resolve_ambiguous() {
        let tmp = TempDir::new().unwrap();
        write_known_users(tmp.path(), &[(pk(8), "frank"), (pk(9), "frank")]).unwrap();
        run_child_case(tmp.path(), "ambiguous", "default", "frank");
    }

    #[test]
    fn test_resolve_not_found() {
        let tmp = TempDir::new().unwrap();
        run_child_case(tmp.path(), "not_found", "default", "missing");
    }

    #[test]
    fn test_resolve_name_mismatch() {
        let tmp = TempDir::new().unwrap();
        write_known_users(tmp.path(), &[(pk(10), "grace")]).unwrap();
        let shortkey = &hex::encode(pk(10))[..8];
        run_child_case(tmp.path(), "name_mismatch", "default", &format!("wrong@{shortkey}"));
    }

    #[test]
    fn test_friends_add_auto_alias_vendor() {
        let _guard = profile::test_home_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        run_friends_child_case(tmp.path(), "vendor");
    }

    #[test]
    fn test_friends_add_auto_alias_explicit_preserved() {
        let _guard = profile::test_home_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        run_friends_child_case(tmp.path(), "explicit");
    }

    #[test]
    fn test_friends_list_vendor_fallback() {
        let _guard = profile::test_home_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        run_friends_child_case(tmp.path(), "vendor_fallback");
    }

    #[test]
    fn test_friends_list_cached_name_preserved() {
        let tmp = TempDir::new().unwrap();
        let _home = HomeGuard::set(tmp.path());
        prepare_profile(tmp.path(), "default").unwrap();
        let friend = profile::Friend {
            pubkey: pk(14),
            alias: Some("pal".to_string()),
            cached_name: Some("Alice".to_string()),
            cached_bio_line: None,
        };
        let line = friend_output(&friend);
        assert!(line.contains("Alice"));
        assert!(!line.contains("(unknown)"));
    }

    #[test]
    fn test_resolve_self_guard() {
        let tmp = TempDir::new().unwrap();
        let seed = [0x42u8; 32];
        write_profile(tmp.path(), "default", seed).unwrap();
        let own_pk = peeroxide_dht::hyperdht::KeyPair::from_seed(seed).public_key;
        run_child_case(tmp.path(), "self_guard", "default", &hex::encode(own_pk));
    }

    #[test]
    fn resolve_recipient_sandbox() {
        let case = match std::env::var("RESOLVE_CASE") {
            Ok(v) => v,
            Err(_) => return,
        };
        let profile_name = std::env::var("RESOLVE_PROFILE").unwrap();
        let input = std::env::var("RESOLVE_INPUT").unwrap();
        match case.as_str() {
            "valid_hex" => {
                let pk = resolve_recipient(&profile_name, &input).unwrap();
                assert_eq!(pk, [0x11u8; 32]);
            }
            "invalid_hex" => {
                let err = resolve_recipient(&profile_name, &input).unwrap_err();
                assert_eq!(err, format!("invalid 64-char hex pubkey: '{input}'"));
            }
            "at_shortkey" => {
                assert_eq!(resolve_recipient(&profile_name, &input).unwrap(), pk(1));
            }
            "name_at_shortkey" => {
                assert_eq!(resolve_recipient(&profile_name, &input).unwrap(), pk(2));
            }
            "bare_shortkey" => {
                assert_eq!(resolve_recipient(&profile_name, &input).unwrap(), pk(3));
            }
            "friend_alias" => {
                assert_eq!(resolve_recipient(&profile_name, &input).unwrap(), pk(4));
            }
            "known_user" => {
                assert_eq!(resolve_recipient(&profile_name, &input).unwrap(), pk(5));
            }
            "friend_priority" => {
                assert_eq!(resolve_recipient(&profile_name, &input).unwrap(), pk(6));
            }
            "ambiguous" => {
                let err = resolve_recipient(&profile_name, &input).unwrap_err();
                assert!(err.contains("ambiguous"));
            }
            "not_found" => {
                let err = resolve_recipient(&profile_name, &input).unwrap_err();
                assert!(err.contains("not found"));
            }
            "name_mismatch" => {
                let err = resolve_recipient(&profile_name, &input).unwrap_err();
                assert_eq!(err, "name mismatch");
            }
            "self_guard" => {
                let err = resolve_recipient(&profile_name, &input).unwrap_err();
                assert_eq!(err, "cannot send a DM to yourself");
            }
            other => panic!("unknown case: {other}"),
        }
    }

    #[test]
    fn friends_sandbox() {
        let case = match std::env::var("FRIENDS_CASE") {
            Ok(v) => v,
            Err(_) => return,
        };
        match case.as_str() {
            "vendor" => {
                let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
                prepare_profile(&home, "default").unwrap();
                let pubkey = pk(11);
                let expected = names::generate_name_from_seed(&pubkey);
                let friend = profile::Friend {
                    pubkey,
                    alias: Some(expected.clone()),
                    cached_name: None,
                    cached_bio_line: None,
                };
                profile::save_friend("default", &friend).unwrap();
                let loaded = profile::load_friends("default").unwrap();
                assert_eq!(loaded.len(), 1);
                assert_eq!(loaded[0].alias.as_deref(), Some(expected.as_str()));
            }
            "explicit" => {
                let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
                prepare_profile(&home, "default").unwrap();
                let friend = profile::Friend {
                    pubkey: pk(12),
                    alias: Some("buddy".to_string()),
                    cached_name: None,
                    cached_bio_line: None,
                };
                profile::save_friend("default", &friend).unwrap();
                let loaded = profile::load_friends("default").unwrap();
                assert_eq!(loaded.len(), 1);
                assert_eq!(loaded[0].alias.as_deref(), Some("buddy"));
            }
            other => panic!("unknown case: {other}"),
        }
    }

    #[test]
    fn compute_chunk_count_single() {
        assert_eq!(compute_chunk_count(0), 1);
        assert_eq!(compute_chunk_count(1), 1);
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX), 1);
    }

    #[test]
    fn compute_chunk_count_two() {
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX + 1), 2);
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX + NON_ROOT_PAYLOAD_MAX), 2);
    }

    #[test]
    fn compute_chunk_count_three() {
        assert_eq!(compute_chunk_count(ROOT_PAYLOAD_MAX + NON_ROOT_PAYLOAD_MAX + 1), 3);
    }

    #[test]
    fn encode_root_chunk_structure() {
        let next_pk = [0xAA; 32];
        let payload = b"hello";
        let encoded = encode_root_chunk(5, 0x12345678, &next_pk, payload);

        assert_eq!(encoded[0], VERSION);
        assert_eq!(u16::from_le_bytes([encoded[1], encoded[2]]), 5);
        assert_eq!(
            u32::from_le_bytes([encoded[3], encoded[4], encoded[5], encoded[6]]),
            0x12345678
        );
        assert_eq!(&encoded[7..39], &[0xAA; 32]);
        assert_eq!(&encoded[39..], b"hello");
        assert_eq!(encoded.len(), ROOT_HEADER_SIZE + 5);
    }

    #[test]
    fn encode_non_root_chunk_structure() {
        let next_pk = [0xBB; 32];
        let payload = b"world";
        let encoded = encode_non_root_chunk(&next_pk, payload);

        assert_eq!(encoded[0], VERSION);
        assert_eq!(&encoded[1..33], &[0xBB; 32]);
        assert_eq!(&encoded[33..], b"world");
        assert_eq!(encoded.len(), NON_ROOT_HEADER_SIZE + 5);
    }

    #[test]
    fn split_and_reassemble_single_chunk() {
        let data = b"short message";
        let seed = [0x42; 32];
        let crc = compute_crc32c(data);
        let chunks = split_into_chunks(data, 1, crc, &seed);

        assert_eq!(chunks.len(), 1);
        let encoded = &chunks[0].encoded;
        assert_eq!(encoded[0], VERSION);
        assert_eq!(u16::from_le_bytes([encoded[1], encoded[2]]), 1);
        let stored_crc = u32::from_le_bytes([encoded[3], encoded[4], encoded[5], encoded[6]]);
        assert_eq!(stored_crc, crc);
        assert_eq!(&encoded[7..39], &[0u8; 32]);
        assert_eq!(&encoded[39..], data.as_slice());
    }

    #[test]
    fn split_and_reassemble_multi_chunk() {
        let data = vec![0x42u8; ROOT_PAYLOAD_MAX + NON_ROOT_PAYLOAD_MAX + 100];
        let crc = compute_crc32c(&data);
        let total = compute_chunk_count(data.len()) as u16;
        assert_eq!(total, 3);

        let seed = [0x01; 32];
        let chunks = split_into_chunks(&data, total, crc, &seed);
        assert_eq!(chunks.len(), 3);

        let root = &chunks[0].encoded;
        let root_total = u16::from_le_bytes([root[1], root[2]]);
        assert_eq!(root_total, 3);
        let root_payload = &root[39..];
        assert_eq!(root_payload.len(), ROOT_PAYLOAD_MAX);

        let c1 = &chunks[1].encoded;
        let c1_payload = &c1[33..];
        assert_eq!(c1_payload.len(), NON_ROOT_PAYLOAD_MAX);

        let c2 = &chunks[2].encoded;
        assert_eq!(&c2[1..33], &[0u8; 32]);
        let c2_payload = &c2[33..];
        assert_eq!(c2_payload.len(), 100);

        let mut reassembled = Vec::new();
        reassembled.extend_from_slice(root_payload);
        reassembled.extend_from_slice(c1_payload);
        reassembled.extend_from_slice(c2_payload);
        assert_eq!(reassembled, data);
        assert_eq!(compute_crc32c(&reassembled), crc);
    }

    #[test]
    fn derive_chunk_keypair_deterministic() {
        let seed = [0xAB; 32];
        let kp1 = derive_chunk_keypair(&seed, 1);
        let kp2 = derive_chunk_keypair(&seed, 1);
        assert_eq!(kp1.public_key, kp2.public_key);

        let kp3 = derive_chunk_keypair(&seed, 2);
        assert_ne!(kp1.public_key, kp3.public_key);
    }

    #[test]
    fn crc32c_basic() {
        let data = b"hello world";
        let crc = compute_crc32c(data);
        assert_eq!(crc, crc32c::crc32c(data));
        assert_ne!(crc, 0);
    }

    #[test]
    fn parse_max_speed_units() {
        assert_eq!(parse_max_speed("100k").unwrap(), 100_000);
        assert_eq!(parse_max_speed("1m").unwrap(), 1_000_000);
        assert_eq!(parse_max_speed("5000").unwrap(), 5000);
        assert_eq!(parse_max_speed(" 2M ").unwrap(), 2_000_000);
        assert!(parse_max_speed("abc").is_err());
    }

    #[test]
    fn derive_pk_from_passphrase_deterministic() {
        let pk1 = derive_pk_from_passphrase("test-phrase");
        let pk2 = derive_pk_from_passphrase("test-phrase");
        assert_eq!(pk1, pk2);

        let pk3 = derive_pk_from_passphrase("different-phrase");
        assert_ne!(pk1, pk3);
    }

    #[test]
    fn chunk_chain_links_correctly() {
        let data = vec![0xFFu8; ROOT_PAYLOAD_MAX + 10];
        let crc = compute_crc32c(&data);
        let total = compute_chunk_count(data.len()) as u16;
        let seed = [0x99; 32];
        let chunks = split_into_chunks(&data, total, crc, &seed);

        let root = &chunks[0].encoded;
        let next_in_root = &root[7..39];
        assert_eq!(next_in_root, chunks[1].keypair.public_key.as_slice());

        let c1 = &chunks[1].encoded;
        let next_in_c1 = &c1[1..33];
        assert_eq!(next_in_c1, &[0u8; 32]);
    }
}
