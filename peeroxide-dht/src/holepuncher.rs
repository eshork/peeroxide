use std::net::SocketAddr;

use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use crate::hyperdht_messages::{FIREWALL_CONSISTENT, FIREWALL_RANDOM, FIREWALL_UNKNOWN};
use crate::messages::Ipv4Peer;
use crate::nat::Nat;
use crate::socket_pool::{coerce_firewall, random_port, SocketPool, SocketRef};

const BIRTHDAY_SOCKETS: usize = 256;

#[non_exhaustive]
pub enum HolepunchEvent {
    Connected {
        addr: SocketAddr,
    },
    Aborted,
}

pub struct RemoteAddress {
    pub host: String,
    pub port: u16,
    pub verified: bool,
}

pub struct Holepuncher {
    pub nat: Nat,
    pub is_initiator: bool,
    pub punching: bool,
    pub connected: bool,
    pub destroyed: bool,

    pub remote_firewall: u64,
    pub remote_addresses: Vec<RemoteAddress>,
    pub remote_holepunching: bool,

    sockets: Vec<SocketRef>,
    event_tx: mpsc::UnboundedSender<HolepunchEvent>,
}

impl Holepuncher {
    pub async fn new(
        pool: &SocketPool,
        runtime: &libudx::UdxRuntime,
        firewalled: bool,
        is_initiator: bool,
        remote_firewall: u64,
        event_tx: mpsc::UnboundedSender<HolepunchEvent>,
    ) -> Result<Self, crate::socket_pool::SocketPoolError> {
        let socket = pool.acquire(runtime).await?;

        Ok(Self {
            nat: Nat::new(firewalled),
            is_initiator,
            punching: false,
            connected: false,
            destroyed: false,
            remote_firewall,
            remote_addresses: Vec::new(),
            remote_holepunching: false,
            sockets: vec![socket],
            event_tx,
        })
    }

    pub fn update_remote(&mut self, punching: bool, firewall: u64, addresses: &[Ipv4Peer], verified_host: Option<&str>) {
        let mut remote_addrs = Vec::new();
        for addr in addresses {
            let is_verified = verified_host == Some(addr.host.as_str())
                || self.is_verified(&addr.host);
            remote_addrs.push(RemoteAddress {
                host: addr.host.clone(),
                port: addr.port,
                verified: is_verified,
            });
        }
        self.remote_firewall = firewall;
        self.remote_addresses = remote_addrs;
        self.remote_holepunching = punching;
    }

    fn is_verified(&self, host: &str) -> bool {
        self.remote_addresses.iter().any(|a| a.verified && a.host == host)
    }

    pub fn primary_socket(&self) -> Option<&SocketRef> {
        self.sockets.first()
    }

    fn unstable(&self) -> bool {
        let fw = self.nat.firewall;
        (self.remote_firewall >= FIREWALL_RANDOM && fw >= FIREWALL_RANDOM)
            || fw == FIREWALL_UNKNOWN
    }

    pub async fn analyze(&mut self, _allow_reopen: bool) -> bool {
        // NAT analysis is driven by add() calls from ping responses.
        // In a full implementation, we'd await nat.analyzing here.
        // For now, check current state.
        !self.unstable()
    }

    pub async fn punch(
        &mut self,
        pool: &SocketPool,
        runtime: &libudx::UdxRuntime,
    ) -> bool {
        if self.done() || self.remote_addresses.is_empty() {
            return false;
        }

        self.punching = true;

        let local = coerce_firewall(self.nat.firewall);
        let remote = coerce_firewall(self.remote_firewall);

        let verified = self.first_verified_address();

        if local == FIREWALL_CONSISTENT && remote == FIREWALL_CONSISTENT {
            self.consistent_probe().await;
            return true;
        }

        let Some(verified_addr) = verified else {
            return false;
        };

        if local == FIREWALL_CONSISTENT && remote >= FIREWALL_RANDOM {
            self.random_probes(&verified_addr).await;
            return true;
        }

        if local >= FIREWALL_RANDOM && remote == FIREWALL_CONSISTENT {
            self.open_birthday_sockets(pool, runtime, &verified_addr).await;
            if self.punching {
                self.keep_alive_random_nat(&verified_addr).await;
            }
            return true;
        }

        false
    }

    fn first_verified_address(&self) -> Option<Ipv4Peer> {
        self.remote_addresses.iter().find(|a| a.verified).map(|a| Ipv4Peer {
            host: a.host.clone(),
            port: a.port,
        })
    }

