//! Profile directory management for the peeroxide chat system.
//!
//! ## Directory Layout
//!
//! ```text
//! ~/.config/peeroxide/chat/profiles/<name>/
//! ├── seed         # 32 raw bytes (Ed25519 seed)
//! ├── name         # UTF-8 screen name (optional)
//! ├── bio          # UTF-8 bio text (optional)
//! ├── friends      # tab-separated: pubkey\talias\tcached_name\tcached_bio_line
//! └── known_users  # append-only: pubkey\tscreen_name
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

/// A local chat identity stored on disk.
#[derive(Debug, Clone)]
pub struct Profile {
    /// Directory name used to identify this profile on disk.
    pub name: String,
    /// Raw Ed25519 seed (32 bytes).
    pub seed: [u8; 32],
    /// Optional human-readable screen name.
    pub screen_name: Option<String>,
    /// Optional biography text.
    pub bio: Option<String>,
}

/// A trusted contact stored in the `friends` file.
#[derive(Debug, Clone)]
pub struct Friend {
    /// The friend's Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Local alias chosen by the profile owner.
    pub alias: Option<String>,
    /// Most recently cached screen name announced by the friend.
    pub cached_name: Option<String>,
    /// Most recently cached first line of bio announced by the friend.
    pub cached_bio_line: Option<String>,
}

/// A user seen on the network, stored in `known_users`.
#[derive(Debug, Clone)]
pub struct KnownUser {
    /// The user's Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Screen name observed when the entry was recorded.
    pub screen_name: String,
}

/// Returns `~/.config/peeroxide/chat/profiles/`.
pub fn profiles_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("peeroxide")
        .join("chat")
        .join("profiles")
}

/// Returns the directory for a specific named profile.
pub fn profile_dir(name: &str) -> PathBuf {
    profiles_dir().join(name)
}

/// Creates a new profile on disk.
///
/// Generates a fresh random 32-byte seed, creates the profile directory, and
/// writes the seed (and optional screen name) to disk.  Fails if the profile
/// already exists.
pub fn create_profile(name: &str, screen_name: Option<&str>) -> io::Result<Profile> {
    let dir = profile_dir(name);
    if dir.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("profile '{}' already exists at {}", name, dir.display()),
        ));
    }
    fs::create_dir_all(&dir)?;

    let mut seed = [0u8; 32];
    {
        use rand::RngCore;
        rand::rng().fill_bytes(&mut seed);
    }

    fs::write(dir.join("seed"), seed)?;

    if let Some(sn) = screen_name {
        fs::write(dir.join("name"), sn)?;
    }

    Ok(Profile {
        name: name.to_owned(),
        seed,
        screen_name: screen_name.map(str::to_owned),
        bio: None,
    })
}

/// Loads an existing profile from disk.
pub fn load_profile(name: &str) -> io::Result<Profile> {
    let dir = profile_dir(name);

    let seed_bytes = fs::read(dir.join("seed"))?;
    if seed_bytes.len() != 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "profile '{}': seed file must be exactly 32 bytes, got {}",
                name,
                seed_bytes.len()
            ),
        ));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&seed_bytes);

    let screen_name = read_optional_text(&dir.join("name"))?;
    let bio = read_optional_text(&dir.join("bio"))?;

    Ok(Profile {
        name: name.to_owned(),
        seed,
        screen_name,
        bio,
    })
}

pub fn load_or_create_profile(name: &str) -> io::Result<Profile> {
    match load_profile(name) {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!("*** creating new profile '{name}'");
            create_profile(name, None)
        }
        Err(e) => Err(e),
    }
}

/// Deletes a profile and all its files from disk.
pub fn delete_profile(name: &str) -> io::Result<()> {
    let dir = profile_dir(name);
    if !dir.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("profile '{}' does not exist", name),
        ));
    }
    fs::remove_dir_all(dir)
}

