//! Single source of truth for resolving a pubkey to a human-readable name.
//!
//! Before this module the same precedence ladder (friend alias → in-flight
//! screen_name → cached known-users screen_name → deterministic vendor
//! name → 8-char shortkey) was duplicated across several call sites:
//! `display::DisplayState::format_display_name`, `inbox_monitor::
//! format_invite_lines`, the various slash-command output paths, and the
//! DM bar name. Each had slightly different framing rules and small
//! inconsistencies.
//!
//! [`NameResolver`] centralises the lookup; callers compose the framing
//! they need from [`ResolvedName`]'s components or its format helpers.
//!
//! The resolver is purely message-agnostic — it takes only a pubkey. The
//! chat-message rendering path layers msg-specific behaviour (the
//! `msg.screen_name` override, the cooldown `!` bang, the `(alias) <name>`
//! framing) on top of this base resolver.

use crate::cmd::chat::known_users::KnownUser;
use crate::cmd::chat::names;
use crate::cmd::chat::profile::Friend;

/// Which source produced the resolved name. Callers use this to pick
/// suitable framing — e.g. friend aliases show bare in compact contexts,
/// while screen / vendor names get the `@shortkey` suffix to disambiguate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSource {
    /// Matched a `Friend.alias` in the user's profile-local friends file.
    FriendAlias,
    /// Matched a `screen_name` in the shared `known_users` cache (i.e.
    /// the pubkey has authored at least one message we've seen).
    KnownScreenName,
    /// Fell through to the deterministic vendor name derived from the
    /// pubkey. Always available.
    VendorName,
}

/// Outcome of a name resolution. Carries the components separately so
/// callers can compose them into a label of their choice; the
/// [`Self::bar_label`] / [`Self::formal`] helpers cover the common cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    /// The primary name segment (no decoration). Examples:
    ///   FriendAlias       → "alice"
    ///   KnownScreenName   → "alice"
    ///   VendorName        → "tyrannical_elbakyan"
    pub name: String,
    /// 8-char hex shortkey suffix (`hex::encode(pubkey)[..8]`). Always
    /// populated regardless of source.
    pub shortkey: String,
    pub source: NameSource,
}

impl ResolvedName {
    /// Compact label suitable for narrow contexts (status bar, single-line
    /// summaries). Friend aliases show bare; everything else gets the
    /// `@shortkey` suffix for disambiguation.
    ///
    /// - `FriendAlias`     → `"alice"`
    /// - `KnownScreenName` → `"alice@abc12345"`
    /// - `VendorName`      → `"tyrannical_elbakyan@abc12345"`
    pub fn bar_label(&self) -> String {
        match self.source {
            NameSource::FriendAlias => self.name.clone(),
            NameSource::KnownScreenName | NameSource::VendorName => {
                format!("{}@{}", self.name, self.shortkey)
            }
        }
    }

    /// "Formal" label suitable for system notices / verbose output where
    /// disambiguation matters: always `"<name> (<shortkey>)"`. Friend
    /// aliases still benefit from the parenthesised shortkey here so the
    /// user can verify they're acting on the expected identity.
    ///
    /// - any source → `"alice (abc12345)"`
    pub fn formal(&self) -> String {
        format!("{} ({})", self.name, self.shortkey)
    }

    /// True when the name came from a friend alias — useful for callers
    /// that want to skip a `*** vendor@short is fullkey` identity notice
    /// when the user has already aliased the sender.
    pub fn is_friend(&self) -> bool {
        matches!(self.source, NameSource::FriendAlias)
    }

    /// True when the resolver only had the deterministic vendor name to
    /// fall back on — i.e. no friend alias and no cached screen name.
    pub fn is_vendor_fallback(&self) -> bool {
        matches!(self.source, NameSource::VendorName)
    }
}

/// Resolver bound to a particular friends list + known-users snapshot.
/// Cheap to construct from borrowed slices; doesn't allocate until
/// `resolve` is called.
pub struct NameResolver<'a> {
    friends: &'a [Friend],
    known_users: &'a [KnownUser],
}

impl<'a> NameResolver<'a> {
    pub fn new(friends: &'a [Friend], known_users: &'a [KnownUser]) -> Self {
        Self {
            friends,
            known_users,
        }
    }

    /// Resolver with no friends list (caller has nothing loaded). Falls
    /// through to known-users / vendor name resolution.
    pub fn from_known_users(known_users: &'a [KnownUser]) -> Self {
        Self {
            friends: &[],
            known_users,
        }
    }