    async fn consistent_probe(&mut self) {
        if !self.is_initiator {
            sleep(Duration::from_secs(1)).await;
        }

        for tries in 0..10 {
            if !self.punching {
                break;
            }

            for addr in &self.remote_addresses {
                if !addr.verified && (tries & 3) != 0 {
                    continue;
                }
                let target: SocketAddr = match format!("{}:{}", addr.host, addr.port).parse() {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                if let Some(socket) = self.sockets.first() {
                    let _ = socket.send_holepunch(target, false);
                }
            }

            if self.punching {
                sleep(Duration::from_secs(1)).await;
            }
        }

        self.auto_destroy();
    }

    async fn random_probes(&mut self, remote_addr: &Ipv4Peer) {
        for _ in 0..1750 {
            if !self.punching {
                break;
            }

            let port = random_port();
            let target: SocketAddr = match format!("{}:{port}", remote_addr.host).parse() {
                Ok(a) => a,
                Err(_) => continue,
            };
            if let Some(socket) = self.sockets.first() {
                let _ = socket.send_holepunch(target, false);
            }

            if self.punching {
                sleep(Duration::from_millis(20)).await;
            }
        }

        self.auto_destroy();
    }

    async fn open_birthday_sockets(
        &mut self,
        pool: &SocketPool,
        runtime: &libudx::UdxRuntime,
        remote_addr: &Ipv4Peer,
    ) {
        let target: SocketAddr = match format!("{}:{}", remote_addr.host, remote_addr.port).parse() {
            Ok(a) => a,
            Err(_) => return,
        };

        while self.punching && self.sockets.len() < BIRTHDAY_SOCKETS {
            match pool.acquire(runtime).await {
                Ok(socket) => {
                    let _ = socket.send_holepunch(target, true);
                    self.sockets.push(socket);
                }
                Err(_) => break,
            }
        }
    }

    async fn keep_alive_random_nat(&mut self, remote_addr: &Ipv4Peer) {
        let target: SocketAddr = match format!("{}:{}", remote_addr.host, remote_addr.port).parse() {
            Ok(a) => a,
            Err(_) => return,
        };

        sleep(Duration::from_millis(100)).await;

        let mut i = 0;
        let mut low_ttl_rounds: u32 = 1;

        for _ in 0..1750 {
            if !self.punching {
                break;
            }

            if i == self.sockets.len() {
                i = 0;
                low_ttl_rounds = low_ttl_rounds.saturating_sub(1);
            }

            if let Some(socket) = self.sockets.get(i) {
                let _ = socket.send_holepunch(target, low_ttl_rounds > 0);
            }
            i += 1;

            if self.punching {
                sleep(Duration::from_millis(20)).await;
            }
        }

        self.auto_destroy();
    }

    pub fn on_holepunch_message(&mut self, addr: SocketAddr, socket_idx: usize) {
        if !self.is_initiator {
            if let Some(socket) = self.sockets.get(socket_idx) {
                let _ = socket.send_holepunch(addr, false);
            }
            return;
        }

        if self.connected {
            return;
        }

        self.connected = true;
        self.punching = false;

        let _ = self.event_tx.send(HolepunchEvent::Connected { addr });
    }

    fn done(&self) -> bool {
        self.destroyed || self.connected
    }

    fn auto_destroy(&mut self) {
        if !self.connected {
            self.destroy();
        }
    }

    pub fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        self.punching = false;
        self.sockets.clear();
        self.nat.destroy();

        if !self.connected {
            let _ = self.event_tx.send(HolepunchEvent::Aborted);
        }
    }
}

pub fn match_address(local_addresses: &[Ipv4Peer], remote_local_addresses: &[Ipv4Peer]) -> Option<Ipv4Peer> {
    if remote_local_addresses.is_empty() {
        return None;
    }

    let mut best_segment = 1u8;
    let mut best_addr: Option<&Ipv4Peer> = None;

    for local in local_addresses {
        let a: Vec<&str> = local.host.split('.').collect();
        if a.len() != 4 {
            continue;
        }

        for remote in remote_local_addresses {
            let b: Vec<&str> = remote.host.split('.').collect();
            if b.len() != 4 {
                continue;
            }

            if a[0] == b[0] {
                if best_segment == 1 {
                    best_segment = 2;
                    best_addr = Some(remote);
                }

                if a[1] == b[1] {
                    if best_segment == 2 {
                        best_segment = 3;
                        best_addr = Some(remote);
                    }

                    if a[2] == b[2] {
                        return Some(remote.clone());
                    }
                }
            }
        }
    }

    best_addr.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(host: &str, port: u16) -> Ipv4Peer {
        Ipv4Peer {
            host: host.into(),
            port,
        }
    }

    #[test]
    fn match_address_exact_subnet() {
        let local = vec![peer("192.168.1.100", 5000)];
        let remote = vec![peer("192.168.1.50", 6000)];
        let result = match_address(&local, &remote).unwrap();
        assert_eq!(result.host, "192.168.1.50");
    }

    #[test]
    fn match_address_partial_subnet() {
        let local = vec![peer("192.168.1.100", 5000)];
        let remote = vec![peer("192.168.2.50", 6000)];
        let result = match_address(&local, &remote).unwrap();
        assert_eq!(result.host, "192.168.2.50");
    }

    #[test]
    fn match_address_different_network() {
        let local = vec![peer("192.168.1.100", 5000)];
        let remote = vec![peer("10.0.0.1", 6000)];
        assert!(match_address(&local, &remote).is_none());
    }

    #[test]
    fn match_address_empty_remote() {
        let local = vec![peer("192.168.1.100", 5000)];
        assert!(match_address(&local, &[]).is_none());
    }

    #[test]
    fn match_address_picks_closest() {
        let local = vec![peer("192.168.1.100", 5000)];
        let remote = vec![
            peer("192.168.2.50", 6000),
            peer("192.168.1.99", 7000),
        ];
        let result = match_address(&local, &remote).unwrap();
        assert_eq!(result.host, "192.168.1.99");
    }

    #[test]
    fn match_address_first_octet_only() {
        let local = vec![peer("10.0.0.1", 5000)];
        let remote = vec![peer("10.1.2.3", 6000)];
        let result = match_address(&local, &remote).unwrap();
        assert_eq!(result.host, "10.1.2.3");
    }
}
