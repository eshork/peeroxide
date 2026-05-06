use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cmd::chat::profile::Friend;

pub struct DisplayMessage {
    pub id_pubkey: [u8; 32],
    pub screen_name: String,
    pub content: String,
    pub timestamp: u64,
    pub is_self: bool,
}

pub struct DisplayState {
    friends: HashMap<[u8; 32], Friend>,
    last_identity_shown: HashMap<[u8; 32], u64>,
    known_names: HashMap<[u8; 32], String>,
    name_change_at: HashMap<[u8; 32], u64>,
}

impl DisplayState {
    pub fn new(friends: Vec<Friend>) -> Self {
        let friends_map: HashMap<[u8; 32], Friend> =
            friends.into_iter().map(|f| (f.pubkey, f)).collect();
        Self {
            friends: friends_map,
            last_identity_shown: HashMap::new(),
            known_names: HashMap::new(),
            name_change_at: HashMap::new(),
        }
    }

    /// Reload the friends map from the given list.
    /// Called periodically to pick up alias edits and nexus name refreshes.
    pub fn reload_friends(&mut self, friends: Vec<Friend>) {
        self.friends = friends.into_iter().map(|f| (f.pubkey, f)).collect();
    }

    pub fn render(&mut self, msg: &DisplayMessage) {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let timestamp_str = format_timestamp(msg.timestamp);
        let display_name = self.format_display_name(msg, now_secs);

        if self.should_show_identity(msg, now_secs) {
            let shortkey = &hex::encode(msg.id_pubkey)[..8];
            let fullkey = hex::encode(msg.id_pubkey);
            eprintln!("*** @{shortkey} is {fullkey}");
            self.last_identity_shown.insert(msg.id_pubkey, now_secs);
        }

        println!("[{timestamp_str}] [{display_name}]: {}", msg.content);

        if !msg.screen_name.is_empty() {
            let prev = self.known_names.get(&msg.id_pubkey);
            if let Some(old_name) = prev {
                if old_name.as_str() != msg.screen_name {
                    let shortkey = &hex::encode(msg.id_pubkey)[..8];
                    eprintln!(
                        "*** {}@{} changed screen name: \"{}\" → \"{}\"",
                        old_name, shortkey, old_name, msg.screen_name
                    );
                    self.name_change_at.insert(msg.id_pubkey, now_secs);
                }
            }
            self.known_names
                .insert(msg.id_pubkey, msg.screen_name.clone());
        }
    }

    fn format_display_name(&self, msg: &DisplayMessage, now_secs: u64) -> String {
        let shortkey = &hex::encode(msg.id_pubkey)[..8];

        let name_cooldown_active = self
            .name_change_at
            .get(&msg.id_pubkey)
            .map(|&t| now_secs.saturating_sub(t) < 300)
            .unwrap_or(false);
        let bang = if name_cooldown_active { "!" } else { "" };

        if let Some(friend) = self.friends.get(&msg.id_pubkey) {
            if let Some(ref alias) = friend.alias {
                if msg.screen_name.is_empty() || *alias == msg.screen_name {
                    format!("({alias}){bang}")
                } else {
                    format!("({alias}) <{}>{bang}", msg.screen_name)
                }
            } else if !msg.screen_name.is_empty() {
                format!("({}){bang}", msg.screen_name)
            } else {
                format!("(@{shortkey}){bang}")
            }
        } else if !msg.screen_name.is_empty() {
            format!("<{}@{}>{bang}", msg.screen_name, shortkey)
        } else {
            format!("<@{shortkey}>{bang}")
        }
    }

    fn should_show_identity(&self, msg: &DisplayMessage, now_secs: u64) -> bool {
        if msg.is_self {
            return false;
        }
        if let Some(friend) = self.friends.get(&msg.id_pubkey) {
            if friend.alias.is_some() {
                return false;
            }
        }
        match self.last_identity_shown.get(&msg.id_pubkey) {
            Some(&last) => now_secs.saturating_sub(last) > 600,
            None => true,
        }
    }
}

fn format_timestamp(unix_secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let secs = unix_secs;
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;

    let today_start = now - (now % 86400);
    if secs >= today_start {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        let days = secs / 86400;
        let y = 1970 + (days / 365);
        let d = days % 365;
        let mo = d / 30 + 1;
        let day = d % 30 + 1;
        format!("{y}-{mo:02}-{day:02} {h:02}:{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_display_name_friend_with_alias() {
        let friend = Friend {
            pubkey: [1u8; 32],
            alias: Some("alice".to_string()),
            cached_name: None,
            cached_bio_line: None,
        };
        let state = DisplayState::new(vec![friend]);
        let msg = DisplayMessage {
            id_pubkey: [1u8; 32],
            screen_name: "alice".to_string(),
            content: "hi".to_string(),
            timestamp: 0,
            is_self: false,
        };
        let name = state.format_display_name(&msg, 0);
        assert_eq!(name, "(alice)");
    }

    #[test]
    fn format_display_name_non_friend() {
        let state = DisplayState::new(vec![]);
        let msg = DisplayMessage {
            id_pubkey: [0xab; 32],
            screen_name: "bob".to_string(),
            content: "hi".to_string(),
            timestamp: 0,
            is_self: false,
        };
        let name = state.format_display_name(&msg, 0);
        assert!(name.starts_with("<bob@"));
        assert!(name.ends_with('>'));
    }

    #[test]
    fn format_display_name_with_name_change_cooldown() {
        let mut state = DisplayState::new(vec![]);
        state.known_names.insert([0xab; 32], "old_name".to_string());
        state.name_change_at.insert([0xab; 32], 1000);

        let msg = DisplayMessage {
            id_pubkey: [0xab; 32],
            screen_name: "new_name".to_string(),
            content: "hi".to_string(),
            timestamp: 0,
            is_self: false,
        };
        let name_during_cooldown = state.format_display_name(&msg, 1100);
        assert!(name_during_cooldown.ends_with('!'), "should show ! during 300s cooldown");

        let name_after_cooldown = state.format_display_name(&msg, 1400);
        assert!(!name_after_cooldown.ends_with('!'), "should NOT show ! after 300s");
    }
}