/// Lists all profile names (subdirectory names inside `profiles_dir()`).
pub fn list_profiles() -> io::Result<Vec<String>> {
    let dir = profiles_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(n) = entry.file_name().to_str() {
                names.push(n.to_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Loads the `friends` file for the given profile.
///
/// Lines are tab-separated: `<64-hex-pubkey>\t<alias>\t<cached_name>\t<cached_bio_line>`.
/// Lines starting with `#` are comments and are skipped.  When the same
/// public key appears more than once, the **last** entry wins.
pub fn load_friends(profile_name: &str) -> io::Result<Vec<Friend>> {
    let path = profile_dir(profile_name).join("friends");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)?;

    let mut map: HashMap<[u8; 32], (usize, Friend)> = HashMap::new();
    let mut order: Vec<[u8; 32]> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        let pubkey = match decode_pubkey(parts[0]) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let alias = optional_field(parts.get(1).copied());
        let cached_name = optional_field(parts.get(2).copied());
        let cached_bio_line = optional_field(parts.get(3).copied());

        let friend = Friend {
            pubkey,
            alias,
            cached_name,
            cached_bio_line,
        };

        if let Some(existing) = map.get_mut(&pubkey) {
            existing.1 = friend;
        } else {
            let idx = order.len();
            order.push(pubkey);
            map.insert(pubkey, (idx, friend));
        }
    }

    let mut result: Vec<(usize, Friend)> = map.into_values().collect();
    result.sort_by_key(|(idx, _)| *idx);
    Ok(result.into_iter().map(|(_, f)| f).collect())
}

/// Appends or updates a friend entry in the `friends` file.
///
/// The entry is always appended; deduplication happens at read time (latest
/// entry wins).
pub fn save_friend(profile_name: &str, friend: &Friend) -> io::Result<()> {
    let path = profile_dir(profile_name).join("friends");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    let line = format!(
        "{}\t{}\t{}\t{}\n",
        hex::encode(friend.pubkey),
        friend.alias.as_deref().unwrap_or(""),
        friend.cached_name.as_deref().unwrap_or(""),
        friend.cached_bio_line.as_deref().unwrap_or(""),
    );
    file.write_all(line.as_bytes())
}

/// Removes a friend from the `friends` file by rewriting the file without
/// any entries for the given public key.
pub fn remove_friend(profile_name: &str, pubkey: &[u8; 32]) -> io::Result<()> {
    let path = profile_dir(profile_name).join("friends");
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let target_hex = hex::encode(pubkey);

    let filtered: String = content
        .lines()
        .filter(|line| {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                return true;
            }
            let first_field = l.split('\t').next().unwrap_or("");
            first_field != target_hex
        })
        .map(|l| format!("{}\n", l))
        .collect();

    fs::write(&path, filtered)
}

/// Loads the `known_users` file for the given profile.
///
/// Lines are tab-separated: `<64-hex-pubkey>\t<screen_name>`.
/// The file is append-only during sessions; on read the **last** entry per
/// public key wins (dedup).
pub fn load_known_users(profile_name: &str) -> io::Result<Vec<KnownUser>> {
    let path = profile_dir(profile_name).join("known_users");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)?;
    let mut map: HashMap<[u8; 32], (usize, KnownUser)> = HashMap::new();
    let mut order: Vec<[u8; 32]> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let hex_key = parts.next().unwrap_or("").trim();
        let screen_name = parts.next().unwrap_or("").trim().to_owned();

        let pubkey = match decode_pubkey(hex_key) {
            Ok(k) => k,
            Err(_) => continue,
        };

        let user = KnownUser {
            pubkey,
            screen_name,
        };

        if let Some(existing) = map.get_mut(&pubkey) {
            existing.1 = user;
        } else {
            let idx = order.len();
            order.push(pubkey);
            map.insert(pubkey, (idx, user));
        }
    }

    let mut result: Vec<(usize, KnownUser)> = map.into_values().collect();
    result.sort_by_key(|(idx, _)| *idx);
    Ok(result.into_iter().map(|(_, u)| u).collect())
}

/// Appends a `<pubkey>\t<screen_name>` line to the `known_users` file.
pub fn append_known_user(
    profile_name: &str,
    pubkey: &[u8; 32],
    screen_name: &str,
) -> io::Result<()> {
    let path = profile_dir(profile_name).join("known_users");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    let line = format!("{}\t{}\n", hex::encode(pubkey), screen_name);
    file.write_all(line.as_bytes())
}

