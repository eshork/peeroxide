# DHT Routing Table / CP Discovery Investigation

## Problem
`peeroxide node` reports 0 peers in routing table. `cp send`/`cp recv` can't find each other in a private 2-node bootstrap cluster.

## Root Cause 1: Random Table ID (FIXED)

Node.js dht-rpc transitions table ID from `randomBytes(32)` → `peer.id(host, port)` after bootstrap (in `_updateNetworkState`). The Rust implementation never did this transition.

**ID validation chain:**
- `io.rs:848` validates incoming response IDs: `peer_id(from.host, from.port) == claimed_id`
- If validation fails → `validated_id = None`
- `rpc.rs:1207` in `add_node_from_network`: `None => return` — node not added

**Fix applied:**
- `routing_table.rs`: Added `rebuild_with_id()` method
- `rpc::spawn()`: For non-ephemeral + non-wildcard host, set ID = `peer_id(host, port)` immediately after bind
- For wildcard host (0.0.0.0): collect `to` field from responses during bootstrap, determine consensus address in `mark_bootstrapped()`, then update ID

**Result:** Nodes now show "routing table: 1 peers" ✅

## Root Cause 2: ANNOUNCE silently dropped (IN PROGRESS)

Even with routing fixed, `cp send`/`cp recv` can't discover each other.

**Symptom:** Send announces successfully (`announce complete closest=2`), recv looks up but finds nothing.

**Analysis:**
- `hyperdht.rs:1816-1841`: When a bootstrap node receives an ANNOUNCE request:
  ```rust
  let node_id = req.id;  // sender's validated ID
  // ...
  ANNOUNCE => {
      if let Some(nid) = node_id {
          storage.on_announce(&incoming, &nid)
      } else {
          HandlerReply::Silent  // ← DROPPED!
      }
  }
  ```
