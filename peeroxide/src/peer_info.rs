use std::time::Instant;

use peeroxide_dht::messages::Ipv4Peer;

const PROVEN_THRESHOLD_SECS: u64 = 15;

/// Peer connection priority level, matching Node.js Hyperswarm semantics.
///
/// Selection logic in [`PeerInfo::get_priority`]:
/// - 0 attempts → Normal
/// - 1 attempt (proven) → VeryHigh, (unproven) → Normal
/// - 2 attempts (proven) → High, (unproven) → Low
/// - 3 attempts → Low
/// - 4+ attempts → VeryLow
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Priority {
    VeryLow = 0,
    Low = 1,
    Normal = 2,
    High = 3,
    VeryHigh = 4,
}

/// Per-peer metadata tracking connection attempts, priority, and topic associations.
///
/// Modelled after `lib/peer-info.js` in the Node.js Hyperswarm.
pub struct PeerInfo {
    pub public_key: [u8; 32],
    pub relay_addresses: Vec<Ipv4Peer>,
    pub reconnecting: bool,
    pub proven: bool,
    pub banned: bool,
    pub queued: bool,
    pub connecting: bool,
    pub explicit: bool,
    pub topics: Vec<[u8; 32]>,
    pub attempts: u32,
    pub priority: Priority,
    connected_at: Option<Instant>,
    waiting: bool,
}

impl PeerInfo {
    pub fn new(public_key: [u8; 32], relay_addresses: Vec<Ipv4Peer>) -> Self {
        Self {
            public_key,
            relay_addresses,
            reconnecting: false,
            proven: false,
            banned: false,
            queued: false,
            connecting: false,
            explicit: false,
            topics: Vec::new(),
            attempts: 0,
            priority: Priority::Normal,
            connected_at: None,
            waiting: false,
        }
    }

    /// Compute priority based on attempt count and proven status.
    pub fn get_priority(&self) -> Priority {
        match self.attempts {
            0 => Priority::Normal,
            1 => {
                if self.proven {
                    Priority::VeryHigh
                } else {
                    Priority::Normal
                }
            }
            2 => {
                if self.proven {
                    Priority::High
                } else {
                    Priority::Low
                }
            }
            3 => Priority::Low,
            _ => Priority::VeryLow,
        }
    }

    pub fn connected(&mut self) {
        self.reconnecting = false;
        self.connected_at = Some(Instant::now());
    }

    /// If the connection lasted ≥ 15s, mark as proven and reset attempts.
    pub fn disconnected(&mut self) {
        if let Some(at) = self.connected_at {
            if at.elapsed().as_secs() >= PROVEN_THRESHOLD_SECS {
                self.reconnecting = true;
                self.attempts = 0;
                self.proven = true;
            }
        }
    }

    pub fn should_gc(&self) -> bool {
        if self.banned || self.queued || self.explicit || self.waiting {
            return false;
        }
        self.topics.is_empty()
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    pub fn set_waiting(&mut self, w: bool) {
        self.waiting = w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_initial() {
        let info = PeerInfo::new([0u8; 32], vec![]);
        assert_eq!(info.get_priority(), Priority::Normal);
    }

    #[test]
    fn priority_after_one_attempt_unproven() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.attempts = 1;
        assert_eq!(info.get_priority(), Priority::Normal);
    }

    #[test]
    fn priority_after_one_attempt_proven() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.attempts = 1;
        info.proven = true;
        assert_eq!(info.get_priority(), Priority::VeryHigh);
    }

    #[test]
    fn priority_after_two_attempts_unproven() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.attempts = 2;
        assert_eq!(info.get_priority(), Priority::Low);
    }

    #[test]
    fn priority_after_two_attempts_proven() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.attempts = 2;
        info.proven = true;
        assert_eq!(info.get_priority(), Priority::High);
    }

    #[test]
    fn priority_after_three_attempts() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.attempts = 3;
        assert_eq!(info.get_priority(), Priority::Low);
    }

    #[test]
    fn priority_after_many_attempts() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.attempts = 10;
        assert_eq!(info.get_priority(), Priority::VeryLow);
    }

    #[test]
    fn should_gc_empty_topics() {
        let info = PeerInfo::new([0u8; 32], vec![]);
        assert!(info.should_gc());
    }

    #[test]
    fn should_gc_with_topics() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.topics.push([1u8; 32]);
        assert!(!info.should_gc());
    }

    #[test]
    fn should_gc_banned() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.banned = true;
        assert!(!info.should_gc());
    }

    #[test]
    fn should_gc_queued() {
        let mut info = PeerInfo::new([0u8; 32], vec![]);
        info.queued = true;
        assert!(!info.should_gc());
    }

    #[test]
    fn priority_ordering() {
        assert!(Priority::VeryHigh > Priority::High);
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert!(Priority::Low > Priority::VeryLow);
    }
}