    /// Apply the precedence ladder to one pubkey:
    /// 1. Friend with non-empty alias → `FriendAlias`.
    /// 2. Known user with non-empty screen_name → `KnownScreenName`.
    /// 3. Deterministic vendor name from `names::generate_name_from_seed`
    ///    → `VendorName`.
    pub fn resolve(&self, pubkey: &[u8; 32]) -> ResolvedName {
        let shortkey = hex::encode(pubkey)[..8].to_string();

        if let Some(friend) = self.friends.iter().find(|f| f.pubkey == *pubkey)
            && let Some(alias) = friend.alias.as_ref()
            && !alias.is_empty()
        {
            return ResolvedName {
                name: alias.clone(),
                shortkey,
                source: NameSource::FriendAlias,
            };
        }

        if let Some(user) = self.known_users.iter().find(|u| u.pubkey == *pubkey)
            && !user.screen_name.is_empty()
        {
            return ResolvedName {
                name: user.screen_name.clone(),
                shortkey,
                source: NameSource::KnownScreenName,
            };
        }

        ResolvedName {
            name: names::generate_name_from_seed(pubkey),
            shortkey,
            source: NameSource::VendorName,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(pubkey_byte: u8, alias: Option<&str>) -> Friend {
        Friend {
            pubkey: [pubkey_byte; 32],
            alias: alias.map(|s| s.to_string()),
            cached_name: None,
            cached_bio_line: None,
        }
    }

    fn known(pubkey_byte: u8, screen_name: &str) -> KnownUser {
        KnownUser {
            pubkey: [pubkey_byte; 32],
            screen_name: screen_name.to_string(),
        }
    }

    #[test]
    fn friend_alias_wins_over_known_screen_name() {
        let friends = vec![friend(0x42, Some("Alice"))];
        let knowns = vec![known(0x42, "alice_v2")];
        let r = NameResolver::new(&friends, &knowns);
        let resolved = r.resolve(&[0x42; 32]);
        assert_eq!(resolved.name, "Alice");
        assert_eq!(resolved.source, NameSource::FriendAlias);
    }

    #[test]
    fn known_screen_name_used_when_no_friend_alias() {
        let friends: Vec<Friend> = vec![];
        let knowns = vec![known(0x42, "alice")];
        let r = NameResolver::new(&friends, &knowns);
        let resolved = r.resolve(&[0x42; 32]);
        assert_eq!(resolved.name, "alice");
        assert_eq!(resolved.source, NameSource::KnownScreenName);
    }

    #[test]
    fn friend_without_alias_falls_through_to_known() {
        // Friend exists but has no alias — known cache should still resolve.
        let friends = vec![friend(0x42, None)];
        let knowns = vec![known(0x42, "alice")];
        let r = NameResolver::new(&friends, &knowns);
        let resolved = r.resolve(&[0x42; 32]);
        assert_eq!(resolved.name, "alice");
        assert_eq!(resolved.source, NameSource::KnownScreenName);
    }

    #[test]
    fn friend_with_empty_alias_falls_through() {
        let friends = vec![friend(0x42, Some(""))];
        let knowns = vec![known(0x42, "alice")];
        let r = NameResolver::new(&friends, &knowns);
        let resolved = r.resolve(&[0x42; 32]);
        assert_eq!(resolved.source, NameSource::KnownScreenName);
    }

    #[test]
    fn vendor_name_fallback_when_unknown() {
        let friends: Vec<Friend> = vec![];
        let knowns: Vec<KnownUser> = vec![];
        let r = NameResolver::new(&friends, &knowns);
        let resolved = r.resolve(&[0xab; 32]);
        assert_eq!(resolved.source, NameSource::VendorName);
        assert!(!resolved.name.is_empty(), "vendor name should be non-empty");
        assert_eq!(resolved.shortkey, "abababab");
    }

    #[test]
    fn known_with_empty_screen_name_falls_through_to_vendor() {
        let friends: Vec<Friend> = vec![];
        let knowns = vec![known(0x42, "")];
        let r = NameResolver::new(&friends, &knowns);
        let resolved = r.resolve(&[0x42; 32]);
        assert_eq!(resolved.source, NameSource::VendorName);
    }

    #[test]
    fn shortkey_always_populated() {
        let r = NameResolver::from_known_users(&[]);
        let resolved = r.resolve(&[0x01; 32]);
        assert_eq!(resolved.shortkey.len(), 8);
        assert_eq!(resolved.shortkey, "01010101");
    }

    #[test]
    fn from_known_users_constructor_implies_no_friends() {
        let knowns = vec![known(0x42, "alice")];
        let r = NameResolver::from_known_users(&knowns);
        let resolved = r.resolve(&[0x42; 32]);
        assert_eq!(resolved.source, NameSource::KnownScreenName);
    }

    #[test]
    fn bar_label_friend_is_bare() {
        let resolved = ResolvedName {
            name: "alice".to_string(),
            shortkey: "abc12345".to_string(),
            source: NameSource::FriendAlias,
        };
        assert_eq!(resolved.bar_label(), "alice");
    }

    #[test]
    fn bar_label_known_includes_short() {
        let resolved = ResolvedName {
            name: "alice".to_string(),
            shortkey: "abc12345".to_string(),
            source: NameSource::KnownScreenName,
        };
        assert_eq!(resolved.bar_label(), "alice@abc12345");
    }

    #[test]
    fn bar_label_vendor_includes_short() {
        let resolved = ResolvedName {
            name: "tyrannical_elbakyan".to_string(),
            shortkey: "abc12345".to_string(),
            source: NameSource::VendorName,
        };
        assert_eq!(resolved.bar_label(), "tyrannical_elbakyan@abc12345");
    }

    #[test]
    fn formal_always_paren_short() {
        for source in [
            NameSource::FriendAlias,
            NameSource::KnownScreenName,
            NameSource::VendorName,
        ] {
            let resolved = ResolvedName {
                name: "alice".to_string(),
                shortkey: "abc12345".to_string(),
                source,
            };
            assert_eq!(resolved.formal(), "alice (abc12345)");
        }
    }

    #[test]
    fn source_predicates() {
        let friend = ResolvedName {
            name: "a".into(),
            shortkey: "00000000".into(),
            source: NameSource::FriendAlias,
        };
        let vendor = ResolvedName {
            name: "a".into(),
            shortkey: "00000000".into(),
            source: NameSource::VendorName,
        };
        let known = ResolvedName {
            name: "a".into(),
            shortkey: "00000000".into(),
            source: NameSource::KnownScreenName,
        };
        assert!(friend.is_friend());
        assert!(!vendor.is_friend());
        assert!(!known.is_friend());
        assert!(vendor.is_vendor_fallback());
        assert!(!friend.is_vendor_fallback());
        assert!(!known.is_vendor_fallback());
    }
}