- `cp send` is ephemeral (`build_dht_config` doesn't set `ephemeral`, defaults to `true` when bootstrap is provided) AND firewalled (`!cfg.public`)
- Ephemeral + firewalled → `include_id = false` in outgoing messages
- So `req.id = None` when the ANNOUNCE arrives → silently dropped

**Node.js reference (lib/persistent.js):**
```js
onannounce(req) {
    if (!req.target || !req.token || !this.dht.id) return
    //                                ^^^^^^^^^^^
    // Checks the RECEIVING node's own ID, NOT the sender's ID
```

The Node.js code uses `this.dht.id` (the storage node's own ID) for:
1. The gate check ("do I have an ID yet?")
2. Signature verification (`ann.verify(target, token, this.dht.id, ...)`)

The Rust code incorrectly uses `req.id` (the sender's validated ID) which:
- Fails the gate check when sender is ephemeral (most clients)
- Would use wrong ID for signature verification even if it passed

**Fix needed:** Change `hyperdht.rs:1838` to use the receiving node's own table ID instead of the sender's request ID. The sender already signs with the target node's ID (from `reply.from_id` in the LOOKUP phase at `hyperdht.rs:489`).

**Fix applied:**
- Added `DhtHandle::table_id()` method (`rpc.rs`) — sends `DhtCommand::TableId` to actor, returns `Option<NodeId>`
- Changed ANNOUNCE/UNANNOUNCE handlers in `hyperdht.rs:run_request_handler` to call `dht.table_id().await` and use the node's own ID for signature verification
- Previously: `if let Some(nid) = req.id { storage.on_announce(&incoming, &nid) }` — used sender's ID (None for ephemeral clients)
- Now: `let own_id = dht.table_id().await.ok().flatten(); if let Some(nid) = own_id { ... }` — uses receiving node's ID

**Result:** Lookup now returns stored peers ✅. Full discovery chain works: announce → store → lookup → connect → handshake complete.

## Root Cause 3: UDX Stream (FIXED)

After successful discovery + Noise handshake, the UDX stream fails:
- Send side: `server: stream establishment failed err=UDX error: I/O error: socket not bound`
- Recv side: `process_datagram: decode failed from=... err=invalid IP family: 255` (UDX packets hitting DHT decoder)

**Analysis:**
- `peeroxide/src/swarm.rs:956` — `create_server_connection` calls `runtime.create_socket().await?` but never binds it
- `libudx/src/native/socket.rs:54,63` — `udp_arc()` and `local_addr()` return "socket not bound" when `OnceLock<UdpSocket>` is unset
- The correct pattern (used in `hyperdht.rs:1490-1493`): `socket.bind("0.0.0.0:0".parse().expect("valid addr")).await?`
- The recv side decode errors are secondary: UDX packets from the client reach the server's DHT socket because the server never opened a UDX socket to receive them

**Fix applied:**
- Added `socket.bind("0.0.0.0:0".parse().expect("valid addr")).await?` after `create_socket()` in `create_server_connection` (swarm.rs)

**"invalid IP family: 255" explanation:**
- UDX packet header starts with byte `0xFF` (255) as a magic marker
- When the recv side sends UDX data to the server's DHT port (because the server never opened a separate UDX socket), the DHT decoder sees `first_byte=255` and tries to parse it as a DHT message → fails with "invalid IP family"
- Once the server properly binds a UDX socket, data flows to the correct socket

## Root Cause 4: Socket sharing (FIXED)

After binding the socket, streams need to share the DHT's existing socket for multiplexing (matching Node.js single-socket model).

**Fix applied:**
- `establish_stream` now accepts `Option<Arc<UdxSocket>>` — reuses DHT primary socket for client side
- `create_server_connection` in swarm.rs uses `dht.listen_socket()` — the actual bound server socket

## Root Cause 5: Router forward table (FIXED)

Bootstrap nodes receiving ANNOUNCE didn't populate the Router forward table, so `route_handshake` couldn't relay PEER_HANDSHAKE to the server.

**Fix applied:**
- Bootstrap nodes store `ForwardEntry { relay: Some(from), has_server: false }` on ANNOUNCE
- Guard prevents overwriting existing `has_server: true` entries

## Root Cause 6: peer_address in handshake reply (FIXED)

`handle_server_handshake` was setting `peer_address: Some(from.clone())` (client's own address) → client connected UDX to itself.

**Fix applied:**
- Changed to `peer_address: None` for direct connections
- Only relay nodes set `peer_address` (to the server's address)

## Root Cause 7: Server-side socket mismatch (FIXED)

`create_server_connection` used `DhtHandle::server_socket()` which returns `io.primary_socket()` (client socket for firewalled nodes). The recv connected to the server's LISTEN port, but the server's UDX stream was on a different socket.

**Fix applied:**
- Added `DhtHandle::listen_socket()` → returns actual `io.server_socket()` (the bound server socket)
- `create_server_connection` now uses `listen_socket()` instead of `server_socket()`
- `server_socket()` remains for client-side `establish_stream` usage (correct: client sends handshake from primary socket)

## Root Cause 8: UDX stream EOF / FIN not propagating (FIXED)

After file transfer, sender's `drop(conn)` didn't send a FIN to the receiver. The receiver would hang waiting for more data.

**Analysis:**
- `UdxAsyncStream::poll_shutdown` queued a FLAG_END (FIN) packet but returned `Poll::Ready(Ok(()))` immediately without waiting for ACK
- After poll_shutdown, `drop(conn)` fired → `UdxAsyncStream::Drop` sent `Shutdown` to processor → processor exited
- Between shutdown and handle.destroy(), there was a race: the processor might not send the FIN before the socket was torn down
- Even if FIN was sent, there was a logic bug: after a prior write's pending_ack resolved, poll_shutdown returned Ready without proceeding to queue the FIN

**Fix applied:**
- `poll_shutdown` now waits for FIN ACK before returning Ready (registers `PendingWrite` with oneshot)
- Added `fin_queued` flag to distinguish between "write ACK resolved, proceed to FIN" vs "FIN ACK resolved, done"
- `SecretStream::shutdown()` method added to expose the underlying transport shutdown
- `cp send` calls `conn.peer.stream.shutdown()` before dropping the connection

## Architecture Notes

### ID lifecycle (Node.js reference):
1. Start: `randomBytes(32)` (ephemeral)
2. After NAT sampling: `peer.id(publicHost, publicPort)` (persistent)
3. ID included in messages only when: `!ephemeral && socketKind == Server`

### Announce/Lookup flow:
1. `cp send` joins topic as server → swarm calls `dht.announce(topic, keypair, relay_addrs)`
2. `announce()` first does LOOKUP query to find closest nodes + get tokens
3. For each closest node with a token, signs announcement using THAT NODE's ID (`reply.from_id`)
4. Sends ANNOUNCE request with signed data + token
5. Storage node verifies signature using ITS OWN ID + the token it issued

### Key files:
- `peeroxide-dht/src/rpc.rs` — DHT node actor, routing table population
- `peeroxide-dht/src/io.rs` — Wire protocol, ID validation, include_id logic
- `peeroxide-dht/src/hyperdht.rs` — HyperDHT layer, announce/lookup/connect
- `peeroxide-dht/src/persistent.rs` — Persistent storage for announcements
- `peeroxide-dht/src/routing_table.rs` — Kademlia routing table
- `peeroxide-cli/src/cmd/node.rs` — Node command
- `peeroxide-cli/src/cmd/cp.rs` — CP send/recv commands
- `peeroxide-cli/src/cmd/mod.rs` — `build_dht_config` helper
