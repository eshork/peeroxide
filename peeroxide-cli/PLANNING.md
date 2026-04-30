# peeroxide-cli

Command-line interface for the peeroxide P2P networking stack.

Installs a single binary called `peeroxide` with subcommands for operating and interacting with the Hyperswarm-compatible network.

## Status

| Subcommand | Status | Notes |
|---|---|---|
| `node` | done | Firewalled=false, memory-only, oldest-first eviction, listen address on stdout |
| `lookup` | done | Identity-scoped --with-data, NDJSON with data_status, dedup+union relays, SIGINT handling |
| `announce` | done | Swarm-based; --data (identity-scoped mutable_put); --ping echo responder with hardening |
| `ping` | done | Progressive resolution pipeline: host:port / pubkey / topic → ping → optional connect |
| `cp send/recv` | done | Streaming transfer via swarm, no sender auth (topic=rendezvous only), --name, stdout mode, interactive prompt, --yes/--force, --timeout, stdin support, --keep-alive sequential |
| `deaddrop leave/pickup` | done | Binary framing (963 B/chunk), ack protocol, privacy model, exit codes, validation |

## Configuration

All subcommands share a common configuration system for network settings. This avoids repeating `--public --bootstrap addr1 addr2` on every invocation.

**Default config location:** `~/.config/peeroxide/config.toml`

**Global flags (available on every subcommand):**

- `--config <path>` — use this config file instead of the default location
- `--no-default-config` — ignore `~/.config/peeroxide/config.toml` entirely; use only CLI flags and compiled defaults
- `--public` / `--no-public` — override config's `network.public` setting
- `--bootstrap <addr>...` — override config's `network.bootstrap` list. Accepts `host:port` or `ip:port`.

**Env var:** `PEEROXIDE_CONFIG` — path to config file (equivalent to `--config`). CLI flag takes precedence over env var.

**Precedence (highest wins):** CLI flags → `--config` file → default config file (`~/.config/peeroxide/config.toml`) → compiled defaults

**Config file format (TOML):**

```toml
# Shared network settings — apply to ALL subcommands
[network]
public = false
bootstrap = ["10.0.1.5:49737", "10.0.1.6:49737"]

# Per-command settings
[node]
port = 49737
host = "0.0.0.0"
stats_interval = 60
max_records = 65536
max_lru_size = 65536
max_per_key = 20
max_record_age = 1200
max_lru_age = 1200

[announce]
# (future: default relay addresses, re-announce interval, etc.)

[cp]
# (future: default chunk size, keep-alive behavior, etc.)
```

All fields are optional. CLI flags for a given subcommand take precedence over that subcommand's config section, which takes precedence over `[network]` defaults.

**Platform paths:**
- Linux: `~/.config/peeroxide/config.toml` (respects `$XDG_CONFIG_HOME` if set)
- macOS: `~/Library/Application Support/peeroxide/config.toml` or `~/.config/peeroxide/config.toml` (either works; XDG path checked first for consistency with other CLI tools)
- Windows: `%APPDATA%\peeroxide\config.toml`

---

## Subcommands

### `peeroxide node`

Run a long-running, non-ephemeral DHT coordination (bootstrap) node. All state is in-memory — a restart loses the routing table and all stored records. The internal node ID (a random 32-byte value used for Kademlia routing) is also regenerated on each restart.

**Options:**

- `--port <port>` — bind port (default: 49737)
- `--host <addr>` — bind address (default: 0.0.0.0)
- `--stats-interval <seconds>` — how often to log routing table size (default: 60)
- `--max-records <n>` — max announcement records stored (library default: 65536). Maps to `PersistentConfig::max_records`.
- `--max-lru-size <n>` — max entries per LRU cache — applies to mutables, immutables, bumps, and refreshes caches independently (library default: 65536). Maps to `PersistentConfig::max_lru_size`.
- `--max-per-key <n>` — max peer announcements stored per topic (library default: 20). Maps to `PersistentConfig::max_per_key`.
- `--max-record-age <seconds>` — ⚠️ TTL for announcement records before expiry (library default: 1200 / 20 minutes). **Advanced:** changing this affects how long records survive without refresh. Peers expect a 20-minute window — reducing this may cause premature record loss; increasing it retains stale records longer.
- `--max-lru-age <seconds>` — ⚠️ TTL for LRU cache entries (mutables, immutables) before expiry (library default: 1200 / 20 minutes). **Advanced:** same protocol-coupling concerns as `--max-record-age`. Only change for isolated networks where you control all peer refresh intervals.

All storage flags map directly to `PersistentConfig` fields in the library — no translation layers or invented abstractions. When count limits are reached, oldest entries are evicted to make room; no requests are refused. Node-specific settings can also be set in the `[node]` section of the global config file (see Configuration above).

**When to tune storage limits:** On the public HyperDHT network (thousands of nodes), Kademlia naturally distributes records across the keyspace — a single node only stores records in its hash neighborhood, and the defaults (65536) are generous. On **private/small networks** (1–10 nodes), each node covers a much larger fraction of the keyspace and may store most or all records. Operators running private DHTs with many clients should increase `--max-records` and `--max-lru-size` accordingly. Note: the library's eviction algorithm is O(n) — at very high record counts (>500K), performance may degrade and library-level optimization would be needed.

**Estimated memory usage with defaults (no manual limits):**

| Store | Max entries | Worst-case size per entry | Estimated max |
|---|---|---|---|
| Announcement records | 65,536 | ~150 B (pubkey + encoded HyperPeer) | ~10 MB |
| Mutable records | 65,536 | ~1,464 B (hex key + value + seq + 64B signature) | ~96 MB |
| Immutable records | 65,536 | ~1,390 B (hex key + raw value) | ~91 MB |
| Bumps (dedup) | 65,536 | ~72 B | ~5 MB |
| Refreshes (dedup) | 65,536 | ~72 B | ~5 MB |
| Routing table | ~10,000 typical | ~50 B | ~0.5 MB |
| **Total** | | | **~200 MB** |

Worst case assumes every mutable/immutable record is at the maximum payload that fits in a single UDP datagram (~1326 bytes at MTU_MAX=1500, minus ~174 bytes wire overhead). The library does not enforce a value size limit — it stores whatever arrives. In practice, most records will be smaller; realistic peak is ~100–150 MB.

**Behavior:**

- Runs non-ephemeral (`ephemeral: false`) and non-firewalled (`firewalled: false`) so the node persists in peers' routing tables and accepts inbound connections. Both are required for a functional bootstrap node — `firewalled: true` would use the client socket exclusively, preventing the node from advertising its ID to peers.
- On startup: prints the bound listen address (`host:port`) to **stdout** (not via the logging system) so it is always visible regardless of `RUST_LOG` setting. This is essential operational output — operators need it to share the node as a bootstrap peer. Note: the library does not expose the internal node ID (a random 32-byte value regenerated each restart), so only the listen address is printed.
- Mode depends on global network flags:
  - **Isolated** (no `--public`, no `--bootstrap`): bootstrap completes immediately (nothing to discover). Logs: `"Node ready (isolated mode) — listening for incoming peers"`. The node sits with an empty routing table until other nodes contact it directly.
  - **Networked** (`--public` and/or `--bootstrap`): bootstrap completes when the initial FIND_NODE query finishes. Note: this means the query ran, not that peers were found — the routing table may still be empty if bootstrap peers were unreachable. Logs: `"Bootstrap complete — routing table: N peers"` (N may be 0).
- Periodically logs routing table size at the configured `--stats-interval`. Also reports routing table size immediately after bootstrap completes (first data point before the periodic cycle begins).
- **Empty routing table warning (networked mode only):** If the routing table is empty at the first `--stats-interval` after bootstrap, log: `warn: "Routing table empty {N}s after bootstrap — this node may be unreachable. Check that UDP port {port} is open and not firewalled."` If still empty at the second interval, log once more: `warn: "Routing table still empty after {N}m — node is likely unreachable from the network. Verify UDP port {port} is reachable from external hosts."` Then stop warning — the operator has been told. Does not apply to isolated mode, where 0 peers is expected.
- Handles SIGINT/SIGTERM for graceful shutdown via `destroy()`

**Storage & eviction:**

A non-ephemeral DHT node stores data on behalf of the network — not just routing pointers. This includes:
- **Announcement records** — which peers announced on which topics (serves LOOKUP queries)
- **Mutable records** — signed key-value pairs from `mutable_put` (serves MUTABLE_GET queries)
- **Immutable records** — content-addressed data from `immutable_put` (serves IMMUTABLE_GET queries)

**Eviction policy:** Oldest-first. When at capacity (`--max-records` or `--max-lru-size`), the oldest-inserted entries are dropped to make room for new writes. This is insertion-order eviction, not true LRU — reading/querying a record does not update its age. No requests are refused — the HyperDHT protocol has no "storage full" error, and Kademlia distributes data across the K closest nodes, so eviction from one node doesn't mean data loss from the network.

**TTL:** All records expire after `--max-record-age` / `--max-lru-age` (default 20 minutes) regardless of storage caps. Peers must refresh periodically to keep data alive.

**Future enhancement — `--data-dir` (disk persistence):** The library currently has no save/load support — `RoutingTable` and `Persistent` storage are purely in-memory with no serialization, no serde derives, and no external access to snapshot state. Adding disk persistence requires library-level changes: exposing export/import methods on `DhtHandle`/`HyperDhtHandle` and adding serialization to internal types. Deferred to a future version.

**Log levels** (controlled via `RUST_LOG` environment variable):

| Level | Output |
|---|---|
| `error` | Fatal failures — socket bind errors, bootstrap failure, unrecoverable state |
| `warn` | Peers failing verification, dropped connections, timeouts |
| `info` | Listen address, bootstrap complete, periodic routing table size, shutdown signal received |
| `debug` | Individual query/response traffic, routing table add/remove, NAT detection |
| `trace` | Wire-level bytes, per-packet encode/decode |

