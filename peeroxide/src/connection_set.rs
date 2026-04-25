use std::collections::HashMap;

pub(crate) struct ConnectionInfo {
    pub is_initiator: bool,
}

pub(crate) struct ConnectionSet {
    by_public_key: HashMap<[u8; 32], ConnectionInfo>,
}

impl ConnectionSet {
    pub fn new() -> Self {
        Self {
            by_public_key: HashMap::new(),
        }
    }

    pub fn has(&self, public_key: &[u8; 32]) -> bool {
        self.by_public_key.contains_key(public_key)
    }

    pub fn get(&self, public_key: &[u8; 32]) -> Option<&ConnectionInfo> {
        self.by_public_key.get(public_key)
    }

    pub fn add(&mut self, public_key: [u8; 32], info: ConnectionInfo) {
        self.by_public_key.insert(public_key, info);
    }

    pub fn remove(&mut self, public_key: &[u8; 32]) -> bool {
        self.by_public_key.remove(public_key).is_some()
    }

    pub fn len(&self) -> usize {
        self.by_public_key.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_has() {
        let mut set = ConnectionSet::new();
        let pk = [1u8; 32];
        assert!(!set.has(&pk));
        set.add(pk, ConnectionInfo { is_initiator: true });
        assert!(set.has(&pk));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn remove() {
        let mut set = ConnectionSet::new();
        let pk = [2u8; 32];
        set.add(pk, ConnectionInfo { is_initiator: false });
        assert!(set.remove(&pk));
        assert!(!set.has(&pk));
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn get_info() {
        let mut set = ConnectionSet::new();
        let pk = [3u8; 32];
        set.add(pk, ConnectionInfo { is_initiator: true });
        let info = set.get(&pk).expect("should exist");
        assert!(info.is_initiator);
    }
}
