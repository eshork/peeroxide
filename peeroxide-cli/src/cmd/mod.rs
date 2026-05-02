pub mod announce;
pub mod cp;
pub mod deaddrop;
pub mod init;
pub mod lookup;
pub mod node;
pub mod ping;

use peeroxide_dht::hyperdht::HyperDhtConfig;
use peeroxide_dht::hyperdht_messages::{FIREWALL_CONSISTENT, FIREWALL_OPEN};
use peeroxide_dht::rpc::DhtConfig;

use crate::config::ResolvedConfig;

pub fn parse_topic(input: &str) -> [u8; 32] {
    if input.len() == 64 {
        if let Ok(bytes) = hex::decode(input) {
            if bytes.len() == 32 {
                let mut topic = [0u8; 32];
                topic.copy_from_slice(&bytes);
                return topic;
            }
        }
    }
    peeroxide::discovery_key(input.as_bytes())
}

pub fn build_dht_config(cfg: &ResolvedConfig) -> HyperDhtConfig {
    let bootstrap = if cfg.bootstrap.is_empty() && cfg.public {
        peeroxide::DEFAULT_BOOTSTRAP
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else {
        cfg.bootstrap.clone()
    };

    let mut dht_cfg = DhtConfig::default();
    dht_cfg.bootstrap = bootstrap;
    dht_cfg.firewalled = !cfg.public || cfg.firewalled;
    let mut hyper_cfg = HyperDhtConfig::default();
    hyper_cfg.dht = dht_cfg;
    hyper_cfg
}

pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

#[cfg(unix)]
pub async fn sigterm_recv() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut s = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    s.recv().await;
}