---

---

### `peeroxide lookup`

Query the DHT for peers announcing a topic.

**Arguments:**

- `<topic>` — target topic. Same grammar as other commands: 64-char hex = raw topic hash (used as-is), plain text = topic name (hashed via BLAKE2b-256).

**Options:**

- `--with-data` — also fetch metadata (via `mutable_get`) for each unique discovered peer (slower: runs an additional iterative DHT query per peer). **Important:** metadata is **identity-scoped** (`hash(public_key)`), NOT topic-scoped. If a peer uses the same identity to announce multiple topics with different `--data`, the value returned here reflects their most recent write — which may have been for a different topic.
- `--json` — output as NDJSON (one JSON object per line to stdout; warnings/errors to stderr). Default: human-readable list.

**Behavior:**

Uses `HyperDhtHandle` directly (not the swarm — no keypair or announcement needed for read-only queries).

1. Bootstrap the DHT node, then call `HyperDhtHandle::lookup(topic) -> Vec<LookupResult>`.
   - Each `LookupResult` contains `from: Ipv4Peer` (the DHT node that replied), `to: Option<Ipv4Peer>` (routing detail), and `peers: Vec<HyperPeer>` (announced peers it knows about, each with `public_key: [u8; 32]` and `relay_addresses: Vec<Ipv4Peer>`).
2. **Deduplicate** peers across all `LookupResult`s by public key. Multiple DHT nodes may report the same peer — emit each unique peer only once. **Relay address merge:** union all relay addresses seen for a given pubkey, deduped by exact `host:port` string. This handles replica skew where different DHT nodes may have slightly different relay sets.
3. **Output order:** Preserve first-seen discovery order (the order in which each unique pubkey is first encountered during flattening of results). This gives deterministic output tied to query topology rather than arbitrary hash ordering.
4. If `--with-data`: for each **unique** peer, call `HyperDhtHandle::mutable_get(&peer.public_key, 0)` to retrieve associated metadata. `seq=0` means "accept any response with seq ≥ 0" — this returns the first valid replica response, which is **typically** the latest value but is **not guaranteed** to be the absolute newest across all replicas under concurrent writes. These queries run concurrently (up to 16 in-flight). Individual `mutable_get` failures (timeout, network error) are non-fatal — report that peer with an error status.
5. Print results and summary.