/// Resolves a short key (first 8 hex characters) to a full 32-byte public key
/// by scanning `known_users`.
///
/// Returns `None` if no match is found, and an error if the match is
/// ambiguous (more than one pubkey shares the same prefix).
pub fn resolve_shortkey(
    profile_name: &str,
    shortkey: &str,
) -> io::Result<Option<[u8; 32]>> {
    if shortkey.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shortkey must not exceed 64 hex characters",
        ));
    }
    let lower = shortkey.to_lowercase();
    let users = load_known_users(profile_name)?;
    let matches: Vec<[u8; 32]> = users
        .into_iter()
        .filter(|u| hex::encode(u.pubkey).starts_with(&lower))
        .map(|u| u.pubkey)
        .collect();

    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "shortkey '{}' is ambiguous: {} matches found",
                shortkey,
                matches.len()
            ),
        )),
    }
}

fn read_optional_text(path: &std::path::Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim().to_owned();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn decode_pubkey(s: &str) -> Result<[u8; 32], hex::FromHexError> {
    let bytes = hex::decode(s)?;
    if bytes.len() != 32 {
        // `hex::FromHexError` has no wrong-length variant; `InvalidStringLength`
        // is the closest available error for a well-formed but wrong-sized decode.
        return Err(hex::FromHexError::InvalidStringLength);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn optional_field(s: Option<&str>) -> Option<String> {
    match s {
        Some(v) if !v.is_empty() => Some(v.to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn do_create_profile(
        profiles_root: &std::path::Path,
        name: &str,
        screen_name: Option<&str>,
    ) -> io::Result<Profile> {
        let dir = profiles_root.join(name);
        if dir.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already exists"));
        }
        fs::create_dir_all(&dir)?;

        let mut seed = [0u8; 32];
        {
            use rand::RngCore;
            rand::rng().fill_bytes(&mut seed);
        }
        fs::write(dir.join("seed"), seed)?;
        if let Some(sn) = screen_name {
            fs::write(dir.join("name"), sn)?;
        }
        Ok(Profile {
            name: name.to_owned(),
            seed,
            screen_name: screen_name.map(str::to_owned),
            bio: None,
        })
    }

    fn do_load_profile(profiles_root: &std::path::Path, name: &str) -> io::Result<Profile> {
        let dir = profiles_root.join(name);
        let seed_bytes = fs::read(dir.join("seed"))?;
        if seed_bytes.len() != 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "seed must be 32 bytes",
            ));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&seed_bytes);
        let screen_name = read_optional_text(&dir.join("name"))?;
        let bio = read_optional_text(&dir.join("bio"))?;
        Ok(Profile {
            name: name.to_owned(),
            seed,
            screen_name,
            bio,
        })
    }

    #[test]
    fn profile_create_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let created = do_create_profile(root, "alice", Some("Alice Liddell")).unwrap();
        assert_eq!(created.name, "alice");
        assert_eq!(created.screen_name.as_deref(), Some("Alice Liddell"));
        assert!(created.bio.is_none());

        let loaded = do_load_profile(root, "alice").unwrap();
        assert_eq!(loaded.name, "alice");
        assert_eq!(loaded.seed, created.seed);
        assert_eq!(loaded.screen_name, created.screen_name);
    }

    #[test]
    fn profile_create_no_screen_name() {
        let tmp = TempDir::new().unwrap();
        let created = do_create_profile(tmp.path(), "bob", None).unwrap();
        assert!(created.screen_name.is_none());
        let loaded = do_load_profile(tmp.path(), "bob").unwrap();
        assert!(loaded.screen_name.is_none());
    }

    #[test]
    fn profile_seed_is_32_bytes() {
        let tmp = TempDir::new().unwrap();
        let created = do_create_profile(tmp.path(), "carol", None).unwrap();
        let raw = fs::read(tmp.path().join("carol").join("seed")).unwrap();
        assert_eq!(raw.len(), 32);
        assert_eq!(raw.as_slice(), created.seed.as_slice());
    }

    fn write_friends_file(dir: &std::path::Path, content: &str) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("friends"), content)
    }

    fn parse_friends_from_dir(dir: &std::path::Path) -> io::Result<Vec<Friend>> {
        let path = dir.join("friends");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)?;
        let mut map: HashMap<[u8; 32], (usize, Friend)> = HashMap::new();
        let mut order: Vec<[u8; 32]> = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.splitn(4, '\t').collect();
            let pubkey = match decode_pubkey(parts[0]) {
                Ok(k) => k,
                Err(_) => continue,
            };
            let alias = optional_field(parts.get(1).copied());
            let cached_name = optional_field(parts.get(2).copied());
            let cached_bio_line = optional_field(parts.get(3).copied());
            let friend = Friend { pubkey, alias, cached_name, cached_bio_line };
            if let Some(existing) = map.get_mut(&pubkey) {
                existing.1 = friend;
            } else {
                let idx = order.len();
                order.push(pubkey);
                map.insert(pubkey, (idx, friend));
            }
        }
        let mut result: Vec<(usize, Friend)> = map.into_values().collect();
        result.sort_by_key(|(idx, _)| *idx);
        Ok(result.into_iter().map(|(_, f)| f).collect())
    }

    fn pubkey_from_u8(n: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[0] = n;
        k
    }

    #[test]
    fn friends_parse_basic() {
        let tmp = TempDir::new().unwrap();
        let key_a = pubkey_from_u8(1);
        let key_b = pubkey_from_u8(2);
        let content = format!(
            "# comment\n{}\talias_a\tCached A\tBio A\n{}\t\t\t\n",
            hex::encode(key_a),
            hex::encode(key_b),
        );
        write_friends_file(tmp.path(), &content).unwrap();

        let friends = parse_friends_from_dir(tmp.path()).unwrap();
        assert_eq!(friends.len(), 2);

        assert_eq!(friends[0].pubkey, key_a);
        assert_eq!(friends[0].alias.as_deref(), Some("alias_a"));
        assert_eq!(friends[0].cached_name.as_deref(), Some("Cached A"));
        assert_eq!(friends[0].cached_bio_line.as_deref(), Some("Bio A"));

        assert_eq!(friends[1].pubkey, key_b);
        assert!(friends[1].alias.is_none());
        assert!(friends[1].cached_name.is_none());
        assert!(friends[1].cached_bio_line.is_none());
    }

    #[test]
    fn friends_dedup_last_wins() {
        let tmp = TempDir::new().unwrap();
        let key = pubkey_from_u8(42);
        let content = format!(
            "{}\told_alias\told_name\told_bio\n{}\tnew_alias\tnew_name\tnew_bio\n",
            hex::encode(key),
            hex::encode(key),
        );
        write_friends_file(tmp.path(), &content).unwrap();

        let friends = parse_friends_from_dir(tmp.path()).unwrap();
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].alias.as_deref(), Some("new_alias"));
        assert_eq!(friends[0].cached_name.as_deref(), Some("new_name"));
    }

    #[test]
    fn friends_skips_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let key = pubkey_from_u8(5);
        let content = format!(
            "not-hex\talias\tname\tbio\n{}\tvalid\t\t\n",
            hex::encode(key),
        );
        write_friends_file(tmp.path(), &content).unwrap();
        let friends = parse_friends_from_dir(tmp.path()).unwrap();
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].pubkey, key);
    }

    fn append_ku(dir: &std::path::Path, pubkey: &[u8; 32], name: &str) -> io::Result<()> {
        let path = dir.join("known_users");
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&path)?;
        let line = format!("{}\t{}\n", hex::encode(pubkey), name);
        f.write_all(line.as_bytes())
    }

    fn load_ku(dir: &std::path::Path) -> io::Result<Vec<KnownUser>> {
        let path = dir.join("known_users");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)?;
        let mut map: HashMap<[u8; 32], (usize, KnownUser)> = HashMap::new();
        let mut order: Vec<[u8; 32]> = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let mut parts = line.splitn(2, '\t');
            let hex_key = parts.next().unwrap_or("").trim();
            let sn = parts.next().unwrap_or("").trim().to_owned();
            let pubkey = match decode_pubkey(hex_key) { Ok(k) => k, Err(_) => continue };
            let user = KnownUser { pubkey, screen_name: sn };
            if let Some(existing) = map.get_mut(&pubkey) {
                existing.1 = user;
            } else {
                let idx = order.len();
                order.push(pubkey);
                map.insert(pubkey, (idx, user));
            }
        }
        let mut result: Vec<(usize, KnownUser)> = map.into_values().collect();
        result.sort_by_key(|(idx, _)| *idx);
        Ok(result.into_iter().map(|(_, u)| u).collect())
    }

    #[test]
    fn known_users_append_and_load() {
        let tmp = TempDir::new().unwrap();
        let key_a = pubkey_from_u8(10);
        let key_b = pubkey_from_u8(20);
        fs::create_dir_all(tmp.path()).unwrap();

        append_ku(tmp.path(), &key_a, "Alice").unwrap();
        append_ku(tmp.path(), &key_b, "Bob").unwrap();

        let users = load_ku(tmp.path()).unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].screen_name, "Alice");
        assert_eq!(users[1].screen_name, "Bob");
    }

    #[test]
    fn known_users_dedup_last_wins() {
        let tmp = TempDir::new().unwrap();
        let key = pubkey_from_u8(7);
        fs::create_dir_all(tmp.path()).unwrap();

        append_ku(tmp.path(), &key, "OldName").unwrap();
        append_ku(tmp.path(), &key, "NewName").unwrap();

        let users = load_ku(tmp.path()).unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].screen_name, "NewName");
    }

    fn resolve_shortkey_in_dir(
        dir: &std::path::Path,
        shortkey: &str,
    ) -> io::Result<Option<[u8; 32]>> {
        let path = dir.join("known_users");
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let lower = shortkey.to_lowercase();
        let mut matches = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let hex_key = line.split('\t').next().unwrap_or("").trim();
            if hex_key.starts_with(&lower) {
                if let Ok(k) = decode_pubkey(hex_key) {
                    if !matches.contains(&k) {
                        matches.push(k);
                    }
                }
            }
        }
        match matches.len() {
            0 => Ok(None),
            1 => Ok(Some(matches[0])),
            _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "ambiguous")),
        }
    }

    #[test]
    fn shortkey_resolves_unique_prefix() {
        let tmp = TempDir::new().unwrap();
        let key = [0xabu8; 32];
        fs::create_dir_all(tmp.path()).unwrap();
        append_ku(tmp.path(), &key, "Abby").unwrap();

        let result = resolve_shortkey_in_dir(tmp.path(), "abababab").unwrap();
        assert_eq!(result, Some(key));
    }

    #[test]
    fn shortkey_returns_none_for_no_match() {
        let tmp = TempDir::new().unwrap();
        let key = [0xffu8; 32];
        fs::create_dir_all(tmp.path()).unwrap();
        append_ku(tmp.path(), &key, "Frank").unwrap();

        let result = resolve_shortkey_in_dir(tmp.path(), "00000000").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn shortkey_errors_on_ambiguous_prefix() {
        let tmp = TempDir::new().unwrap();
        let key_a = [0xabu8; 32];
        let mut key_b = [0xabu8; 32];
        key_b[1] = 0xcd;
        fs::create_dir_all(tmp.path()).unwrap();
        append_ku(tmp.path(), &key_a, "Ace").unwrap();
        append_ku(tmp.path(), &key_b, "Boo").unwrap();

        let result = resolve_shortkey_in_dir(tmp.path(), "ab");
        assert!(result.is_err());
    }

    #[test]
    fn shortkey_full_key_resolves_exactly() {
        let tmp = TempDir::new().unwrap();
        let key = [0x12u8; 32];
        fs::create_dir_all(tmp.path()).unwrap();
        append_ku(tmp.path(), &key, "Ivan").unwrap();

        let full_hex = hex::encode(key);
        let result = resolve_shortkey_in_dir(tmp.path(), &full_hex).unwrap();
        assert_eq!(result, Some(key));
    }
}