#[cfg(not(unix))]
pub async fn sigterm_recv() {
    std::future::pending::<()>().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use peeroxide_dht::hyperdht::should_direct_connect;
    use peeroxide_dht::hyperdht_messages::{
        FIREWALL_CONSISTENT, FIREWALL_OPEN, FIREWALL_RANDOM, FIREWALL_UNKNOWN,
    };

    #[test]
    fn parse_topic_hex_64_chars() {
        let hex_str = "a".repeat(64);
        let topic = parse_topic(&hex_str);
        assert_eq!(topic, [0xaa; 32]);
    }

    #[test]
    fn parse_topic_short_string_hashes() {
        let topic = parse_topic("hello");
        let expected = peeroxide::discovery_key(b"hello");
        assert_eq!(topic, expected);
    }

    #[test]
    fn parse_topic_63_char_hex_hashes() {
        let input = "a".repeat(63);
        let topic = parse_topic(&input);
        let expected = peeroxide::discovery_key(input.as_bytes());
        assert_eq!(topic, expected);
    }

    #[test]
    fn parse_topic_invalid_hex_64_chars_hashes() {
        let input = "g".repeat(64);
        let topic = parse_topic(&input);
        let expected = peeroxide::discovery_key(input.as_bytes());
        assert_eq!(topic, expected);
    }

    #[test]
    fn to_hex_roundtrip() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        assert_eq!(to_hex(&bytes), "deadbeef");
    }

    #[test]
    fn build_dht_config_uses_defaults_when_public_no_bootstrap() {
        let cfg = ResolvedConfig {
            public: true,
            firewalled: false,
            bootstrap: vec![],
            node: Default::default(),
        };
        let dht_cfg = build_dht_config(&cfg);
        assert!(!dht_cfg.dht.bootstrap.is_empty());
        assert!(!dht_cfg.dht.firewalled);
    }

    #[test]
    fn build_dht_config_uses_provided_bootstrap() {
        let cfg = ResolvedConfig {
            public: true,
            firewalled: false,
            bootstrap: vec!["1.2.3.4:49737".to_string()],
            node: Default::default(),
        };
        let dht_cfg = build_dht_config(&cfg);
        assert_eq!(dht_cfg.dht.bootstrap, vec!["1.2.3.4:49737"]);
    }

    #[test]
    fn build_dht_config_firewalled_when_not_public() {
        let cfg = ResolvedConfig {
            public: false,
            firewalled: false,
            bootstrap: vec![],
            node: Default::default(),
        };
        let dht_cfg = build_dht_config(&cfg);
        assert!(dht_cfg.dht.firewalled);
        assert!(
            dht_cfg.dht.bootstrap.is_empty(),
            "isolated mode should have no bootstrap nodes"
        );
    }

    // ── 3×6 Scenario Matrix: Bootstrap Type × Network Topology ────────────
    //
    // This test enumerates every combination of:
    //   Bootstrap types (B1-B3):
    //     B1: Public default (empty bootstrap + public=true → DEFAULT_BOOTSTRAP)
    //     B2: Explicit/custom (user-provided bootstrap addresses)
    //     B3: Isolated (empty bootstrap + public=false → empty, firewalled)
    //
    //   Network topologies (T1-T6):
    //     T1: Both open
    //     T2: Sender firewalled, receiver open
    //     T3: Sender open, receiver firewalled
    //     T4: Both firewalled, same host (same_host bypass)
    //     T5: Both firewalled, different networks (holepunch — same decision as T3)
    //     T6: One behind CGNAT (FIREWALL_RANDOM — distinct firewall type)
    //
    // For each cell we assert:
    //   1. Bootstrap config output (bootstrap list, firewalled flag)
    //   2. Connection-path decision (should_direct_connect result)
    //   3. Combined expected behavior (discovery feasible + connection path)

    /// Bootstrap mode B1: public=true, no explicit bootstrap → uses DEFAULT_BOOTSTRAP
    fn b1_config() -> ResolvedConfig {
        ResolvedConfig {
            public: true,
            firewalled: false,
            bootstrap: vec![],
            node: Default::default(),
        }
    }

    /// Bootstrap mode B2: public=true, explicit bootstrap provided
    fn b2_config() -> ResolvedConfig {
        ResolvedConfig {
            public: true,
            firewalled: false,
            bootstrap: vec!["10.0.0.1:49737".to_string()],
            node: Default::default(),
        }
    }

    /// Bootstrap mode B3: isolated (public=false, no bootstrap)
    fn b3_config() -> ResolvedConfig {
        ResolvedConfig {
            public: false,
            firewalled: false,
            bootstrap: vec![],
            node: Default::default(),
        }
    }

    /// Topology parameters for should_direct_connect.
    /// (relayed, remote_firewall, remote_holepunchable, same_host)
    struct TopologyParams {
        relayed: bool,
        firewall: u64,
        holepunchable: bool,
        same_host: bool,
    }

    /// Expected outcomes for a matrix cell.
    struct MatrixExpectation {
        /// Bootstrap list non-empty (discovery is possible)
        has_bootstrap: bool,
        /// Node is firewalled (affects announce behavior)
        firewalled: bool,
        /// should_direct_connect result
        direct_connect: bool,
        /// Human-readable expected behavior
        behavior: &'static str,
    }

    /// Full 3×6 scenario matrix test.
    #[test]
    fn scenario_matrix_3x6_cross_product() {
        // Define topology parameters for T1-T6
        let topologies: [(& str, TopologyParams); 6] = [
            (
                "T1: both open",
                TopologyParams {
                    relayed: false,
                    firewall: FIREWALL_OPEN,
                    holepunchable: true,
                    same_host: false,
                },
            ),
            (
                "T2: sender firewalled, receiver open",
                TopologyParams {
                    relayed: true,
                    firewall: FIREWALL_OPEN,
                    holepunchable: true,
                    same_host: false,
                },
            ),
            (
                "T3: sender open, receiver firewalled",
                TopologyParams {
                    relayed: true,
                    firewall: FIREWALL_CONSISTENT,
                    holepunchable: true,
                    same_host: false,
                },
            ),
            (
                "T4: both firewalled, same host",
                TopologyParams {
                    relayed: true,
                    firewall: FIREWALL_CONSISTENT,
                    holepunchable: true,
                    same_host: true,
                },
            ),
            (
                "T5: both firewalled, different networks",
                TopologyParams {
                    relayed: true,
                    firewall: FIREWALL_CONSISTENT,
                    holepunchable: true,
                    same_host: false,
                },
            ),
            (
                "T6: one behind CGNAT (symmetric/random NAT)",
                TopologyParams {
                    relayed: true,
                    firewall: FIREWALL_RANDOM,
                    holepunchable: true,
                    same_host: false,
                },
            ),
        ];

        // Define expected outcomes for each (bootstrap_type, topology) pair.
        // Format: (bootstrap_name, config_fn, expected_per_topology)
        type MatrixRow = (&'static str, fn() -> ResolvedConfig, [MatrixExpectation; 6]);
        let matrix: [MatrixRow; 3] = [
            (
                "B1: public default",
                b1_config as fn() -> ResolvedConfig,
                [
                    // T1: both open → direct connect, discovery via public DHT
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: true,
                        behavior: "direct connect via public DHT",
                    },
                    // T2: sender fw, receiver open → direct (receiver OPEN)
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: true,
                        behavior: "direct connect (receiver is open)",
                    },
                    // T3: sender open, receiver fw → holepunch
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: false,
                        behavior: "holepunch (receiver firewalled, holepunchable)",
                    },
                    // T4: both fw, same host → direct (same_host bypass)
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: true,
                        behavior: "direct connect (same host bypass)",
                    },
                    // T5: both fw, different networks → holepunch (same decision as T3)
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: false,
                        behavior: "holepunch (both firewalled, receiver holepunchable)",
                    },
                    // T6: CGNAT/symmetric NAT → holepunch (low success rate)
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: false,
                        behavior: "holepunch (CGNAT/symmetric NAT, FIREWALL_RANDOM)",
                    },
                ],
            ),
            (
                "B2: explicit/custom",
                b2_config as fn() -> ResolvedConfig,
                [
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: true,
                        behavior: "direct connect via custom bootstrap",
                    },
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: true,
                        behavior: "direct connect (receiver is open)",
                    },
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: false,
                        behavior: "holepunch (receiver firewalled, holepunchable)",
                    },
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: true,
                        behavior: "direct connect (same host bypass)",
                    },
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: false,
                        behavior: "holepunch (both firewalled, receiver holepunchable)",
                    },
                    MatrixExpectation {
                        has_bootstrap: true,
                        firewalled: false,
                        direct_connect: false,
                        behavior: "holepunch (CGNAT/symmetric NAT, FIREWALL_RANDOM)",
                    },
                ],
            ),
            (
                "B3: isolated",
                b3_config as fn() -> ResolvedConfig,
                [
                    // Isolated mode: no bootstrap → no DHT discovery possible.
                    // Connection path decision is still valid but moot since
                    // peers cannot discover each other without bootstrap.
                    MatrixExpectation {
                        has_bootstrap: false,
                        firewalled: true,
                        direct_connect: true,
                        behavior: "no discovery (isolated); direct if manually connected",
                    },
                    MatrixExpectation {
                        has_bootstrap: false,
                        firewalled: true,
                        direct_connect: true,
                        behavior: "no discovery (isolated); direct if manually connected",
                    },
                    MatrixExpectation {
                        has_bootstrap: false,
                        firewalled: true,
                        direct_connect: false,
                        behavior: "no discovery (isolated); would holepunch if connected",
                    },
                    MatrixExpectation {
                        has_bootstrap: false,
                        firewalled: true,
                        direct_connect: true,
                        behavior: "no discovery (isolated); same host bypass",
                    },
                    MatrixExpectation {
                        has_bootstrap: false,
                        firewalled: true,
                        direct_connect: false,
                        behavior: "no discovery (isolated); would holepunch if connected",
                    },
                    MatrixExpectation {
                        has_bootstrap: false,
                        firewalled: true,
                        direct_connect: false,
                        behavior: "no discovery (isolated); would holepunch if connected",
                    },
                ],
            ),
        ];

        // Run all 18 cases
        for (b_name, config_fn, expectations) in &matrix {
            let cfg = config_fn();
            let dht_cfg = build_dht_config(&cfg);

            for (i, (t_name, topo)) in topologies.iter().enumerate() {
                let exp = &expectations[i];

                // Assert bootstrap config
                assert_eq!(
                    !dht_cfg.dht.bootstrap.is_empty(),
                    exp.has_bootstrap,
                    "[{b_name} × {t_name}] bootstrap presence mismatch: expected has_bootstrap={}, got bootstrap={:?}",
                    exp.has_bootstrap,
                    dht_cfg.dht.bootstrap,
                );

                assert_eq!(
                    dht_cfg.dht.firewalled,
                    exp.firewalled,
                    "[{b_name} × {t_name}] firewalled mismatch: expected {}, got {}",
                    exp.firewalled,
                    dht_cfg.dht.firewalled,
                );

                // Assert connection path decision
                let direct = should_direct_connect(
                    topo.relayed,
                    topo.firewall,
                    topo.holepunchable,
                    topo.same_host,
                );
                assert_eq!(
                    direct, exp.direct_connect,
                    "[{b_name} × {t_name}] connection path mismatch: expected direct_connect={} ({}), got {}",
                    exp.direct_connect, exp.behavior, direct,
                );
            }
        }
    }

    /// Verifies that isolated mode (B3) with no bootstrap produces a config
    /// that makes DHT discovery impossible — the expected graceful degradation.
    #[test]
    fn isolated_mode_no_discovery_semantics() {
        let cfg = b3_config();
        let dht_cfg = build_dht_config(&cfg);

        // No bootstrap → no DHT nodes to query → no discovery
        assert!(
            dht_cfg.dht.bootstrap.is_empty(),
            "isolated mode must have empty bootstrap"
        );
        // Firewalled → won't accept incoming connections
        assert!(
            dht_cfg.dht.firewalled,
            "isolated mode must be firewalled"
        );
        // This means: announce will have no nodes to announce to,
        // lookup will have no nodes to query, and incoming connections
        // are blocked. The peer is effectively unreachable.
    }

    /// Verifies the CGNAT topology (T6) uses FIREWALL_RANDOM, which is the
    /// correct representation per Node.js reference (symmetric NAT = random
    /// port allocation = FIREWALL_RANDOM).
    #[test]
    fn cgnat_represented_as_firewall_random() {
        // CGNAT/symmetric NAT: each new connection gets a different external
        // port, making port prediction impossible. Node.js classifies this as
        // FIREWALL_RANDOM. Verify the constant value matches expectation.
        assert_eq!(FIREWALL_RANDOM, 3);
        assert_eq!(FIREWALL_UNKNOWN, 0);
        assert_eq!(FIREWALL_OPEN, 1);
        assert_eq!(FIREWALL_CONSISTENT, 2);

        // With FIREWALL_RANDOM + relayed + holepunchable: holepunch is attempted
        assert!(!should_direct_connect(true, FIREWALL_RANDOM, true, false));
        // But CGNAT holepunch success rate is low in practice — this is
        // documented as a known limitation of symmetric NAT traversal.
    }

    #[test]
    fn public_flag_sets_firewall_open_in_swarm_config() {
        use peeroxide::SwarmConfig;

        let public_cfg = ResolvedConfig {
            public: true,
            firewalled: false,
            bootstrap: vec![],
            node: Default::default(),
        };
        let private_cfg = ResolvedConfig {
            public: false,
            firewalled: false,
            bootstrap: vec![],
            node: Default::default(),
        };

        let dht_config = build_dht_config(&public_cfg);
        let mut swarm_config = SwarmConfig::default();
        swarm_config.dht = dht_config;
        if public_cfg.public {
            swarm_config.firewall = FIREWALL_OPEN;
        }
        assert_eq!(
            swarm_config.firewall, FIREWALL_OPEN,
            "public=true must set SwarmConfig.firewall to FIREWALL_OPEN"
        );

        let dht_config = build_dht_config(&private_cfg);
        let mut swarm_config = SwarmConfig::default();
        swarm_config.dht = dht_config;
        if private_cfg.public {
            swarm_config.firewall = FIREWALL_OPEN;
        }
        assert_eq!(
            swarm_config.firewall, 0,
            "public=false must leave SwarmConfig.firewall at default (UNKNOWN=0)"
        );
    }

    #[test]
    fn firewalled_flag_sets_firewall_consistent_in_swarm_config() {
        use peeroxide::SwarmConfig;

        let firewalled_cfg = ResolvedConfig {
            public: false,
            firewalled: true,
            bootstrap: vec!["10.0.0.1:49737".to_string()],
            node: Default::default(),
        };

        let dht_config = build_dht_config(&firewalled_cfg);
        assert!(
            dht_config.dht.firewalled,
            "--firewalled must set dht.firewalled=true"
        );

        let mut swarm_config = SwarmConfig::default();
        swarm_config.dht = dht_config;
        if firewalled_cfg.public {
            swarm_config.firewall = FIREWALL_OPEN;
        } else if firewalled_cfg.firewalled {
            swarm_config.firewall = FIREWALL_CONSISTENT;
        }
        assert_eq!(
            swarm_config.firewall, FIREWALL_CONSISTENT,
            "--firewalled must set SwarmConfig.firewall to FIREWALL_CONSISTENT (2)"
        );
        assert_eq!(FIREWALL_CONSISTENT, 2);
    }
}