**Deduplication rationale:** The iterative lookup contacts ~20 DHT nodes near the topic. Each may independently report the same announced peer (the peer's record is replicated). Without dedup, the same pubkey would appear 5–20× in output, which is useless noise.

**Output (human-readable, default):**

```
$ peeroxide lookup "my-app"
LOOKUP blake2b("my-app")
  found 3 peers

  @cd34ef56...full64charhex
    relays: 1.2.3.4:49876, 5.6.7.8:51234

  @ab12cd34...full64charhex
    relays: (direct only)

  @99887766...full64charhex
    relays: 10.0.0.1:40000

$ peeroxide lookup "my-app" --with-data
LOOKUP blake2b("my-app")
  found 2 peers

  @cd34ef56...full64charhex
    relays: 1.2.3.4:49876
    data: "v2.1.0 port=8080" (seq=1745773200)

  @ab12cd34...full64charhex
    relays: (direct only)
    data: (not stored)

$ peeroxide lookup "empty-topic"
LOOKUP blake2b("empty-topic")
  found 0 peers
```

**Human-mode rendering rules:**
- UTF-8 data: Print in double quotes with Rust-style escaping for control characters (`\n`, `\t`, `\x1b`, etc.). No raw newlines or terminal escapes in output.
- Non-UTF-8 data: Print as `0x<hex>` (no quotes).
- Relay addresses: Comma-separated `host:port` list, or `(direct only)` if empty.

**Output (JSON mode, NDJSON):**

NDJSON records go to stdout only. Warnings and errors go to stderr.

```
$ peeroxide lookup "my-app" --json
{"type":"peer","public_key":"cd34ef56...64chars","relay_addresses":["1.2.3.4:49876","5.6.7.8:51234"]}
{"type":"peer","public_key":"ab12cd34...64chars","relay_addresses":[]}
{"type":"peer","public_key":"99887766...64chars","relay_addresses":["10.0.0.1:40000"]}
{"type":"summary","topic":"cd34ef56...64char_topic_hash","peers_found":3}

$ peeroxide lookup "my-app" --with-data --json
{"type":"peer","public_key":"cd34ef56...64chars","relay_addresses":["1.2.3.4:49876"],"data_status":"ok","data":"v2.1.0 port=8080","seq":1745773200}
{"type":"peer","public_key":"ab12cd34...64chars","relay_addresses":[],"data_status":"none"}
{"type":"peer","public_key":"99887766...64chars","relay_addresses":[],"data_status":"error","error":"timeout"}
{"type":"summary","topic":"cd34ef56...64char_topic_hash","peers_found":3}

$ peeroxide lookup "empty-topic" --json
{"type":"summary","topic":"ab99...64char_topic_hash","peers_found":0}
```

**JSON schema:**

Peer record (one per discovered peer):
```json
{
  "type": "peer",
  "public_key": "<64-char hex>",
  "relay_addresses": ["host:port", ...]
}
```

With `--with-data`, peer records include metadata fields:
```json
{
  "type": "peer",
  "public_key": "<64-char hex>",
  "relay_addresses": ["host:port", ...],
  "data_status": "ok" | "none" | "error",
  "data": "<string or null>",
  "data_encoding": "utf8" | "hex",
  "seq": <number or null>,
  "error": "<message, only when data_status=error>"
}
```

- `data_status`: `"ok"` = metadata retrieved successfully, `"none"` = no record stored for this identity, `"error"` = mutable_get failed (see `error` field).
- `data_encoding`: Present only when `data_status` is `"ok"`. `"utf8"` for valid UTF-8 strings; `"hex"` for binary data (value is hex-encoded string without `0x` prefix).
- `data`/`seq`: `null` when `data_status` is `"none"` or `"error"`.
- Without `--with-data`: `data_status`, `data`, `data_encoding`, `seq`, `error` fields are all omitted.

Summary record (always last line):
```json
{
  "type": "summary",
  "topic": "<64-char hex topic hash>",
  "peers_found": <number>
}
```

**Edge cases:**

- **0 peers found:** Print "found 0 peers" (human) or summary-only (JSON) and exit 0. This is not an error — it means nobody is currently announced on this topic (or all announcements have expired).
- **DHT unreachable (bootstrap fails):** Exit 1 with error message to stderr. No NDJSON output.
- **Partial mutable_get failures:** Report affected peers with `data_status: "error"` in JSON, warning to stderr in human mode. Do not fail the whole command.
- **Non-UTF-8 data:** Display as hex (see rendering rules above).
- **SIGINT during operation:** Exit 130. In human mode, print partial results gathered so far. In JSON mode, emit peer records already collected plus a summary with actual count emitted.

**Performance note:** Without `--with-data`, lookup is a single iterative DHT query (contacts ~20 nodes, completes in seconds on a healthy network). With `--with-data`, each unique peer triggers an additional full iterative query at a *different* DHT coordinate (`hash(public_key)` vs the topic hash). For N unique peers, that's N+1 total queries. There is no signal in the lookup response indicating whether a peer has stored metadata — you must query to find out. The concurrent cap of 16 prevents UDP socket exhaustion on large peer sets.

**Exit codes:**

- `0` — lookup completed (regardless of whether peers were found; 0 peers is valid)
- `1` — fatal error (bootstrap failed, DHT unreachable, network error during lookup itself)
- `130` — interrupted by SIGINT (partial results printed before exit)

**Implementation notes:**

- Uses `HyperDhtHandle` directly via `hyperdht::spawn()`. No swarm needed — lookup is a read-only query that doesn't require a keypair or announcement.
- `DhtConfig` should use the same bootstrap nodes as other commands (from global `--bootstrap` option or default public bootstrap).
- Dedup: Flatten all `HyperPeer`s from all `LookupResult`s. Use an `IndexMap<[u8; 32], Vec<Ipv4Peer>>` (or equivalent ordered map) keyed by public_key to preserve first-seen order. Union relay addresses per key, deduped by `format!("{}:{}", host, port)`.
- `mutable_get` concurrency: Use `futures::stream::iter(...).buffer_unordered(16)` or equivalent to cap in-flight queries.
- After all work completes, call `dht.destroy()` then await the spawned task handle for clean shutdown (releases UDP socket).
- The `from` and `to` fields in `LookupResult` (DHT routing details) are intentionally NOT exposed in output — they are implementation details of the iterative query, not useful to CLI users.

---

### `peeroxide announce`

Announce presence on a topic so other peers can discover you.

**Arguments:**

- `<topic>` — target topic. Same grammar as `ping`: 64-char hex = raw topic hash (used as-is), plain text = topic name (hashed via BLAKE2b-256).

**Options:**

- `--seed <string>` — seed to derive a deterministic keypair. Any string is accepted (hashed via BLAKE2b-256 to produce 32-byte seed). Same seed = same identity across runs. **Treat as secret** — anyone with the seed can impersonate this identity. If omitted, a random ephemeral keypair is generated.
- `--data <string>` — metadata to store on the DHT (max 1000 UTF-8 bytes after shell parsing). Stored via `mutable_put` **keyed by the announce keypair's public key** (identity-scoped, NOT topic-scoped — see note below). Retrievable by any peer that knows the public key.
- `--duration <seconds>` — how long to stay announced (default: indefinite until SIGINT). Must be > 0.
- `--ping` — accept incoming connections and run the echo protocol (enables `ping --connect` from remote peers)

**Metadata scoping:** `--data` is stored under `hash(public_key)`, making it **one record per identity**, not per topic. If you use the same `--seed` to announce on multiple topics with different `--data`, only the last write is visible. This is a DHT protocol constraint, not a bug. For per-topic metadata, use different seeds per topic.

**Behavior:**

Uses the `peeroxide` swarm crate. Spawns a swarm node with the provided keypair, then:

1. `handle.join(topic, JoinOpts { server: true, client: false })` — announces on the topic. The swarm automatically re-announces at a 10-minute interval (with jitter), well within the 20-minute DHT record TTL.
2. If `--data` provided: calls `handle.dht().mutable_put(handle.key_pair(), data.as_bytes(), seq)` to store metadata. Uses current Unix timestamp as `seq`. A background timer re-puts every 10 minutes with a fresh timestamp to keep data from expiring. **Failure handling:** initial or refresh `mutable_put` failures emit a warning to stderr but do NOT abort the announce — the announcement itself is the primary operation.
3. Prints initial status: public key (full hex), topic hash, number of closest nodes announced to.
4. Enters event loop, accepting connections from `conn_rx` (if `--ping`).
5. On SIGINT/SIGTERM: calls `handle.leave(topic)` then `handle.destroy()`. Unannounce is best-effort — if the process is killed hard (`kill -9`) or crashes, the stale announce record persists on the DHT until TTL expiry (~20 minutes).

**`--ping` echo responder:**

When `--ping` is active, for each incoming `SwarmConnection` from `conn_rx`:
1. Spawn a task for the connection (max 64 concurrent echo sessions — reject beyond this with connection close)
2. Read first message with a **5-second handshake timeout**
3. If it is the 4-byte magic `"PING"` (0x50494E47): respond with `"PONG"` (0x504F4E47), then enter echo loop
4. If first message is NOT the ping magic, OR handshake times out: close connection immediately
5. **Echo loop:** read message, write same bytes back — but ONLY for exactly 16-byte messages. Any other message size is a **protocol violation → close connection immediately** (do not leave the connection open).
6. **Idle timeout:** if no message received within 30 seconds, close connection
7. On client disconnect: clean up task, log event
8. Log all connect/disconnect events to stderr (with remote public key)

**Without `--ping`:** Incoming connections from `conn_rx` are drained and dropped silently. The swarm handles the handshake automatically (necessary for announce to work), but the CLI has no application protocol to serve.

**Data size limit:** The `--data` value must be ≤ 1000 bytes (measured as UTF-8 byte length of the string after shell parsing). This cap is conservative — DHT messages are single UDP datagrams, and 1000 bytes of payload leaves ample room for protocol overhead within the network's base MTU.

**Output (human-readable):**

```
$ peeroxide announce "my-app" --ping
ANNOUNCE blake2b("my-app") as @cd34ef56...full64charhex (ephemeral)
  announced to 20 closest nodes
  listening for echo connections...
  [connected] @ab12...ef34 (echo mode)
  [disconnected] @ab12...ef34 (3 probes echoed)
  ^C
UNANNOUNCE blake2b("my-app")
  done

$ peeroxide announce "my-app" --seed "my-stable-identity" --data "v2.1.0 port=8080"
ANNOUNCE blake2b("my-app") as @cd34ef56...full64charhex
  announced to 20 closest nodes
  metadata: "v2.1.0 port=8080" (16 bytes, seq=1745773200)
  ^C
UNANNOUNCE blake2b("my-app")
  done
```

**Exit codes:**

- `0` — announced successfully, exited cleanly (SIGINT, SIGTERM, or `--duration` elapsed)
- `1` — runtime/network error (bootstrap failed, announce failed, DHT unreachable)

**Implementation notes:**

- Keypair: If `--seed` provided, hash the string via BLAKE2b-256 to get 32 bytes, then `KeyPair::from_seed(seed)`. If omitted, `KeyPair::generate()` for a random ephemeral identity. Either way, print the **full** public key on startup (64-char hex with `@` prefix) so the user can reference it with `ping @<key>`.
- **Single-writer assumption:** `seq = Unix timestamp` works correctly for a single announcer per identity. Concurrent same-seed announcers racing to `mutable_put` are explicitly unsupported (last write wins, data may oscillate). Clock rollback on the same machine is accepted as an unlikely edge case.
- `SwarmConfig.key_pair` accepts our keypair, so the swarm uses it for both announce signatures and Noise handshakes.
- Without `--ping`: `conn_rx` is drained in the background (incoming connections accepted by swarm but immediately dropped by CLI).
- The swarm's `conn_rx` delivers `SwarmConnection` which contains `peer: PeerConnection` with `stream: SecretStream<UdxAsyncStream>` — ready for reading/writing messages immediately.

---

### `peeroxide ping`

Diagnose reachability of a DHT node or peer. Resolves the target step-by-step, reports each stage (discovery, UDP ping, optional full connection), and fails with clear output indicating which step broke.

**Arguments:**

- `<target>` — one of (unambiguous grammar via prefix convention):
  - `host:port` — direct UDP ping (CMD_PING), no resolution needed. Detected by presence of `:` followed by digits.
  - `@<64-char hex>` — public key (prefix `@` = "target this identity"). Resolves via FIND_PEER, then pings discovered addresses.
  - `<64-char hex>` (no prefix) — raw topic hash, used as-is. Resolves via LOOKUP (find announcing peers), then pings each.
  - `<plain text>` (not hex, no `@`, no `:port`) — topic name, hashed via BLAKE2b-256 to produce the topic key. Resolves via LOOKUP.

**Options:**

- `--count <n>` — number of probes to send (default: 1). Use `--count 0` for infinite (ping until SIGINT). When combined with `--connect`, this controls echo probes over the encrypted channel (UDP pre-check is always 1 probe regardless of `--count`).
- `--interval <seconds>` — delay between sends (default: 1). Accepts decimal values (e.g. `0.1`, `0.01`). Minimum precision: 1ms. Use `0` for no delay (flood). Only meaningful with `--count` > 1 or `--count 0`.
- `--connect` — after ping phase, attempt a full Noise handshake (holepunch + relay if needed). Requires generating an ephemeral local keypair. Verifies end-to-end encrypted connectivity, not just UDP reachability.
- `--json` — output as NDJSON (newline-delimited JSON). Each event is one JSON object per line: resolution steps, individual probe results, and a final summary object on completion or SIGINT. Enables real-time processing by tools like `jq`. Event types: `{"type":"resolve",...}`, `{"type":"probe",...}`, `{"type":"connect",...}`, `{"type":"summary",...}`.

**Behavior:**

Resolution pipeline (stops at first failure, reports what broke):

1. **Parse target format:**
   - `host:port` (contains `:` + trailing digits) → skip to step 3 (direct ping)
   - `@<64-char hex>` → treat as public key → step 2a
   - `<64-char hex>` (no `@`) → treat as raw topic hash → step 2b (used as-is, no hashing)
   - anything else → treat as topic name (hash via BLAKE2b-256) → step 2b

2. **Resolve target to addresses:**
   - **(2a) Public key** (`@<hex>`): calls `HyperDhtHandle::find_peer(pubkey)`. Returns a `HyperPeer` record containing relay addresses. If not found: fail with `"Peer not found on DHT — no nodes have a record for this public key."`
   - **(2b) Topic** (name or raw hash): calls `HyperDhtHandle::lookup(topic)`. Returns `HyperPeer` records, each containing a public key and relay addresses. No subsequent `find_peer` needed — `lookup` provides everything required for both pinging (addresses) and connecting (pubkey). If no peers: fail with `"No peers announcing on this topic."` If more than 20 peers found, only the first 20 are used (note in output: `"showing 20 of N peers"`).

3. **UDP ping** (CMD_PING): For each resolved address, sends **one** probe via `DhtHandle::ping(host, port)`.
   - Reports per-address: reachable/unreachable, RTT (ms), retries (if non-overlapping mode and retries > 0)
   - The `node_id` returned by CMD_PING identifies the DHT node at that address, which for pubkey/topic targets is typically a relay node — **not** the target peer itself. Display it for direct `host:port` targets; omit for resolved targets to avoid confusion.
   - If all unreachable and `--connect` not given: fail with `"All resolved addresses unreachable — target may be offline or firewalled."`
   - If `--connect` not given and `--count` > 1 or `--count 0`: repeats UDP pings per step 4.

4. **Repeat UDP pings** (only without `--connect`, if `--count` > 1 or `--count 0`): Wait `--interval` seconds between sends, then repeat step 3. Probes are fire-and-forget — do not wait for a response before sending the next. On SIGINT, stop and print summary.

5. **Full connection** (only with `--connect`): Attempted **regardless of ping results** (firewalled peers fail UDP ping but may be reachable via holepunch/relay). Calls `HyperDhtHandle::connect(ephemeral_keypair, remote_pubkey, runtime)` for one resolved peer.
   - Reports: success/failure and total connection time
   - On failure: maps `HyperDhtError` variant to human-readable stage description:
     - `PeerNotFound` → "peer not found on DHT"
     - `NoRelayNodes` → "no relay nodes available"
     - `HolepunchFailed` → "holepunch failed"
     - `HolepunchAborted` → "holepunch aborted by remote"
     - `FirewallRejected` → "remote firewall rejected connection"
     - `HandshakeFailed(msg)` → "Noise handshake failed: {msg}"
     - `StreamEstablishment(msg)` → "stream setup failed: {msg}"
     - `Udx(e)` / `SecretStream(e)` / `Relay(e)` → "transport error: {e}"
   - Note: On success, we cannot determine whether the connection used holepunch, relay, or direct path — that information is not exposed by the current API. Only failure variants reveal which stage broke.

6. **Echo pings over SecretStream** (only with `--connect`, after successful connect): `--count` restarts here — sends `--count` echo probes over the encrypted channel (default 1, `--count 0` = infinite until SIGINT). Requires remote to be running `announce --ping`.
   - **Send model:** Non-blocking — probes are sent at `--interval` rate without waiting for responses. If `--interval 0`, sends as fast as the transport accepts writes. When UDX backpressure is hit (write buffer full), the sender slows to match available throughput.
   - **Receive model:** Responses are matched by sequence number. Latency = `now - embedded_timestamp` for each matched pong.
   - Reports per-probe: latency over the encrypted path (application-level, includes transport reliability overhead)
   - On SIGINT or `--count` reached: print summary and disconnect.
   - Note: `connect()` is the only way to test reachability of firewalled/NATted peers. Direct UDP ping will always fail against them because `connect()` uses a separate socket with holepunch/relay negotiation that the DHT RPC layer does not share.

**Retry detection via IoStats deltas:**

The RPC layer retries failed UDP packets up to 3 times before declaring timeout (DEFAULT_RETRIES = 3). Per-probe retry count is obtained by snapshotting the aggregate `IoStats.retries` counter before and after each `ping()` call.

- `retries = 0` → first packet got a response (clean)
- `retries = 1–2` → some packets were lost but response eventually arrived (degraded)
- `retries = 3` + timeout → all 4 attempts failed (unreachable)

**Accuracy constraint:** Per-probe retry attribution is only valid when probes do not overlap in flight (i.e., each `ping()` completes before the next is sent). At the default `--interval 1`, this is always true. When `--interval` is low enough that probes overlap, per-probe retries are **omitted** from individual output lines — only aggregate retry/loss stats appear in the final summary. The implementation tracks whether any probe is still in-flight when the next is dispatched; if so, it switches to aggregate-only mode for the remainder of the run.

**Note on background noise:** The IoStats counter is process-global. DHT housekeeping (routing table refresh, etc.) could theoretically increment retries during a probe. In practice this is negligible for a short-lived CLI process, but the summary should be labeled "approximate" in documentation.

**Library addition required:** `DhtHandle::stats() -> IoStats`. The `IoStats` struct is already public and derives `Clone`; the accessor returns a read-only snapshot (same pattern as existing `DhtHandle::table_size()`). The JS reference exposes equivalent aggregate stats via `node.stats` — this is parity, not a new feature. No wire protocol changes, no compliance risk.

**Output (human-readable):**

```
$ peeroxide ping 192.168.1.5:49737
PING 192.168.1.5:49737 (direct)
  [1] OK  12ms  node_id=ab12...ef34

$ peeroxide ping 192.168.1.5:49737 --count 0
PING 192.168.1.5:49737 (direct)
  [1] OK  12ms
  [2] OK  14ms
  [3] OK  45ms (2 retries)
  [4] TIMEOUT (3 retries, no response)
  [5] OK  11ms
  ^C
--- 192.168.1.5:49737 ping statistics ---
5 probes, 4 responded, 1 timed out (20% probe loss)
8 packets tx, 4 packets rx (50% datagram loss)
rtt min/avg/max = 11/20.5/45 ms

$ peeroxide ping @ab12...ef34
RESOLVE find_peer(ab12...ef34): found, 2 relay addresses
PING 203.0.113.5:49737
  [1] OK  45ms
PING 198.51.100.2:49737
  [1] TIMEOUT (3 retries, no response)

$ peeroxide ping "my-app" --connect
RESOLVE lookup(blake2b("my-app")): 1 peer found (cd34..., 1 address)
PING 203.0.113.5:49737
  [1] TIMEOUT — direct UDP unreachable (firewalled)
CONNECT cd34...
  OK (250ms)
ECHO cd34...
  [1] OK  48ms (e2e encrypted)

$ peeroxide ping "my-app" --connect --count 0
RESOLVE lookup(blake2b("my-app")): 1 peer found (cd34..., 1 address)
PING 203.0.113.5:49737
  [1] TIMEOUT — direct UDP unreachable (firewalled)
CONNECT cd34...
  OK (250ms)
ECHO cd34...
  [1] OK  48ms (e2e encrypted)
  [2] OK  45ms (e2e encrypted)
  [3] OK  52ms (e2e encrypted)
  ^C
--- cd34... echo statistics ---
3 probes sent, 3 responded, 0 timed out
latency min/avg/max = 45/48.3/52 ms
throughput: ~0.1 KB/s (32 bytes x 3 round-trips in 3.1s)

$ peeroxide ping "my-app" --connect --count 1000 --interval 0
RESOLVE lookup(blake2b("my-app")): 1 peer found (cd34..., 1 address)
PING 203.0.113.5:49737
  [1] TIMEOUT — direct UDP unreachable (firewalled)
CONNECT cd34...
  OK (250ms)
ECHO cd34... (flood mode)
  ...1000 probes sent
--- cd34... echo statistics ---
1000 probes sent, 998 responded, 2 timed out
latency min/avg/max = 22/47.1/310 ms
throughput: ~1.3 KB/s (32 bytes x 998 round-trips in 47.0s)
```

**Summary line explanation:**
- **Probe loss (UDP mode):** Application-level — did the `ping()` call succeed? (1 timeout out of 5 probes = 20%)
- **Packet loss (UDP mode only):** RPC datagram efficiency — how many UDP datagrams were sent (including retransmissions) vs how many successful responses came back. Computed as: `total_packets_sent = probes + total_retries`, `packets_received = successful_probes`. This is not true network-level packet loss (we can't observe individual drops), but rather a measure of RPC overhead from retransmissions. Available because `DhtHandle::stats()` exposes DHT RPC layer retries.
- **Probe timeout (echo mode):** Application-level only — no pong received within deadline. We cannot report underlying UDP packet loss or retransmission counts because UDX stream transport stats (`StreamInner.rto_timeouts`, retransmit counts) are not publicly accessible.
- **Throughput (echo mode only):** Average effective data rate achieved over the encrypted path. Computed as `(payload_size * successful_round_trips * 2) / elapsed_time` (×2 for send + receive). Reports a single average value in human-friendly units (KB/s or MB/s depending on magnitude). No sliding windows — just total bytes over total time.

**Implementation notes:**

- Step 2b (topic → peers) discovers public keys, but CMD_PING targets `host:port`. The relay addresses from the LOOKUP/FIND_PEER response are what gets pinged. If a peer has no relay addresses in its record (announced without relays), it cannot be pinged by pubkey/topic — report: `"Peer ab12... has no advertised addresses — cannot ping."`
- `--connect` requires a public key (needs it for Noise handshake). Works when target is a `@pubkey` directly, or when topic resolution yielded peers (each has a pubkey in its `HyperPeer` record). If topic yields multiple peers, attempts connection to each sequentially until one succeeds — reports which peer connected. If all fail, reports each failure.
- The ephemeral keypair generated for `--connect` is discarded after the probe. The remote peer sees a connection from an unknown key — this is fine for diagnostics.
- **Timeouts:** Resolution stages (`lookup`, `find_peer`, `connect`) use library-default timeouts and retry behavior. The CLI does not impose additional timeouts beyond what the DHT/connection APIs already enforce.
- **Library addition (approved):** Add `DhtHandle::stats() -> Result<IoStats, DhtError>` — a read-only clone of the internal `Io.stats` counter, sent via `DhtCommand::Stats` + oneshot (mirrors existing `table_size()` pattern). ~10 lines of code in `peeroxide-dht/src/rpc.rs`.

**Application-level echo protocol (for `--connect` pings):**

Used between `ping --connect` (client) and `announce --ping` (responder) over SecretStream:

- **Handshake:**
  1. Client sends 4-byte magic `"PING"` (0x50494E47) as first message after connect
  2. Responder recognizes the magic and replies with 4-byte `"PONG"` (0x504F4E47) to confirm protocol support
  3. If responder does not reply within 5 seconds, client reports: `"Remote peer does not support echo protocol (not running announce --ping?)"`
  4. After PONG received, client enters probe loop

- **Ping message:** 16 bytes — `[8B sequence number (LE u64)][8B timestamp nanos (LE u64)]`
- **Pong message:** responder echoes the exact 16 bytes back unchanged (only for 16-byte messages after handshake)
- **Latency:** client measures `now - embedded_timestamp` on pong receipt. Timestamps use `Instant::now()` (monotonic clock) converted to nanos — immune to wall-clock adjustments since both send and receive happen in the same process.
- **Timeout:** if no pong within 5 seconds for a given probe, mark that probe as timed out. Late pongs (arriving after deadline) are silently discarded — they do not retroactively clear a timeout or affect stats.
- **Responder logic:** After sending PONG, enter echo loop — read message, write same message back. Only echo messages that are exactly 16 bytes; ignore/discard unexpected sizes (future extensibility). On client disconnect, clean up and log.
- **No framing needed:** SecretStream is message-oriented — one `write()` = one `read()` on the other side

**Measurement terminology:**

- **RTT** — used for UDP datagram probes (CMD_PING). Measured from first send to response receipt. **Note:** When retries occur, this includes the time spent waiting for timed-out attempts, so a probe with retries will report inflated RTT (it's "time to eventual response," not pure network round-trip). The summary `rtt min/avg/max` only includes successful probes; timed-out probes are excluded.
- **Latency** — used for echo probes over SecretStream. Measures application-level delivery time through the full reliable stack (encryption + UDX ordered delivery + congestion control). Includes retransmission delays if packets are lost at the transport layer. Under no-loss conditions, approximates RTT + crypto overhead.

**Exit codes:**

- `0` — all probes succeeded (100% response rate)
- `1` — partial or total failure (some/all probes timed out, resolution failed, connect failed)
- `130` — interrupted by SIGINT (summary still printed before exit)

---

### `peeroxide cp`

Copy files between peers over the swarm. Direct encrypted connection — no data stored on DHT nodes.

**Subcommands:**

```
peeroxide cp send <file|->           # sender: announce on topic, print topic, stream file to receiver
peeroxide cp recv <topic> [dest]     # receiver: connect to sender, download file (dest=- for stdout)
```

#### `peeroxide cp send`

**Arguments:**

- `<file|->` — path to file to send, or `-` for stdin. Directories are not supported in v1 (use `tar` externally). When sending stdin, the metadata header reports filename as `"stdin"` and size as `null` (unknown length — stream until EOF).

**Options:**

- `[topic]` — optional positional: a 64-char hex string or a plain name (hashed via BLAKE2b-256). If omitted, a random 32-byte topic is generated and the corresponding hex is printed. The topic is the rendezvous point — anyone who knows it can connect.
- `--name <filename>` — override the filename reported in the metadata header. Useful when piping from stdin (e.g., `tar c dir/ | peeroxide cp send - --name dir.tar`). If omitted: uses basename of the file path, or `"stdin"` for `-`.
- `--keep-alive` — don't exit after first transfer; wait for the next connection (sequential, one at a time). Without this flag, exit after the first successful transfer. **Incompatible with stdin (`-`)** — stdin is one-shot; fail fast with error if both are specified.

**Behavior:**

Uses `peeroxide::spawn()` (full swarm, not just DHT handle).

1. Determine topic: use provided topic (64-char hex = raw bytes, otherwise BLAKE2b-256 hash of the plaintext name) or generate a random 32-byte topic.
2. Validate input: for regular files, stat the file and confirm it exists and is readable. Fail fast with exit 1 if not. For stdin (`-`), skip validation.
3. Spawn swarm and join the topic in server mode (`JoinOpts { server: true, client: false }`), announcing presence.
4. Flush the swarm (waits for initial announce attempt to complete — note: this is best-effort; flush signals that the announce cycle ran, not that all DHT nodes accepted the record. In rare cases of total network failure, the topic may be printed but not yet discoverable. This is acceptable — the receiver has `--timeout` to handle this gracefully).
5. Print the topic to stdout:
   - If a plaintext name was provided: echo back the name (receiver can use the same name).
   - If 64-char hex was provided: echo back the hex.
   - If random: print the 64-char hex of the generated topic.
   - Print **only** this line to stdout. All other output goes to stderr.
6. Wait for incoming connections via the swarm connection receiver channel.
7. On connection:
   - If a transfer is already in progress (`--keep-alive` mode): reject the connection (drop it immediately). Only one transfer at a time.
   - For regular files: open and stat the file (this should succeed — file was validated in step 1).
   - For stdin: begin reading from stdin on-demand after connection is established. Data is streamed directly — no pre-buffering.
   - Send metadata header (message 1): JSON `{"filename": "<name>", "size": <bytes|null>, "version": 1}`.
     - `filename`: value of `--name` if provided, otherwise basename of the file path, or `"stdin"` for `-` without `--name`.
     - `size`: file size in bytes (from stat), or `null` for stdin (unknown length).
   - Stream file data in 64KB messages (messages 2..N). Read from file/stdin, write to SecretStream.
   - After all data sent: close the connection (signals EOF to receiver).
   - Log transfer summary to stderr: bytes sent, duration, average speed.
8. If `--keep-alive`: return to step 6. Otherwise: clean up and exit 0.
9. On SIGINT/SIGTERM:
   - If mid-transfer: close the connection (receiver sees unexpected EOF → abort).
   - Clean up and exit 130.

**Cleanup:** Call `handle.destroy()` which tears down the swarm. Announcement TTL on the DHT will expire naturally (~20 min). There is no public `unannounce` on `SwarmHandle` — `leave()` only cancels discovery, it does not actively unannounce. Best-effort teardown via `destroy()` is the correct approach.

**Output (stderr):**

```
$ peeroxide cp send archive.tar.gz
# stdout:
a1b2c3d4...64chars
# stderr:
CP SEND archive.tar.gz (15.2 MB)
  topic: a1b2c3d4...
  waiting for receiver...
  connected from @ef01... (holepunch)
  streaming... 15.2 MB / 15.2 MB [100%]
  done: 15.2 MB in 4.3s (3.5 MB/s)
```

**Exit codes:**

- `0` — transfer(s) completed successfully
- `1` — fatal error (file not found, file read error, swarm bootstrap failed)
- `130` — interrupted by SIGINT

#### `peeroxide cp recv`

**Arguments:**

- `<topic>` — the topic from the sender. 64-char hex = raw topic hash, plaintext = hashed via BLAKE2b-256 (same as sender's input).
- `[dest]` — destination file path, or `-` for stdout. If omitted: use the sender's filename in the current directory. If dest is an existing directory: write the sender's filename inside it. If dest is a path: write to that exact path. If `-`: stream received data directly to stdout (implies `--yes`, skips prompt).

**Options:**

- `--yes` — skip the confirmation prompt; accept the transfer immediately.
- `--force` — allow overwriting an existing file without prompting. (Without this, the receiver aborts if the destination already exists, unless the user confirms at the prompt.)
- `--timeout <seconds>` — how long to wait for a sender before giving up. Default: 60 seconds. If no connection is established within this window, exit 1 with "sender not found" error. This timeout also applies as an idle timeout during transfer: if no data is received for `--timeout` seconds after the connection is established, abort with error (prevents stalled/malicious senders from hanging the receiver indefinitely).

**Behavior:**

Uses `peeroxide::spawn()` (full swarm).

1. Resolve topic (same rules as sender: 64-char hex = raw, otherwise BLAKE2b-256).
2. Join the swarm on the topic in client mode (`JoinOpts { server: false, client: true }`), looking for peers announcing on the topic.
3. Wait for a connection to be established. The swarm handles lookup + holepunch + Noise handshake. If multiple peers are announcing on the same topic, accept the first successful connection. If no connection is established within `--timeout` seconds, exit 1 with error.
4. Receive metadata header (message 1): parse JSON. Validate: `version == 1`, `filename` is a non-empty string (no path separators — basename only), `size` is a non-negative integer or `null`.
5. **Determine destination path:**
   - If `[dest]` is `-`: stdout mode. Skip steps 6 (prompt) — write data directly to stdout. No temp file, no atomic rename.
   - If `[dest]` was provided and is a directory: `dest / filename`.
   - If `[dest]` was provided and is not a directory: use `dest` directly.
   - If `[dest]` was omitted: `./ filename` (current directory + sender's filename).
   - Sanitize filename (for non-stdout modes): strip any path separators, reject `.` and `..`.
6. **Confirmation prompt** (unless `--yes`): Print to stderr:
   ```
   Incoming file: archive.tar.gz (15.2 MB)
   Save to: ./archive.tar.gz
   Accept? [y/N]
   ```
   Wait for `y` or `Y` + Enter from tty. On `N`/Enter/EOF: disconnect, exit 1. On SIGINT: disconnect, exit 130.
   - If destination exists and `--force` is not set: add `(file exists — will overwrite)` to the prompt. On `N`: abort.
   - If `--yes` is set and destination exists but `--force` is not set: abort with error (refuse to silently overwrite without explicit `--force`).
   - If `--force` is set: overwrite without warning (regardless of `--yes`).
   - If `size` is `null`: display `(unknown size — streaming from stdin)`.
7. **Receive data:** Read messages from SecretStream and write to destination (temp file for disk, direct for stdout). Track bytes received.
   - If `size` is known: abort immediately if `bytes_received > size` at any point during transfer (delete temp file if applicable, disconnect, exit 1). After EOF: validate `bytes_received == size` exactly — mismatch means truncation, delete temp file and abort.
   - If `size` is null: accept any amount until EOF.
8. **EOF** (read returns `None`): For disk mode: rename temp file to destination atomically. Print summary to stderr.
9. Exit 0.

**On errors during transfer:**
- Unexpected disconnect (sender crashed/killed): delete temp file, print error, exit 1.
- Disk full / write error: disconnect, delete temp file, print error, exit 1.

**Output (stderr):**

```
$ peeroxide cp recv a1b2c3d4...64chars
CP RECV topic: a1b2c3d4...
  looking up sender...
  connected to @cd34... (holepunch)
  Incoming file: archive.tar.gz (15.2 MB)
  Save to: ./archive.tar.gz
  Accept? [y/N] y
  receiving... 15.2 MB / 15.2 MB [100%]
  done: 15.2 MB in 4.3s (3.5 MB/s)
  saved to ./archive.tar.gz

$ peeroxide cp recv a1b2c3d4...64chars - | tar x
CP RECV topic: a1b2c3d4...
  looking up sender...
  connected to @cd34... (holepunch)
  receiving... 15.2 MB / 15.2 MB [100%]
  done: 15.2 MB in 4.3s (3.5 MB/s)
```

**Exit codes:**

- `0` — file received and written successfully
- `1` — fatal error (sender not found, transfer aborted, size mismatch, disk error, user rejected)
- `130` — interrupted by SIGINT

#### Transfer protocol (over SecretStream)

The connection is already Noise-encrypted by peeroxide-dht. On top of the raw SecretStream, the file transfer uses a simple framing:

**Message 1 (metadata):** JSON object (UTF-8 encoded)
```json
{"filename": "data.tar.gz", "size": 1048576, "version": 1}
```

Fields:
- `filename` (string, required): basename only, no path separators.
- `size` (integer or null, required): file size in bytes (non-negative), or `null` for stdin/unknown-length streams. Zero is valid (empty file).
- `version` (integer, required): protocol version. Must be `1`. Receiver rejects unknown versions.

**Messages 2..N (data chunks):** Raw file bytes, up to 64KB per message.

**EOF:** Sender closes the connection. Receiver sees `SecretStream::read() -> Ok(None)`.

**Design rationale:**
- Uses the peeroxide swarm directly — sender announces on the topic, receiver looks up the topic, swarm establishes a Noise-encrypted direct connection. No custom authentication layer needed.
- 64KB chunk size — proven by croc and Magic Wormhole, fits well within SecretStream's 16MB max message size while keeping memory pressure low.
- Connection close = EOF — simpler than a sentinel message. Empty messages can't be used (treated as keepalives by SecretStream).
- No resumption in v1 — keep it simple. Can add chunk offsets later if needed.
- JSON metadata — self-describing, easy to extend (just add fields), human-debuggable. One-time cost (single message) so parsing overhead is negligible.
- `size: null` — enables stdin streaming without requiring the sender to buffer the entire input to compute length.

#### Security model

- **Topic = rendezvous.** The topic is used solely for discovery on the DHT. It is not a secret — DHT nodes see the topic during announce/lookup. Anyone who knows the topic can connect to the sender and request the file.
- **Noise = transport encryption.** The swarm establishes a Noise-encrypted direct connection between sender and receiver. All data in transit is encrypted and authenticated at the transport layer. An observer cannot read the file content without being a connection endpoint.
- **No sender authentication.** The receiver does not verify the sender's identity beyond "they announced on this topic." If multiple peers announce on the same topic, the receiver connects to whichever it discovers first. This is intentional — the topic is a coordination point, not a credential.
- **No receiver authentication.** The sender accepts any incoming connection. The first peer to connect receives the file. For private transfers, keep the topic secret (use randomly generated topics and share them out-of-band).
- **Access control = topic secrecy.** For sensitive transfers, the randomly generated topic functions as a one-time capability token. Share it only with the intended recipient over a secure channel. If the topic leaks, anyone can connect and download.
- **Forward secrecy.** The Noise handshake uses ephemeral keys mixed with the static swarm keypairs. Since swarm keypairs are random (not derived from the topic), transport encryption has standard Noise forward secrecy properties.

#### Practical limits

- **No file size limit:** Limited only by disk space and transfer time. The protocol streams — neither side needs to hold the full file in memory.
- **Single file only (v1):** No directory support. Use `tar` externally: `tar c dir/ | peeroxide cp send - --name dir.tar` on the sender, `peeroxide cp recv <topic> - | tar x` on the receiver.
- **No resumption:** If transfer is interrupted, start over. v2 could add byte-range requests.
- **Sequential transfers with `--keep-alive`:** One receiver at a time. Second connection is rejected until the current transfer completes. Note: the swarm's internal connection tracking does not clean up closed connections, so `--keep-alive` is best suited for a bounded number of transfers. For unbounded use, restart the process periodically.

---

### `peeroxide deaddrop`

Anonymous store-and-forward via the DHT. No direct connection between sender and receiver — data is stored as linked mutable records on DHT nodes. Neither party learns the other's network address.

**Subcommands:**

```
peeroxide deaddrop leave <file|->       # store data on DHT, print pickup key
peeroxide deaddrop pickup <key>         # retrieve data from DHT using pickup key
```

#### `peeroxide deaddrop leave`

**Arguments:**

- `<file|->` — file path to store, or `-` for stdin. Maximum size is bounded by the chunk format: 65535 chunks → ~60 MB (see storage protocol). Empty files (0 bytes) are valid (produces one chunk with empty payload).

**Options:**

- `--max-speed <bytes/s>` — hard cap on outbound byte rate. Accepts human-friendly suffixes: `100k`, `1m`, `500k`. Default: unlimited. Enforced via two mechanisms: (1) a maximum put concurrency of `floor(max_speed / ~22KB_per_put)` (minimum 1), and (2) a minimum inter-put delay of `~22KB / max_speed` between dispatching successive puts. Together these approximate a byte-rate ceiling.
- `--refresh-interval <seconds>` — target time between refresh cycle starts (default: 600s / 10 minutes). The adaptive scheduler attempts to spread each refresh cycle evenly across this interval.
- `--ttl <seconds>` — stop refreshing after this duration and let the drop expire (default: indefinite). Must be > 0.
- `--max-pickups <n>` — exit after N pickup acknowledgements detected (default: exit on SIGINT only). Must be > 0.
- `--passphrase` — derive the root keypair from an interactive passphrase (read from tty, not stdin) instead of random generation. Both sides must know the passphrase to use this mode. **Security warning:** weak passphrases are vulnerable to dictionary attack — anyone who guesses the passphrase can derive the pickup key and read the data. Use a strong random secret (e.g., 128+ bits of entropy). Reusing the same passphrase for different content within the ~20-minute DHT TTL window may produce mixed-version pickups.

**Behavior:**

Uses `HyperDhtHandle` directly (no swarm needed).

1. Read entire file/stdin into memory. Reject if it would exceed 65535 chunks (~60 MB).
2. Generate a root `KeyPair` (random via `KeyPair::generate()`, or from `--passphrase` via `BLAKE2b-256(passphrase_bytes) → 32-byte seed → KeyPair::from_seed(seed)`). Passphrase bytes = UTF-8 encoding of the tty input with trailing newline stripped. No Unicode normalization (byte-exact match required between leave and pickup).
3. Compute CRC-32C of the entire file payload.
4. Split data into chunks: first chunk payload ≤ 961 bytes, subsequent chunks ≤ 967 bytes each (see binary chunk format below).
5. Generate a derived `KeyPair` for each non-root chunk: `KeyPair::from_seed(BLAKE2b-256(root_seed || chunk_index_as_u16_le))`
   - `root_seed` = the 32-byte seed used to create the root keypair (the BLAKE2b hash for passphrase mode, or the random 32 bytes for random mode). NOT the 64-byte expanded secret key.
6. Serialize each chunk in the binary format (root chunk includes total count N and CRC-32C; non-root chunks include only version + next pointer + payload).
7. Write chunks via `mutable_put` in **head-to-tail dispatch order** (root first, then each subsequent chunk following the linked list). Puts are dispatched concurrently (up to the adaptive concurrency limit, starting at 4) — a put for chunk N+1 is not dispatched until chunk N has been admitted to the semaphore. Use `seq = current Unix timestamp` for each put (monotonic per record key across refresh cycles; concurrent puts to different keys may share the same second-resolution timestamp — this is fine since seq is only compared within a single key's history). This order is used for both initial publish and refresh.
    - Track per-node success/failure from each `mutable_put` to feed the AIMD congestion controller (see refresh rate control below).
    - If any `mutable_put` returns a hard error (not a per-node timeout, but a complete failure like bootstrap unreachable): abort immediately, print error to stderr, exit 1. Do NOT print the pickup key.
    - Note: Per-node timeouts during a put are expected under congestion — the adaptive controller backs off. Only unrecoverable errors abort.
8. Print the root public key (64-char hex) to stdout — this is the pickup key. Print **only** this line to stdout (no decoration). All other output goes to stderr.
9. Stay alive, refreshing chunks to keep them within the 20-minute DHT record TTL. Refresh follows head-to-tail dispatch order. Each put uses `seq = current Unix timestamp` (increases monotonically per key across cycles). Refresh failures (degraded puts) feed the AIMD controller but do not abort.

   **Refresh rate control:** Three mechanisms work together:

   - **`--max-speed` (hard cap):** Approximate ceiling on outbound byte rate. Enforced via two levers: (1) max concurrency = `floor(max_speed / ~22KB)` (minimum 1), and (2) minimum inter-put delay = `~22KB / max_speed` between successive put dispatches. The delay ensures we don't burst beyond the target rate even when puts complete faster than expected.
   - **Adaptive concurrency (AIMD):** The client runs multiple `mutable_put` calls concurrently (dispatch gated by a `tokio::Semaphore`). It detects local link saturation via put-level outcomes and adjusts concurrency using AIMD:
     - Starting concurrency: **4** (a reasonable default; ramps from there).
     - A put is **"degraded"** if ANY of its per-node requests timed out (indicates outbound packet loss from local link saturation).
     - Every 10 put completions (one "window"), compute `degraded_ratio = degraded_puts / total_puts`.
     - If `degraded_ratio > 30%`: **backoff** — halve concurrency (multiplicative decrease, min 1).
     - If `degraded_ratio == 0%`: **ramp up** — increase concurrency by 1 (additive increase).
     - Otherwise: **hold steady**.
     - Never exceeds max concurrency derived from `--max-speed` (if set).
   - **`--refresh-interval` (scheduling target):** The client attempts to complete each full refresh cycle within this interval (default 10 minutes). The adaptive concurrency ramps up toward this objective. If the link cannot sustain the required throughput even at maximum concurrency, the cycle simply takes longer — the oldest chunks (head of chain) may temporarily expire before the next cycle refreshes them. The reader retries with backoff and will find them on the next refresh pass.

   **Why this works:** The sender's local uplink is always the bottleneck — the distributed DHT nodes can absorb far more inbound traffic than any single client can produce. Per-node timeouts therefore signal local link saturation (dropped outbound packets), not remote overload. A single timeout within a put is sufficient signal to count that put as degraded.

   **Dispatch ordering:** Puts are dispatched in head-to-tail order (root first). The semaphore controls how many are in-flight concurrently, but dispatch is strictly sequential in chain order — a put for chunk N+1 is not dispatched until chunk N's put has been admitted (acquired the semaphore permit). Completion order is NOT guaranteed (later chunks may finish before earlier ones). This preserves the property that head chunks get refreshed earliest in each cycle.

   **Cycle behavior:** Cycles do not overlap. The next refresh cycle starts immediately after the previous one completes (all puts finished). If cycle duration < `--refresh-interval`, the client sleeps for the remainder before starting the next cycle.

   **Safety net:** The DHT IO layer maintains its own congestion window (max 80 in-flight packets, 4-slot rotating window drained every 750ms). If application-layer concurrency produces more outbound requests than the window allows, excess requests queue internally and drain at the IO layer's pace. This prevents UDP socket saturation regardless of application-layer decisions.

   **Example:** 1000 chunks, `--max-speed 100k`. Max concurrency = `floor(100000/22000)` = 4. Inter-put delay = `22000/100000` = 220ms. If average put latency is 300ms: with concurrency=4, throughput ≈ 4 puts completing every ~300ms ≈ 13 puts/s, but inter-put delay gates dispatch to at most 1/220ms ≈ 4.5 puts/s. The delay dominates: effective rate ≈ 4.5 × 22KB ≈ 99 KB/s (within the cap). Cycle time ≈ 1000/4.5 ≈ 222s (well within 10-min interval).
10. Periodically (every 30 seconds) call `lookup(ack_topic)` where `ack_topic = BLAKE2b-256(root_public_key_bytes || b"ack")` (32-byte raw public key concatenated with the literal ASCII bytes "ack"). Deduplicate seen ack pubkeys across the entire process lifetime. Each new unique pubkey = one pickup detected.
11. On pickup detected: log to stderr. If `--max-pickups` count reached, stop refreshing and exit 0.
12. On SIGINT/SIGTERM or `--ttl` expiry: stop refreshing and exit 0. Records expire from the DHT within ~20 minutes.

**Output (all to stderr except the pickup key):**

```
$ peeroxide deaddrop leave secret.txt
# stdout (single line, the pickup key):
cd34ef5678901234567890123456789012345678901234567890123456789012
# stderr:
DEADDROP LEAVE 5 chunks (4.7 KB)
  published to DHT (best-effort)
  pickup key printed to stdout
  refreshing every 10m, monitoring for acks...
  [ack] pickup #1 detected
  ^C
  stopped refreshing; records expire in ~20m
```

**Exit codes:**

- `0` — published successfully, exited cleanly (SIGINT/SIGTERM, `--ttl` elapsed, or `--max-pickups` reached)
- `1` — fatal error (bootstrap failed, file too large, file read error, initial publish failed)
- `130` — interrupted by SIGINT before initial publish completed

#### `peeroxide deaddrop pickup`

**Arguments:**

- `<key>` — the pickup key. If 64-char hex: used directly as root public key. Otherwise: treated as passphrase → `BLAKE2b-256(UTF-8 bytes of argument) → seed → KeyPair::from_seed(seed) → extract public key`. (Note: a passphrase that happens to be exactly 64 hex characters will be misinterpreted as a key. This is a known limitation — such passphrases are unsupported. The passphrase bytes are the raw UTF-8 encoding of the argv string — no newline stripping needed since argv doesn't include one.)

**Options:**

- `--output <path>` — write output to file (atomic: write to temp file, rename on success). Default: stdout.
- `--timeout <seconds>` — give up on any single chunk after this duration of retrying (default: 1200s / 20 minutes, matching the DHT record TTL). If the sender's refresh cycle is known to be slow, increase this. Must be > 0.
- `--no-ack` — don't announce pickup acknowledgement (maximum receiver anonymity — no network activity beyond the fetch itself)

**Behavior:**

Uses `HyperDhtHandle` directly (no swarm needed for fetch; swarm-less announce for ack).

1. Determine root public key from `<key>` argument (see grammar above).
2. Fetch root chunk: `mutable_get(root_public_key, 0)`. If not found, **retry with exponential backoff** (initial 1s, max 30s between attempts). This handles the case where the reader arrives before or during the sender's initial publish.
3. **Validate root chunk:** version == 0x01, N > 0, N ≤ 65535 (format maximum). Abort on any validation failure.
4. Walk the linked list: fetch each `next` public key via `mutable_get` sequentially. If a chunk is not found, **retry with backoff** (same policy: initial 1s, max 30s). This allows the reader to follow behind a sender that is still writing or mid-refresh. **Loop detection:** maintain a `HashSet` of all fetched public keys; if any `next` pointer refers to an already-seen key, abort immediately.
5. **Chain termination:** The walk ends when a chunk has `next == [0u8; 32]` (all zeros). This must occur at exactly the Nth chunk (counting from root as chunk 0). If the chain ends early or doesn't end after N chunks, abort.
6. **Give-up timeout:** If any single chunk is not found after `--timeout` duration of retrying (default 20 minutes), abort with error (the drop is likely expired or was never fully published). The default matches the DHT record TTL — if a chunk hasn't appeared within one full TTL window, the sender has likely stopped.
7. Reassemble: concatenate payload bytes from chunks 0..N-1 in traversal order.
8. **CRC verification:** Compute CRC-32C of the assembled payload. Compare against the root chunk's stored CRC. Mismatch = abort (mixed-version or corrupted data). Do NOT write output or send ack.
9. Write assembled data:
   - If `--output <path>`: write to a temp file in the same directory, then `rename()` atomically. This prevents partial files on failure/interrupt.
   - If stdout (default): write all bytes at once after full reassembly. All status output goes to stderr to avoid corrupting the data stream.
10. Unless `--no-ack`: announce on the ack topic (`BLAKE2b-256(root_public_key_bytes || b"ack")`) using a fresh ephemeral keypair with empty relay addresses. Do NOT unannounce — the record persists on the DHT (~20 min TTL) for the sender to discover during their next lookup cycle. Exit immediately after the announce call completes.

**Output contract:**
- **stdout:** Raw file data only (when not using `--output`). No status text, no decoration. Binary-safe.
- **stderr:** All human-readable status (progress, errors, warnings).

**Output (stderr):**

```
$ peeroxide deaddrop pickup cd34ef56...64chars > recovered.txt
DEADDROP PICKUP @cd34...
  fetching chunk 1/5...
  fetching chunk 2/5...
  fetching chunk 3/5...
  fetching chunk 4/5...
  fetching chunk 5/5...
  reassembled 4.7 KB
  ack sent (ephemeral identity)
  done

$ peeroxide deaddrop pickup cd34ef56...64chars --output recovered.txt --no-ack
DEADDROP PICKUP @cd34...
  fetching chunk 1/5...
  ...
  reassembled 4.7 KB
  written to recovered.txt
  done (no ack sent)
```

**Exit codes:**

- `0` — data retrieved and written successfully
- `1` — fatal error (bootstrap failed, root chunk not found, chain broken/malformed, write error)
- `130` — interrupted by SIGINT before completion (no output written, no ack sent)

#### Storage protocol (binary chunk format)

Each chunk is stored via `mutable_put` at `hash(chunk_public_key)`. Two frame types:

**Root chunk (chunk 0):**

```
Offset  Size   Field
0       1      Version (0x01)
1       2      Total chunk count N (u16 LE, includes root itself)
3       4      CRC-32C of the fully assembled payload (Castagnoli)
7       32     Next chunk public key (32 zero bytes if N=1, i.e., single-chunk drop)
39      ...    Payload (raw file bytes, up to 961 bytes)
```

Root header overhead: **39 bytes** → max **961 bytes** payload.

**Non-root chunk (chunks 1..N-1):**

```
Offset  Size   Field
0       1      Version (0x01)
1       32     Next chunk public key (32 zero bytes if final chunk)
33      ...    Payload (raw file bytes, up to 967 bytes)
```

Non-root header overhead: **33 bytes** → max **967 bytes** payload.

**Design rationale:**

- **No chunk index:** Ordering is implicit from the linked-list traversal. The `next` pointer defines the sequence — no redundant counter needed.
- **Total count in root only:** The receiver learns `N` from the root chunk and uses it for progress display and validation. Non-root chunks don't repeat it — the receiver already knows.
- **CRC-32C in root:** Covers the entire reassembled payload (all chunks concatenated). Computed by the sender over the original file bytes before chunking. The receiver reassembles all chunks, computes CRC-32C over the result, and compares against the root's stored value. This detects mixed-version corruption (e.g., chunks from different `leave` sessions with the same passphrase that got partially overwritten).
- **`next` as frame header:** Positioned before payload data so the receiver can extract the next fetch target without parsing/buffering the payload.
- **Version byte on all chunks:** Enables independent validation without context. Future format revisions can coexist.
- **All-zeros `next`:** Signals end-of-chain. The receiver knows it's the final chunk when `next == [0u8; 32]`.

**Validation rules (receiver MUST enforce):**

1. Root chunk: version == 0x01, N > 0 (N ≤ 65535 by format — u16 LE).
2. Non-root chunks: version == 0x01.
3. **Loop detection:** Track all fetched public keys in a `HashSet`. If a `next` pointer refers to an already-seen key, abort immediately — the chain is malicious or corrupted.
4. **Chain length:** If more than N chunks are traversed without reaching an all-zeros `next`, abort.
5. **Final chunk agreement:** The Nth chunk (counting from 0) MUST have `next == [0u8; 32]`. If it doesn't, abort.
6. **CRC-32C:** After reassembly, compute CRC-32C of the concatenated payload. Compare against root's stored CRC. Mismatch = abort (do not write output, do not ack).

**Chunk keypair derivation:** `KeyPair::from_seed(BLAKE2b-256(root_seed || chunk_index_as_u16_le))`

- `root_seed`: 32 bytes. For random mode, the seed passed to `KeyPair::from_seed()`. For passphrase mode, `BLAKE2b-256(passphrase_bytes)`.
- `chunk_index_as_u16_le`: the 2-byte little-endian encoding of the chunk's position (0 for root, 1 for first non-root, etc.).
- Chunk 0 uses the root keypair directly (not derived).

The receiver only needs the root public key. From there, each chunk's `next` field provides the public key needed to fetch the following chunk. The receiver **cannot** derive chunk public keys without the root seed — the linked-list walk is the only path.

**Why binary, not JSON+base64?** Base64 in JSON adds ~48% overhead (675 usable bytes vs 961/967 with binary). Binary framing is simple (fixed small headers), trivially parseable, and maximizes per-chunk payload.

**Capacity:**
- Single chunk (N=1): 961 bytes
- For larger files: 961 + (N-1) × 967 bytes total payload
- Format maximum: N=65535 → ~60 MB (bounded by u16 LE chunk count field)

#### Pickup acknowledgement protocol

- **Ack topic:** `BLAKE2b-256(root_public_key_bytes || b"ack")` — deterministic from the pickup key (32-byte raw public key + ASCII "ack"), known to both sides.
- **Receiver acks:** Calls `announce(ack_topic, ephemeral_keypair, relay_addresses=[])` once and exits immediately. The announcement record persists on the DHT for ~20 minutes (standard TTL), giving the sender ample time to discover it. No explicit unannounce needed — TTL expiry handles cleanup.
- **Sender monitors:** Polls `lookup(ack_topic)` every 30 seconds. Each unique pubkey seen = one pickup. Deduplicates across the process lifetime to avoid double-counting the same ack (which persists for ~20 min).
- **Opt-out:** `--no-ack` skips the announce entirely for maximum receiver anonymity (no network activity after fetch completes).
- **Advisory only:** Acks are NOT proof of pickup. Anyone who knows the pickup key can announce on the ack topic and fake a pickup. The sender cannot distinguish real pickups from spoofed ones. `--max-pickups` should be treated as a convenience heuristic, not a security guarantee.

#### Privacy model

| Property | Guarantee |
|---|---|
| Sender → Receiver identity | Hidden (ack uses ephemeral key with no relay addresses; sender sees only count) |
| Receiver → Sender identity | Hidden (no direct connection; receiver only fetches from DHT nodes) |
| Sender IP ↔ Receiver IP | Never directly connected; mediated by separate DHT nodes at different coordinates |
| Data at rest on DHT | **Signed but NOT encrypted.** Storage nodes can read the raw payload bytes. For confidentiality, pre-encrypt the file before leaving (e.g., `age -e file \| peeroxide deaddrop leave -`). |
| Data expiry | Auto-expires ~20 min after sender stops refreshing |
| Pickup key as capability | Knowing the root public key = ability to read AND ability to spoof acks. Treat as secret for private drops. |
| DHT node visibility | DHT nodes routing/storing records can observe source IPs of write/read requests. Timing correlation between leave and pickup is theoretically possible for a sufficiently positioned adversary. |

#### Practical limits

- **961 bytes payload in root chunk** (1000-byte cap minus 39-byte root header)
- **967 bytes payload in non-root chunks** (1000-byte cap minus 33-byte header)
- **Format maximum:** 65535 chunks (u16 LE) → ~60 MB theoretical ceiling
- **Refresh is concurrent:** Chunk puts are dispatched in head-to-tail order with adaptive concurrency (starting at 4, AIMD-controlled). Bottleneck is outbound bandwidth: each put commits ~1.1 KB to ~20 DHT nodes ≈ 22 KB outbound per chunk.
- **Fetch latency (v1 bottleneck):** N sequential DHT queries. Each ~1-3 seconds on a healthy network. 100 chunks ≈ 2-5 min; 1000 chunks ≈ 20-50 min.
- **Practical ceiling:** Limited by fetch latency (sequential reads on pickup), not refresh cost. For files where sequential fetch is unacceptable, use `cp` (live connection, no size limit) or the v2 protocol (parallel data fetch).

**Refresh bandwidth reference:**

| Metric | Value |
|--------|-------|
| Format max chunks | 65,535 (~60 MB) |
| Outbound per chunk (commit to ~20 nodes) | ~22 KB |
| Total refresh outbound at format max | ~1.4 GB |
| Starting concurrency | 4 puts in parallel |
| Approx throughput at concurrency=4, 200ms RTT | ~440 KB/s |
| Time to refresh 1000 chunks at that rate | ~50s |
| Time to refresh format max at that rate | ~8 min |

If the adaptive controller cannot ramp high enough (link saturation, high timeouts), the refresh cycle extends beyond `--refresh-interval`. Head chunks (written first, oldest TTL) may temporarily expire — reader retry handles this gracefully. The format maximum is usable at any speed; slower links just mean the reader waits longer for head chunks to reappear on the next refresh pass.

#### Design decisions

- **No encryption layer in v1:** DHT records are signed (integrity) but not encrypted. Anyone with the pickup key can read the data. For confidentiality, encrypt before dropping. Adding built-in encryption is a v2 consideration.
- **Sequential fetch, not parallel:** The receiver can't know chunk N+1's public key without fetching chunk N. This is inherent to the linked-list design. A future "manifest mode" could trade the size ceiling for parallelism.
- **Binary framing:** Maximizes per-chunk payload (961/967 vs ~675 bytes with JSON+base64). Two frame types (root vs non-root) keep headers minimal — no redundant metadata in non-root chunks.
- **CRC-32C in root, not per-chunk:** Per-chunk CRC is unnecessary because `mutable_get` already verifies Ed25519 signatures on each record. The CRC-32C in the root validates the *assembled* result — catching mixed-version chunks from passphrase reuse or partial overwrites.
- **No chunk index:** The linked-list `next` pointer defines ordering. An index field would be redundant and waste a byte per chunk. The receiver simply concatenates payloads in traversal order.
- **Passphrase mode:** Both sender and receiver derive the same root keypair from the passphrase. The receiver can derive the root public key without the sender needing to transmit it out-of-band. Useful for pre-arranged drops where the pickup key can't be transmitted. **Reuse caution:** same passphrase = same keypair = same DHT coordinates. Two concurrent `leave` calls with the same passphrase will overwrite each other's chunks.
- **Write order (always head→tail):** Both initial publish and refresh dispatch root first, then follow the chain. The reader handles not-yet-available chunks via retry with exponential backoff (max 30s). Head-first dispatch means root is the oldest chunk in each cycle — making it the first to expire if the cycle overruns the TTL. This is acceptable: the root is also the first chunk refreshed in the *next* cycle, and the reader's retry-with-backoff handles temporary root unavailability. The alternative (tail-first refresh) would keep the root fresh but sacrifice the property that the reader's sequential walk always proceeds in the same direction as the refresh wave.
- **No artificial size cap:** The format's natural limit is 65535 chunks (~60 MB), defined by the u16 LE chunk count field. The practical constraint for large files is sequential fetch latency (v1's linked-list requires one round-trip per chunk). For files where this becomes unacceptable, use `cp` or the v2 protocol.
- **Best-effort storage with congestion feedback:** `mutable_put` dispatches write requests to the ~20 closest nodes and awaits per-node responses. Any per-node timeout within a put marks that put as "degraded" — signaling local link saturation. The AIMD controller uses the degraded-put ratio to adaptively adjust concurrency. On a healthy network most nodes respond quickly; on a saturated uplink, timeouts trigger automatic backoff.

---

## Crate structure

```
peeroxide-cli/
  Cargo.toml
  src/
    main.rs          — clap entrypoint, subcommand dispatch
    config.rs        — global config loading (TOML), XDG paths, precedence logic
    cmd/
      mod.rs
      node.rs        — `peeroxide node` implementation
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
toml = "0.8"
dirs = "6"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
hex = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
indicatif = "0.17"
```

## Future library enhancements (no changes planned now)

These are library-level additions that would improve CLI diagnostics but are **not required** for the initial implementation. Noted here for future consideration.

- **Connection type reporting:** `PeerConnection` currently provides no public indicator of how the connection was established. The internal `_relay_task` field distinguishes relay from non-relay, but holepunch vs direct is indistinguishable — that information is lost after `establish_stream()` returns. A `pub fn connection_type(&self) -> ConnectionType` (with variants `Direct`, `Holepunch`, `Relay`) would let `ping --connect` report the path used.
- **UDX stream-level stats:** `StreamInner` tracks retransmission counts, RTO timeouts, and SACK state internally (`pub(crate)`), but none of this is accessible from outside the crate. Exposing a `UdxStream::stats()` (or similar) returning packet loss, retransmissions, and congestion events would give `ping --connect` the same transport-level visibility that UDP mode gets from `DhtHandle::stats()`. Without this, echo mode can only report application-level timeouts — we cannot distinguish "2 probes timed out due to packet loss + retransmission delay" from "remote stopped responding." This is a significant diagnostics gap.
- **UDX stream RTT access:** The UDX layer maintains a Jacobson/Karels RTT estimator (`srtt`, `rttvar`, `rto`) and BBR min-RTT internally, but these are `pub(crate)` with no public getter. Exposing read-only access would enable sub-millisecond latency reporting without app-level echo pings.
- **Disk persistence:** `RoutingTable` and `Persistent` storage are purely in-memory with no serialization. Adding export/import to `DhtHandle` would enable long-lived nodes to survive restarts.

## Open questions

- Should the binary name conflict with the `peeroxide` library crate? (Cargo allows it but may confuse `cargo install` users.)
- `announce`: should re-announce interval be configurable or hardcoded?

## Documentation notes (for user-facing docs)

**Command relationships that need explicit explanation:**

- **announce + ping:** A peer running `announce <topic>` becomes discoverable and pingable by topic name. Another peer can `ping <topic>` to test reachability without knowing the announcer's IP. This is the primary way to test peer-to-peer connectivity — not obvious from command names alone.
- **node as ping target:** `peeroxide node` is a long-lived process that responds to `ping host:port`. Useful for testing raw UDP reachability to a known address.
- **ping target types map to different scenarios:**
  - `ping host:port` → "can I reach this specific address?" (requires knowing the address)
  - `ping <pubkey>` → "can I reach this specific peer?" (requires knowing their public key)
  - `ping <topic>` → "can I reach anyone on this topic?" (requires only the shared topic name)

**Usage patterns to document:**
- Monitoring: `peeroxide ping <topic> --count 0` for continuous reachability monitoring of a service
- Diagnostics: `peeroxide ping <bootstrap-node> --count 5` to verify network connectivity
- End-to-end test: `peeroxide ping <topic> --connect` to verify full encrypted connection path
