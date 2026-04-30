# Public Bootstrap / Relay Investigation

## Problem
`cp send --public` + `cp recv --public` fails. Both peers bootstrap to the public HyperDHT network successfully, the sender announces, the receiver does LOOKUP and finds the peer, but all PEER_HANDSHAKE attempts fail with "handshake failed: empty reply" or timeouts.

## Observed Behavior

Receiver logs:
```
connect_with_nodes: trying node candidate candidate=34.130.154.17:49737
query relay attempt failed relay=34.130.154.17:49737 err=handshake failed: empty reply
query relay attempt failed relay=88.99.3.86:39087 err=handshake failed: empty reply
query relay attempt failed relay=170.64.207.57:38610 err=handshake failed: empty reply
```

The receiver iterates through dozens of DHT nodes, sending PEER_HANDSHAKE (command=0) to each. All respond with empty value (no error code, but no handshake data).

## Root Cause Analysis

### Key Mismatch: ANNOUNCE target ≠ PEER_HANDSHAKE target

**ANNOUNCE** uses `target = topic` (the 32-byte topic hash)
**PEER_HANDSHAKE** uses `target = hash(remote_public_key)` (blake2b of server's public key)

These are different values unless the topic IS the hash of the public key (which it isn't for `cp`).

### Why Private Bootstrap Worked

In the 2-node private cluster:
1. Sender is one of very few nodes in the routing table
2. `connect_with_nodes` Phase 2 does FIND_NODE(hash(sender_pk))
3. In a tiny network, bootstrap nodes return the sender itself as a "closer node"
4. Receiver sends PEER_HANDSHAKE **directly to the sender**
5. Sender has `hash(pk)` registered in its LOCAL router (via `register_server`) → handles locally → success

### Why Public Bootstrap Fails

With thousands of nodes in the public network:
1. Sender announces to nodes closest to `topic` — stores ForwardEntry under `topic` (Root Cause 5 fix)
2. Receiver does FIND_NODE(hash(sender_pk)) — finds nodes close to hash(pk), which are DIFFERENT nodes
3. None of these nodes have hash(pk) OR the topic ForwardEntry
4. Receiver sends PEER_HANDSHAKE → Router lookup for hash(pk) → not found → `HandshakeAction::CloserNodes` → `req.reply(None)` → **"empty reply"**

The Phase 1 relay addresses (from LOOKUP result.from) also fail because:
- Those are nodes closest to `topic` that stored the ANNOUNCE
- Their Router has ForwardEntry under `topic`, NOT under `hash(pk)`
- PEER_HANDSHAKE target = hash(pk) → miss → empty reply

### How Node.js Solves This

In Node.js hyperdht:
1. `persistent.js` creates `this.router = new Router(dht)` — this SAME Router handles both FIND_PEER AND PEER_HANDSHAKE routing
2. When `onannounce()` receives a **self-announce** (where `hash(peer.publicKey) == target`):
   - It stores in `this.router.set(target, { relay: req.from })` 
   - This makes the entry findable by PEER_HANDSHAKE routing under hash(pk)
3. The server's Hyperswarm performs a **self-announce** (`target = hash(publicKey)`) in addition to the topic announce
4. This ensures nodes closest to hash(pk) can route PEER_HANDSHAKE to the server

In our Rust implementation:
- `persistent.rs` has a separate `HashMap<String, RouterEntry>` (used only by FIND_PEER)
- `router.rs` `Router` is a different struct (used by PEER_HANDSHAKE)
- These aren't connected — an announce_self entry in persistent.router doesn't help PEER_HANDSHAKE
- BUT: our Root Cause 5 fix already stores ForwardEntry in the Router under `incoming.target`
- For a self-announce, `incoming.target = hash(pk)` → ForwardEntry IS stored correctly!

**The only missing piece: the sender never does a self-announce.**

## Fix Required

### Fix 1: Self-Announce (CRITICAL)

The swarm must announce with `target = hash(publicKey)` in addition to the topic announce. This ensures:
- Nodes closest to hash(pk) store the ForwardEntry (via Root Cause 5 ANNOUNCE handler)
- `connect_with_nodes` Phase 2 (FIND_NODE(hash(pk))) reaches those same nodes
- PEER_HANDSHAKE to them succeeds (Router finds entry → relays to sender)

For public Node.js DHT nodes:
- Their persistent module stores self-announce in Router._entries
- PEER_HANDSHAKE routing finds the entry and relays to sender's address

### Fix 2: Direct Connect Path (SUPPLEMENTARY)

When the server sets `firewall = FIREWALL_OPEN` (--public flag makes firewalled=false), Node.js connect.js checks:
```javascript
if (payload.firewall === FIREWALL.OPEN) {
    const addr = getFirstRemoteAddress(payload.addresses4, serverAddress)
    if (addr) { c.onsocket(socket, addr.port, addr.host); return }
}
```

After the initial PEER_HANDSHAKE succeeds (via self-announce), the server's NoisePayload includes addresses4 and firewall=OPEN. The client should try connecting directly to those addresses instead of holepunching/relaying.

Currently the Rust connect_through_node sends `addresses4: vec![]` (empty) — the server never advertises its reachable addresses.

## Architecture Notes

### Router Forward Table Entry Path (on ANNOUNCE)

```
ANNOUNCE(target=X, from=serverAddr) 
  → persistent.on_announce() [stores peer record]
  → Root Cause 5 handler: Router.set(X, ForwardEntry { relay: serverAddr })
```

For topic announce: X = topic → ForwardEntry stored under topic
For self-announce: X = hash(pk) → ForwardEntry stored under hash(pk) ✓

### PEER_HANDSHAKE Routing

```
PEER_HANDSHAKE(target=hash(pk))
  → Router.get(hash(pk))
  → IF found: relay to entry.relay (server's address)
  → IF not found: CloserNodes → reply(None) → "empty reply"
```

### Node.js Connect Flow (reference)

1. Client LOOKUP(topic) → finds peer { publicKey, relayAddresses }
2. Client `connect(publicKey, { nodes: closestNodes })` 
3. `connect_with_nodes` sends PEER_HANDSHAKE to nodes closest to hash(pk)
4. Those nodes have self-announce stored → relay to server
5. Server handles PEER_HANDSHAKE locally (has hash(pk) registered)
6. Noise handshake completes → NoisePayload exchanged
7. If server firewall=OPEN → direct UDX connect
8. If NAT'd → holepunch or blind-relay

## Key Files

- `peeroxide/src/swarm.rs:487-491` — `register_server` (local router)
- `peeroxide/src/peer_discovery.rs:84-121` — discovery + PeerFound event
- `peeroxide-dht/src/hyperdht.rs:463-531` — `announce()` function
- `peeroxide-dht/src/hyperdht.rs:928-1031` — `connect_with_nodes` (3-phase connect)
- `peeroxide-dht/src/hyperdht.rs:1054-1113` — `connect_through_node` (PEER_HANDSHAKE send)
- `peeroxide-dht/src/hyperdht.rs:1855-1876` — ANNOUNCE handler (Root Cause 5 forward table)
- `peeroxide-dht/src/router.rs:189-234` — `route_handshake` (the CloserNodes fallback)
- `peeroxide-dht/src/persistent.rs:471-493` — `announce_self` check and persistent.router storage

## Implementation Plan

1. Add self-announce to the swarm: after topic announce, also `dht.announce(hash(publicKey), keypair, relay_addrs)`
2. Populate `addresses4` in the local NoisePayload (connect_through_node) with the server's reachable addresses
3. Test with public bootstrap — verify PEER_HANDSHAKE succeeds via relay

## Fix Applied

### Root Cause 10 — Missing Self-Announce (FIXED)

**File**: `peeroxide/src/peer_discovery.rs`

**Change**: After announcing the topic, `do_refresh` now also announces `hash(public_key)` as a second announce call. This ensures nodes closest to `hash(pk)` store a ForwardEntry with the server's address, enabling PEER_HANDSHAKE routing.

```rust
// In do_refresh(), after topic announce:
let pk_target = hash(&key_pair.public_key);
dht.announce(pk_target, key_pair, relay_addresses).await;
```

**Result**: 
- Private bootstrap: `test_cp_local_roundtrip` passes (6.3s, previously #[ignore])
- Public bootstrap: `test_live_cp_send_recv` passes (30.7s, file transfer over public HyperDHT)
- All 534 workspace tests pass (0 failures)

**Why this works**: The announce with `target = hash(pk)` hits the same ANNOUNCE handler (Root Cause 5 fix at hyperdht.rs:1861-1872) which stores `ForwardEntry { relay: server_addr }` under `hash(pk)` in the Router. When a client later sends PEER_HANDSHAKE with `target = hash(pk)`, the bootstrap/relay node finds this entry and forwards the handshake to the server.

### addresses4 (NOT YET IMPLEMENTED — optimization only)

The server handshake reply currently sends `addresses4: vec![]`. For direct connect when FIREWALL_OPEN, the server should advertise its NAT-sampled public address. This is an optimization — the current code falls back to `hs_result.server_address` (extracted from the handshake relay path) which already works for establishing direct connections.

---

## Required Scenario Coverage

All combinations of bootstrap type × network topology must work for `cp send`/`cp recv`.

### Bootstrap Types
| Type | Config | Status |
|------|--------|--------|
| Private-local | `--bootstrap 127.0.0.1:PORT` | ✅ Verified (test_cp_local_roundtrip) |
| Custom-remote | `--bootstrap remote:PORT` | Same codepath as private-local |
| Public | `--public` (uses DEFAULT_BOOTSTRAP) | ✅ Verified (test_live_cp_send_recv) |

### Network Topologies (sender = server, receiver = client)
| # | Scenario | Connection Path | Status |
|---|----------|----------------|--------|
| 1 | Same host | Direct (loopback) | ✅ Verified |
| 2 | Same LAN, no firewall | Direct connect (FIREWALL_OPEN) | Should work — same as #1 |
| 3 | Same LAN, NAT between | Holepunch or relay | Needs verification |
| 4 | Internet, no firewall | Direct connect (FIREWALL_OPEN) | ✅ Verified (live test, same machine but through public DHT) |
| 5 | Internet, sender firewalled | relay_through → blind relay | Needs verification |
| 6 | Internet, receiver firewalled | sender OPEN → direct from receiver | Should work (client initiates) |
| 7 | Internet, both firewalled | Holepunch → fallback blind relay | Needs verification |

### Connection Path Decision Tree (connect_through_node, line ~1167)
```
After PEER_HANDSHAKE completes:
  IF relay_through in server reply → blind relay connection
  ELIF !relayed OR firewall==OPEN OR !remote_holepunchable OR same_host:
    → Direct connect using server_address/addresses4
  ELSE:
    → Holepunch rounds via PEER_HOLEPUNCH relay
```

### Paths Requiring Verification
1. **relay_through** — Server behind firewall tells client to use blind-relay
2. **holepunch** — Both peers behind symmetric NAT
3. **addresses4** — Server advertises public address for direct connect optimization

---

## Root Cause 11: Server Entries Expired on TTL (Production Bug)

**Found by**: Oracle verification review
**File**: `peeroxide-dht/src/router.rs` lines 89-100

### Problem
`Router::get()` applied TTL expiry uniformly to ALL entries, including entries with `has_server: true` (local server registrations). After 20 minutes, a long-lived server would silently stop being routable because its own Router entry expired. Incoming handshakes would get `CloserNodes` instead of `HandleLocally`.

In Node.js, server entries use `retain` semantics and persist until explicitly removed. Only forwarded announce entries (from remote peers) are subject to TTL expiry.

### Fix
- `Router::get()`: Skip TTL check when `entry.has_server == true`
- `Router::gc()`: Preserve entries with `has_server == true` during garbage collection
- Server entries now persist until explicitly removed via `delete()` / `unregister_server()`

### Tests Added
- `server_entry_never_expires_on_ttl` — server entry survives 1 hour past insertion
- `server_entry_survives_gc` — gc preserves server entries
- `server_entry_routes_handshake_after_ttl` — handshake still routes to local server after 1 hour

---

## Scenario Matrix Test Coverage

### Topology Unit Tests (`hyperdht.rs::tests::topology_*`)
Maps to user-defined topology matrix via `should_direct_connect()`:

| Topology | Test | Outcome |
|----------|------|---------|
| T1: Both open | `topology_both_open` | Direct connect (not relayed) |
| T2: Sender firewalled, receiver open | `topology_sender_firewalled_receiver_open` | Direct connect (FIREWALL_OPEN) |
| T3: Sender open, receiver firewalled | `topology_sender_open_receiver_firewalled` | Holepunch (if holepunchable), direct (if not) |
| T4: Both firewalled, same network | `topology_both_firewalled_same_network` | Direct connect (same_host) |
| T5: Both firewalled, different networks | `topology_both_firewalled_different_networks` | Holepunch (if holepunchable), direct fallback |
| T6: One behind CGNAT | `topology_one_behind_cgnat` | Holepunch (FIREWALL_RANDOM + holepunchable) |

### Integration Tests (`local_commands.rs`)
All use private (local spawned) bootstrap. Same-host tests exercise E2E transfer paths:

| Test | Sender | Receiver | Path Taken |
|------|--------|----------|------------|
| `test_cp_local_roundtrip` | --public | --public | Direct connect |
| `test_cp_sender_firewalled_receiver_open` | (default) | --public | Direct connect |
| `test_cp_sender_open_receiver_firewalled` | --public | (default) | Direct connect |
| `test_cp_both_firewalled_same_host` | (default) | (default) | Direct connect (same_host short-circuit) |

### Known Limitations
- **Same-host short-circuit**: All integration tests take the direct connect path because `should_direct_connect` returns true when `same_host=true`. This means relay/holepunch paths are NOT exercised by integration tests.
- **Bootstrap mode coverage**: All tests use explicit local `--bootstrap` addresses (custom mode). Public bootstrap and private-only modes are not integration-tested.
- **Different-network and CGNAT topologies**: Require separate network namespaces or multi-host setups; covered only at unit level.

---

## `same_host` Condition: Node.js Compatibility Verified

**Concern raised by**: Oracle review
**Claim**: "`should_direct_connect` should not treat `same_host` as unconditional direct-connect"

### Investigation (Librarian: holepunchto/hyperdht connect.js)

Node.js handles LAN/same-host at `lib/connect.js:242-259` inside `holepunch()`:
```javascript
if (c.lan && relayed && clientAddress.host === serverAddress.host) {
  // LAN direct connect via addresses4
}
```

The Rust condition `!relayed || firewall == OPEN || !holepunchable || same_host` is **logically equivalent** to `!relayed || firewall == OPEN || !holepunchable || (relayed && same_host)` because:
- When `relayed=false`: `!relayed` is already true, making the expression true regardless of `same_host`
- When `relayed=true`: `same_host` activates the LAN shortcut, matching Node.js behavior

**Conclusion**: The current Rust code is correct. No change needed.

## Bootstrap Mode Test Coverage

### Three Bootstrap Modes
1. **Public default** (`--public`, no `--bootstrap`): Uses `DEFAULT_BOOTSTRAP` (3 well-known nodes)
2. **Explicit/custom** (`--bootstrap <addr>`): Uses provided addresses
3. **Isolated** (no `--public`, no `--bootstrap`): Empty bootstrap, firewalled=true

### Unit Tests (peeroxide-cli/src/cmd/mod.rs)
| Test | Mode Covered |
|------|-------------|
| `build_dht_config_uses_defaults_when_public_no_bootstrap` | Public default |
| `build_dht_config_uses_provided_bootstrap` | Explicit/custom |
| `build_dht_config_firewalled_when_not_public` | Isolated mode |

### Why Integration Tests Don't Vary by Bootstrap Type
All three modes flow through the same `build_dht_config` → `DhtConfig.bootstrap` → `hyperdht::spawn` path. The only difference is which addresses populate `DhtConfig.bootstrap`. Topology scenarios (firewall, holepunch, relay) are independent of bootstrap type. Public bootstrap integration tests would require connecting to live network (flaky, not CI-suitable).
