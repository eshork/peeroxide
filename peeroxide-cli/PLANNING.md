# peeroxide-cli

Command-line interface for the peeroxide P2P networking stack.

Installs a single binary called `peeroxide` with subcommands for operating and interacting with the Hyperswarm-compatible network.

## Status

| Subcommand | Status | Notes |
|---|---|---|
| `node` | done | Config file, data-dir persistence, stats-interval all specified |
| `keygen` | under_review | Output format TBD |
| `lookup` | under_review | `--with-data` opt-in decided |
| `announce` | under_review | `--data` + mutable_put defined; 1000 byte cap |
| `ping` | under_review | Direct + pubkey modes defined |
| `cp send/recv` | under_review | Transfer protocol defined; open questions on dirs, confirmation |
| `deaddrop leave/pickup` | new | Linked-list storage, ack protocol, privacy model drafted |

## Subcommands

### `peeroxide node`

Run a persistent DHT coordination (bootstrap) node.

**Modes:**

| Flag | Behavior |
|---|---|
| (no flags) | Isolated node — starts a new DHT network with no upstream peers |
| `--public` | Joins the public HyperDHT network (uses `peeroxide-dht`'s built-in bootstrap list) |
| `--bootstrap <addr>...` | Connects to specified bootstrap peers (private or custom network) |
| `--public --bootstrap <addr>...` | Public network + additional custom peers |

**Options:**

- `--port <port>` — bind port (default: 49737)
- `--host <addr>` — bind address (default: 0.0.0.0)
- `--stats-interval <seconds>` — how often to log routing table size (default: 60)
- `--max-records <n>` — max announcement records stored (library default: 65536). Maps to `PersistentConfig::max_records`.
- `--max-lru-size <n>` — max entries per LRU cache — applies to mutables, immutables, bumps, and refreshes caches independently (library default: 65536). Maps to `PersistentConfig::max_lru_size`.
- `--max-per-key <n>` — max peer announcements stored per topic (library default: 20). Maps to `PersistentConfig::max_per_key`.
- `--max-record-age <seconds>` — ⚠️ TTL for announcement records before expiry (library default: 1200 / 20 minutes). **Advanced:** changing this affects how long records survive without refresh. Peers expect a 20-minute window — reducing this may cause premature record loss; increasing it retains stale records longer.
- `--max-lru-age <seconds>` — ⚠️ TTL for LRU cache entries (mutables, immutables) before expiry (library default: 1200 / 20 minutes). **Advanced:** same protocol-coupling concerns as `--max-record-age`. Only change for isolated networks where you control all peer refresh intervals.
- `--config <path>` — load configuration from a TOML file (all CLI flags can be specified in config; CLI flags override config values)

All storage flags map directly to `PersistentConfig` fields in the library — no translation layers or invented abstractions. When count limits are reached, oldest entries are evicted (LRU) to make room; no requests are refused.

**When to tune storage limits:** On the public HyperDHT network (thousands of nodes), Kademlia naturally distributes records across the keyspace — a single node only stores records in its hash neighborhood, and the defaults (65536) are generous. On **private/small networks** (1–10 nodes), each node covers a much larger fraction of the keyspace and may store most or all records. Operators running private DHTs with many clients should increase `--max-records` and `--max-lru-size` accordingly. Note: the library's eviction algorithm is O(n) — at very high record counts (>500K), performance may degrade and library-level optimization would be needed.

**Estimated disk/memory usage with defaults (no manual limits):**

| Store | Max entries | Worst-case size per entry | Estimated max |
|---|---|---|---|
| Announcement records | 65,536 | ~150 B (pubkey + encoded HyperPeer) | ~10 MB |
| Mutable records | 65,536 | ~1,464 B (hex key + value + seq + 64B signature) | ~96 MB |
| Immutable records | 65,536 | ~1,390 B (hex key + raw value) | ~91 MB |
| Bumps (dedup) | 65,536 | ~72 B | ~5 MB |
| Refreshes (dedup) | 65,536 | ~72 B | ~5 MB |
| Routing table | ~10,000 typical | ~50 B | ~0.5 MB |
| **Total** | | | **~200 MB** |

Worst case assumes every mutable/immutable record is at the maximum payload that fits in a single UDP datagram (~1326 bytes at MTU_MAX=1500, minus ~174 bytes wire overhead). The library does not enforce a value size limit — it stores whatever arrives. In practice, most records will be smaller; realistic peak is ~100–150 MB. This applies to both in-memory usage (always) and disk usage (when `--data-dir` is set).

**Config file format (TOML):**

```toml
port = 49737
host = "0.0.0.0"
public = true
bootstrap = ["node1.example.com:49737", "node2.example.com:49737"]
stats_interval = 60
max_records = 65536
max_lru_size = 65536
max_per_key = 20
max_record_age = 1200
max_lru_age = 1200
```

All fields are optional. CLI flags take precedence over config file values.

**Behavior:**

- Runs non-ephemeral (`ephemeral: false`) so the node persists in peers' routing tables
- On startup: prints the node's ID (public key hash) and bound address to **stdout** (not via the logging system) so it is always visible regardless of `RUST_LOG` setting. This is essential operational output — operators need it to share the node as a bootstrap peer.
- Bootstrap handling depends on mode:
  - **Isolated** (no `--public`, no `--bootstrap`): bootstrap completes immediately (nothing to discover). Logs: `"Node ready (isolated mode) — listening for incoming peers"`. The node sits with an empty routing table until other nodes contact it directly.
  - **Networked** (`--public` and/or `--bootstrap`): bootstrap completes when the initial FIND_NODE query finishes and the routing table is populated. Logs: `"Bootstrap complete — routing table: N peers"`.
- Periodically logs routing table size at the configured `--stats-interval`. Also reports routing table size immediately after bootstrap completes (first data point before the periodic cycle begins).
- Handles SIGINT/SIGTERM for graceful shutdown via `destroy()`

**Storage & eviction:**

A non-ephemeral DHT node stores data on behalf of the network — not just routing pointers. This includes:
- **Announcement records** — which peers announced on which topics (serves LOOKUP queries)
- **Mutable records** — signed key-value pairs from `mutable_put` (serves MUTABLE_GET queries)
- **Immutable records** — content-addressed data from `immutable_put` (serves IMMUTABLE_GET queries)

**Eviction policy:** LRU (least recently used). When at capacity (`--max-records` or `--max-lru-size`), the oldest entries are dropped to make room for new writes. No requests are refused — the HyperDHT protocol has no "storage full" error, and Kademlia distributes data across the K closest nodes, so eviction from one node doesn't mean data loss from the network.

**TTL:** All records expire after `--max-record-age` / `--max-lru-age` (default 20 minutes) regardless of storage caps. Peers must refresh periodically to keep data alive.

**Future enhancement — `--data-dir` (disk persistence):** The library currently has no save/load support — `RoutingTable` and `Persistent` storage are purely in-memory with no serialization, no serde derives, and no external access to snapshot state. Adding disk persistence requires library-level changes: exposing export/import methods on `DhtHandle`/`HyperDhtHandle` and adding serialization to internal types. Deferred to a future version.

**Log levels** (controlled via `RUST_LOG` environment variable):

| Level | Output |
|---|---|
| `error` | Fatal failures — socket bind errors, bootstrap failure, unrecoverable state |
| `warn` | Peers failing verification, dropped connections, timeouts |
| `info` | Node ID + listen address, bootstrap complete, periodic routing table size, shutdown signal received |
| `debug` | Individual query/response traffic, routing table add/remove, NAT detection |
| `trace` | Wire-level bytes, per-packet encode/decode |

---

### `peeroxide keygen`

*THIS NEEDS MORE THOUGHT* — should probably include generating keys from seed phrases (as a accepted by the other commands) - also none of the current commands support loading from a key file, so maybe this is really just for debugging and curiosity?

Generate an Ed25519 keypair for use with announce/connect operations.

**Options:**

- `--seed <hex>` — derive from a specific seed (deterministic)
- `--output <path>` — write keypair to file (default: stdout)

**Output format:** TBD — likely JSON with hex-encoded public/secret keys.

---

### `peeroxide lookup`

Query the DHT for peers announcing a topic.

**Arguments:**

- `<topic>` — either a 64-char hex string (raw 32-byte topic) or a plain name (hashed via BLAKE2b-256)

**Options:**

- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes
- `--with-data` — also fetch metadata (via `mutable_get`) for each discovered peer (slower: runs an additional iterative query per peer)
- `--json` — output as JSON (default: human-readable table)

**Output:**

For each responding node, display the peers it knows about:
- Responding node address (host:port)
- For each peer: public key (hex) and relay addresses (host:port list)
- If `--with-data`: any metadata stored via `mutable_get` for that peer's public key

**Implementation:**

1. Calls `HyperDhtHandle::lookup(topic) -> Vec<LookupResult>` where each `LookupResult` contains `from: Ipv4Peer` and `peers: Vec<HyperPeer>` (public_key + relay_addresses).
2. If `--with-data`: for each discovered peer, calls `HyperDhtHandle::mutable_get(&peer.public_key, 0)` to retrieve any associated metadata. A `seq` of 0 fetches the latest version regardless of sequence number. These queries run in parallel for speed.
3. Displays combined results: peer identity, reachability info, and metadata (if any).

**Performance note:** Without `--with-data`, lookup is a single iterative DHT query. With `--with-data`, each discovered peer triggers an additional full iterative query at a *different* DHT coordinate (`hash(public_key)` vs the topic hash). For N peers, that's N+1 total queries. There is no signal in the lookup response that indicates whether a peer has metadata stored — you must query to find out.

---

### `peeroxide announce`

Announce presence on a topic so other peers can discover you.

**Arguments:**

- `<topic>` — either a 64-char hex string or a plain name (hashed via BLAKE2b-256)

**Options:**

- `--seed <hex>` — 32-byte seed to derive keypair (required unless --key-file provided)
- `--key-file <path>` — path to keypair file (from `peeroxide keygen`)
- `--relay <addr>...` — relay addresses to advertise (max 3 used)
- `--data <string>` — metadata to store alongside the announcement (max 1000 bytes; stored via `mutable_put`)
- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes
- `--duration <seconds>` — how long to stay announced (default: indefinite, re-announces periodically)

**Behavior:**

- Calls `HyperDhtHandle::announce(topic, &keypair, &relay_addresses)`
- If `--data` is provided, also calls `HyperDhtHandle::mutable_put(&keypair, data, seq)` to store metadata on the DHT keyed by the announcement keypair's public key
- Stays running and re-announces periodically (DHT records expire after ~20 minutes)
- Prints closest nodes on initial announce
- SIGINT/SIGTERM triggers `unannounce()` then exits cleanly

**Data size limit:** The `--data` value must be ≤ 1000 bytes. DHT messages are single UDP datagrams (no fragmentation at the protocol layer). With ~174 bytes of wire overhead for the mutable_put request envelope, 1000 bytes of payload fits safely within the 1200-byte base MTU used by the network.

**Implementation:**

Uses `KeyPair::from_seed(seed)` or loads from file. Re-announce interval should be well under the 20-minute `max_record_age` TTL (e.g. every 5 minutes). If `--data` is present, `mutable_put` is called with `seq: 1` initially and incremented on each refresh.

---

### `peeroxide ping`

Check reachability of a DHT node or peer.

**Arguments:**

- `<target>` — either `host:port` (direct DHT ping) or a 64-char hex public key (peer discovery + ping)

**Options:**

- `--count <n>` — number of pings to send (default: 1)
- `--connect` — perform a full Noise handshake to verify end-to-end connectivity (heavier, requires generating a local keypair)
- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes

**Behavior:**

When target is `host:port`:
- Calls `DhtHandle::ping(host, port)` directly
- Reports: reachable/unreachable, RTT (ms), node ID (if present)

When target is a public key (hex):
- Calls `HyperDhtHandle::query_find_peer(target)` to discover candidate addresses
- Pings each candidate via `DhtHandle::ping(host, port)`
- Reports per-candidate: address, reachable/unreachable, RTT
- With `--connect`: performs `HyperDhtHandle::connect()` to verify full Noise handshake (then disconnects)

---

### `peeroxide cp`

Copy files between peers over the swarm.

**Subcommands:**

```
peeroxide cp send <file>           # sender: announce file, print topic code
peeroxide cp recv <topic> [dest]   # receiver: connect to sender, download file
```

#### `peeroxide cp send`

**Arguments:**

- `<file>` — path to file to send
- `[topic]` — optional: a 64-char hex string or a plain name (hashed via BLAKE2b-256). If omitted, a random topic is generated.

**Options:**

- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes
- `--keep-alive` — don't exit after first transfer (allow multiple receivers)

**Behavior:**

1. Generate an ephemeral `KeyPair`
2. Determine topic: use provided topic (hex or hashed name) or generate a random 32-byte topic
3. Announce on that topic
4. Print the topic to stdout for the receiver to use (hex, or echo back the plain name if one was provided)
5. Wait for incoming connection (via server events)
6. On connection: send metadata header, then stream file in 64KB chunks
7. Close connection after transfer completes (signals EOF to receiver)
8. Exit (or wait for next connection if `--keep-alive`)

#### `peeroxide cp recv`

**Arguments:**

- `<topic>` — a 64-char hex string or a plain name (same value the sender used/was given)
- `[dest]` — destination path (default: current directory, using sender's filename)

**Options:**

- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes

**Behavior:**

1. Lookup the topic on the DHT to discover the sender
2. Connect to the sender (Noise handshake authenticates both sides)
3. Receive metadata header (first message): filename, file size
4. Stream remaining messages to disk in order
5. Connection close (read returns `None`) signals transfer complete
6. Print summary: filename, bytes received, transfer speed

#### Transfer protocol (over SecretStream)

The connection is already Noise-encrypted by peeroxide-dht. On top of the raw SecretStream, the file transfer uses a simple framing:

**Message 1 (metadata):** JSON object
```json
{"filename": "data.tar.gz", "size": 1048576, "version": 1}
```

**Messages 2..N (data chunks):** Raw file bytes, up to 64KB per message.

**EOF:** Sender closes the connection. Receiver sees `SecretStream::read() -> Ok(None)`.

**Design rationale:**
- No PAKE needed — Noise handshake with the sender's keypair (derived from the topic) provides mutual authentication. Only the sender holds the secret key for the announced public key.
- 64KB chunk size — proven by croc and Magic Wormhole, fits well within SecretStream's 16MB max message size while keeping memory pressure low.
- Connection close = EOF — simpler than a sentinel message. Empty messages can't be used (treated as keepalives by SecretStream).
- No resumption in v1 — keep it simple. Can add chunk offsets later if needed.

#### Security model

- The Noise handshake authenticates the sender — only the holder of the announced keypair's secret key can complete the handshake with a connecting receiver.
- The topic is a rendezvous point, not an authentication token. Anyone who knows the topic can attempt to connect. The sender should only accept connections from expected peers (or accept all, depending on use case).
- For secret sharing: use an unguessable topic (random, or a passphrase only known to sender/receiver). The topic functions as a "room code" — knowing it lets you find the sender on the DHT, but Noise still authenticates the connection.
- An attacker who discovers the topic can connect to the sender. The sender can optionally restrict this by requiring the receiver's public key in advance (not implemented in v1 — accept-all is simpler for ad-hoc transfers).

---

### `peeroxide deaddrop`

Anonymous store-and-forward via the DHT. No direct connection between sender and receiver — data is stored as linked mutable records on DHT nodes. Neither party learns the other's identity or IP.

**Subcommands:**

```
peeroxide deaddrop leave <file|->       # store data on DHT, print pickup key
peeroxide deaddrop pickup <key>         # retrieve data from DHT using pickup key
```

#### `peeroxide deaddrop leave`

**Arguments:**

- `<file|->` — file path to store, or `-` for stdin

**Options:**

- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes
- `--max-pickups <n>` — exit after N pickup acknowledgements received (default: exit on SIGINT)
- `--ttl <seconds>` — stop refreshing after this duration and let the drop expire (default: indefinite)
- `--passphrase` — derive the root keypair from an interactive passphrase instead of random generation (both sides must know it)

**Behavior:**

1. Generate a root `KeyPair` (random, or from `--passphrase` via BLAKE2b-256 hash → seed)
2. Split data into chunks (~950 bytes each, reserving space for the `next` pointer)
3. Generate a derived `KeyPair` for each chunk: `KeyPair::from_seed(blake2b(root_secret_key || chunk_index))`
4. Write chunks via `mutable_put`, each containing:
   ```json
   {"i": 0, "n": 5, "d": "<base64 chunk>", "next": "<hex public_key of next chunk>"}
   ```
   Final chunk has `"next": null`.
5. Print the root public key (hex) to stdout — this is the pickup key
6. Stay alive, refreshing all chunks every 5 minutes (within the 20-min DHT TTL)
7. Periodically check the **ack topic** (`blake2b(root_public_key || "ack")`) for pickup announcements
8. On pickup detected: log it. If `--max-pickups` reached, unannounce and exit.
9. On SIGINT/SIGTERM or `--ttl` expiry: stop refreshing and exit (records expire from DHT within ~20 min)

#### `peeroxide deaddrop pickup`

**Arguments:**

- `<key>` — the root public key (64-char hex) printed by `deaddrop leave`, OR a passphrase if sender used `--passphrase`

**Options:**

- `--public` — use public HyperDHT bootstrap nodes (default)
- `--bootstrap <addr>...` — use custom bootstrap nodes
- `--output <path>` — write output to file (default: stdout)
- `--no-ack` — don't announce pickup (stay fully silent)

**Behavior:**

1. Determine root public key: if `<key>` is 64-char hex, use directly. Otherwise treat as passphrase → derive root keypair → extract public key.
2. Fetch root chunk: `mutable_get(root_public_key, 0)` → parse JSON → learn total chunk count (`n`) and `next` pointer
3. Walk the linked list: fetch each `next` public key via `mutable_get` sequentially
4. Reassemble: concatenate decoded `d` fields in order
5. Write to `--output` or stdout
6. Unless `--no-ack`: briefly announce on the **ack topic** (`blake2b(root_public_key || "ack")`) using an ephemeral keypair, then exit. This lets the sender know a pickup occurred without revealing the receiver's identity.

#### Storage protocol (linked mutable records)

Each chunk is stored via `mutable_put` at `hash(chunk_public_key)`:

```
Chunk 0 (root keypair):     {"i": 0, "n": 5, "d": "base64...", "next": "ab12..."}
Chunk 1 (derived keypair):  {"i": 1, "n": 5, "d": "base64...", "next": "cd34..."}
Chunk 2 (derived keypair):  {"i": 2, "n": 5, "d": "base64...", "next": "ef56..."}
...
Chunk 4 (derived keypair):  {"i": 4, "n": 5, "d": "base64...", "next": null}
```

**Chunk keypair derivation:** `KeyPair::from_seed(blake2b(root_secret_key || chunk_index_le_bytes))`

The receiver only needs the root public key. From there, each chunk's `next` field provides the public key needed to fetch the following chunk.

**Why linked-list (not manifest)?** A manifest listing all chunk public keys in the root record would allow parallel fetching but caps total size at ~12KB (manifest must fit in one record). The linked-list approach has no size ceiling — limited only by refresh overhead and patience.

#### Pickup acknowledgement protocol

- **Ack topic:** `blake2b(root_public_key || "ack")` — deterministic from the pickup key, known to both sides
- **Receiver acks:** Announces on the ack topic with an ephemeral keypair, stays announced just long enough for the sender's next lookup cycle (~10 seconds), then exits
- **Sender monitors:** Periodic `lookup(ack_topic)` to discover announcing peers. Each unique public key seen = one pickup. The sender doesn't learn anything about the receiver except "someone picked it up."
- **Opt-out:** `--no-ack` skips this entirely for maximum receiver anonymity (no network activity after fetch)

#### Privacy model

| Property | Guarantee |
|---|---|
| Sender → Receiver identity | Hidden (ephemeral signing keys only; no handshake) |
| Receiver → Sender identity | Hidden (ack uses ephemeral key; sender sees count, not identity) |
| Sender IP vs Receiver IP | Never directly connected; mediated by separate DHT nodes |
| Data at rest on DHT | Signed but opaque to storage nodes (they see bytes, not semantics) |
| Data expiry | Auto-expires ~20 min after sender stops refreshing |
| Pickup key as capability | Knowing the root public key = ability to read. Treat as secret for private drops. |

#### Practical limits

- **~950 bytes per chunk** (1000 byte record minus JSON envelope + next pointer overhead)
- **Refresh cost:** N chunks × 1 mutable_put every 5 min. 100 chunks (~95KB file) = 100 puts/5min = manageable
- **Fetch latency:** N sequential DHT queries. Each is ~1-3 RTT. 100 chunks ≈ 5-30 seconds depending on network
- **Reasonable ceiling:** Files up to ~100KB are practical. Beyond that, latency and refresh costs become painful. For larger payloads, use `cp` (which requires a live connection but has no size limit).

#### Design decisions

- **No encryption layer in v1:** DHT records are signed (integrity) but not encrypted. Anyone with the pickup key can read the data. For confidentiality, encrypt the file before dropping (e.g., `age --encrypt file | peeroxide deaddrop leave -`). Adding built-in encryption is a v2 consideration.
- **Sequential fetch, not parallel:** The receiver can't know chunk N+1's public key without fetching chunk N. This is inherent to the linked-list design. A future "manifest mode" could trade size ceiling for parallelism.
- **Base64 encoding in JSON:** Adds ~33% overhead (950 usable → ~712 bytes of raw data per chunk). Binary framing could be more efficient but JSON keeps the format inspectable and simple for v1.
- **Passphrase mode:** Both sender and receiver derive the same root keypair from the passphrase. The receiver can derive the root public key without the sender needing to transmit it out-of-band. Useful for pre-arranged drops.

---

## Crate structure

```
peeroxide-cli/
  Cargo.toml
  src/
    main.rs          — clap entrypoint, subcommand dispatch
    cmd/
      mod.rs
      node.rs        — `peeroxide node` implementation
      keygen.rs      — `peeroxide keygen` implementation
      lookup.rs      — `peeroxide lookup` implementation
      announce.rs    — `peeroxide announce` implementation
      ping.rs        — `peeroxide ping` implementation
      cp.rs          — `peeroxide cp send/recv` implementation
      deaddrop.rs    — `peeroxide deaddrop leave/pickup` implementation
```

```toml
# Cargo.toml
[package]
name = "peeroxide-cli"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "peeroxide"
path = "src/main.rs"

[dependencies]
peeroxide = { path = "../peeroxide" }
peeroxide-dht = { path = "../peeroxide-dht" }
libudx = { path = "../libudx" }
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full", "signal"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
hex = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
indicatif = "0.17"
```

## Open questions

- What's the right output format for `keygen`? JSON, TOML, raw hex lines?
- Should the binary name conflict with the `peeroxide` library crate? (Cargo allows it but may confuse `cargo install` users.)
- `cp send`: should we support sending directories (tar on the fly) or just single files for v1?
- `cp`: should there be a confirmation prompt on the receiver side before accepting the file?
- `announce`: should re-announce interval be configurable or hardcoded?
- `deaddrop`: should the JSON chunk format use base64 or hex encoding for the `d` field? (base64 is denser; hex is consistent with the rest of the CLI's output)
- `deaddrop`: should `--passphrase` read from stdin interactively, or accept a `--passphrase <value>` argument? (Interactive is safer — no shell history. But less scriptable.)
- `deaddrop`: how long should the receiver's ack announcement persist? (Too short and sender misses it; too long and it's unnecessary network presence)
- `deaddrop`: should there be a `--encrypt` flag with built-in age/chacha20 encryption, or leave that to the user's pipeline?
