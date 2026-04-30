# Ralph Loop: peeroxide-cli Technical Documentation (mdBook)

## Mission

Scaffold and author a professional-quality mdBook documenting the architecture,
wire formats, blob data layouts, and behaviors of five peeroxide-cli tools:
**lookup**, **announce**, **ping**, **cp**, and **deaddrop**. The book lives
under `docs/` at the workspace root and will be published to GitHub Pages.

Two audiences:
1. **End users** — want to understand how the tools work at a conceptual level
   without reading source code.
2. **App developers** — writing their own Rust apps using the peeroxide crates,
   who want to understand how the CLI tools use the library to accomplish
   specific tasks (reference implementation patterns).

---

## Hard Constraints

- **NO source code changes.** Documentation only.
- If you find something in the code that is incorrect, surprising, or worth
  investigating, append it to `ISSUES.md` (create if it does not exist) with a
  short description and file/line reference. Do not fix it.
- **Git commit after every completed work item** (when a checkbox moves to
  `[X]`). Also commit after every intermediate checkbox state change. Do not
  push at any point.
- Use subtasks (subagents) wherever research or writing benefit from
  parallelism.
- After each major section is written, perform a broader review pass: verify
  accuracy against the source files listed in each section, reorganize for
  flow, and polish prose and diagrams to professional quality before marking
  `[X]`.
- `peeroxide-cli/PLANNING.md` will be deleted by the developer at some point.
  Extract all useful information from it into the docs. Do not modify or delete
  it yourself.
- `peeroxide-cli/DEADDROP_V2.md` is a forward-looking design document. Do not
  modify it. Use its content only for the "Future Direction" section of the
  deaddrop chapter.

---

## Checkbox Convention

```
[ ]  not started
[/]  drafted / ready to review
[X]  written, reviewed against source, and complete
```

YOU MUST GIT COMMIT AFTER YOU CHANGE ANY CHECKBOX STATE.

---

## Phase 0 — DOCS_PLAN.md

Before touching `docs/`, create `DOCS_PLAN.md` at the workspace root. Populate
it with the full checkbox task list from this prompt before any other work
begins. Commit it. This file is the authoritative progress tracker for the
entire loop.

---

## Phase 1 — mdBook Scaffold

Create the following structure. Commit once the scaffold builds cleanly.

```
docs/
  book.toml
  src/
    SUMMARY.md          <- full chapter outline (stub content acceptable)
    introduction.md
    concepts/
      dht-primitives.md
      encrypted-streams.md
      peer-identity.md
    lookup/
      overview.md
      output-formats.md
    announce/
      overview.md
      architecture.md
      echo-protocol.md
    ping/
      overview.md
      architecture.md
      output-formats.md
    cp/
      overview.md
      protocol.md
      reliability.md
    deaddrop/
      overview.md
      architecture.md
      format.md
      operations.md
      future-direction.md
    appendices/
      security-model.md
      limits-and-performance.md
```

Chapter ordering rationale: lookup (pure DHT read, no connections) → announce
(DHT write + optional echo server) → ping (uses DHT + connects to echo servers)
→ cp (full swarm connection) → deaddrop (most complex, deep mutable-record usage).
Each chapter builds on concepts introduced by earlier chapters. The echo
protocol (PING/PONG handshake + 16-byte probe frames) is defined once in the
announce chapter and cross-referenced by the ping chapter. Do not re-explain
it in both places.

`book.toml` requirements:
- title: "peeroxide-cli: Technical Reference"
- description: "Architecture, wire formats, and behavioral reference for the
  peeroxide command-line tools."
- language: en
- Include `[preprocessor.mermaid]` for mdbook-mermaid.
- Output path: `docs/book` (so the built site is at `docs/book/`).

`SUMMARY.md` must include all chapters above as a fully linked outline. Stub
pages are fine at this stage.

Verify `mdbook build docs/` exits 0 before moving to Phase 2.

---

## Phase 2 — GitHub Actions Workflow

Create `.github/workflows/docs-site.yml`. Requirements:

- Filename must not conflict with existing `ci.yml` or `release.yml`.
- Trigger: `push` to `main` only.
- Job name: `mdbook-deploy` (distinct from the existing `docs` job in ci.yml).
- Install both `mdbook` and `mdbook-mermaid` via `cargo install` with pinned
  versions. Check the latest stable releases before writing the versions in.
- Run `mdbook-mermaid install docs/` before build.
- Run `mdbook build docs/`.
- Deploy built output (`docs/book/`) to GitHub Pages using:
  - `actions/configure-pages@v5`
  - `actions/upload-pages-artifact@v3`
  - `actions/deploy-pages@v4`
- Required permissions: `contents: read`, `pages: write`, `id-token: write`.
- Concurrency guard: `group: "pages"`, `cancel-in-progress: false`.

Add a comment block at the top of the workflow file noting that GitHub Pages
must be configured in repo Settings -> Pages -> Source: GitHub Actions before
this workflow will deploy successfully.

---

## Phase 3 — Shared Concepts

These chapters provide background used by all three tool chapters. Scope is
limited to behaviors within the peeroxide workspace. Do not explain how
cryptographic primitives work internally — point readers to the relevant
external library or crate documentation instead.

### 3a. `concepts/dht-primitives.md`

Cover:
- What the DHT is and what operations the peeroxide-dht crate exposes to apps
  (announce, lookup, find_peer, mutable_put/get, immutable_put/get).
- How topics work: `discovery_key` (BLAKE2b-256) maps a human string to a
  32-byte DHT key. The same function is used in Node.js Hyperswarm for
  interoperability. Topic grammar: 64-char hex string = raw key; any other
  string = `discovery_key(input.as_bytes())`.
- Mutable vs immutable records: key structure (public key), `seq` (monotonic
  counter, typically Unix timestamp), Ed25519 signature (link to ed25519-dalek
  for cryptographic details). Explain operational meaning: who can update,
  how clients find the latest.
- DHT record TTL: approximately 20 minutes. Records not refreshed are evicted.
- `DEFAULT_BOOTSTRAP`: public bootstrap addresses bundled in the peeroxide crate.

Source files:
- `peeroxide-dht/src/rpc.rs` — DhtHandle public API
- `peeroxide-dht/src/persistent.rs` — storage limits, eviction
- `peeroxide/src/lib.rs` — re-exports, discovery_key
- `peeroxide-cli/src/cmd/mod.rs` — build_dht_config, parse_topic

### 3b. `concepts/encrypted-streams.md`

Cover:
- What a `SwarmConnection` is: an established, encrypted P2P connection with
  message-level read/write semantics. `stream.read()` returns one logical
  message; `stream.write(bytes)` sends one. This boundary preservation is
  relied on by the cp and ping protocols.
- The Noise XX handshake — overview only. Refer to the noise-handshake crate
  for cryptographic details.
- SecretStream: ChaCha20-Poly1305 AEAD, length-prefixed framing. From the
  application perspective: writes and reads are message-delimited. Refer to
  the `@hyperswarm/secret-stream` spec for wire details.
- How connection establishment works from a caller's perspective: join a topic
  as server and/or client via `SwarmHandle::join()`, receive `SwarmConnection`
  objects on the channel.
- The `--connect` path in ping: using `HyperDhtHandle::connect` directly.

Source files:
- `peeroxide-dht/src/secret_stream.rs`
- `peeroxide/src/swarm.rs`
- `peeroxide-cli/src/cmd/ping.rs` — run_connect

### 3c. `concepts/peer-identity.md`

Cover:
- Ed25519 key pair: `KeyPair::generate()` for ephemeral; deterministic with
  `KeyPair::from_seed(seed)`.
- Public key as stable network identity.
- `discovery_key(public_key)` is used as the DHT lookup key for finding a peer
  by identity (self-announce). This is how `peeroxide ping @<pubkey>` works.
- The `--seed` option in `announce`: deterministic identity from a passphrase
  string; derivation is `KeyPair::from_seed(discovery_key(passphrase.as_bytes()))`.
- Passphrase-based derivation is a single BLAKE2b-256 invocation — not a
  memory-hard KDF. Deliberate tradeoff for DHT key consistency; not a password
  storage mechanism.

Source files:
- `peeroxide-dht/src/hyperdht.rs`
- `peeroxide-cli/src/cmd/announce.rs`
- `peeroxide-cli/src/cmd/deaddrop.rs`
- `peeroxide/src/peer_discovery.rs`

After writing all three concept pages, perform a cross-chapter review for
consistency and accuracy before marking complete.

---

## Phase 4 — lookup Chapter

Source files: `peeroxide-cli/src/cmd/lookup.rs`, `peeroxide-cli/src/cmd/mod.rs`.

### 4a. `lookup/overview.md`  (end-user focus)

Cover:
- What lookup does: discovers all peers announcing on a given DHT topic and
  optionally fetches the mutable metadata each peer has stored.
- Topic grammar: 64-char hex string = raw 32-byte topic key; any other string
  is hashed with BLAKE2b-256 via `discovery_key`. Same rule applies across all
  five CLI tools — explain here, cross-reference from other chapters.
- `--with-data`: why you'd want it (inspect metadata stored by `announce --data`).
- `--json`: machine-readable NDJSON on stdout; all human output goes to stderr.
- Worked example: lookup a plaintext topic, show human output. Second example
  with `--json --with-data`.

### 4b. `lookup/output-formats.md`  (app developer focus)

Document every NDJSON object type emitted to stdout when `--json` is used,
with exact field names, types, and semantics. Also document the human-readable
output lines emitted to stderr.

**NDJSON peer record (without `--with-data`):**
```json
{
  "type": "peer",
  "public_key": "<64-hex>",
  "relay_addresses": ["<host:port>", ...]
}
```

**NDJSON peer record (with `--with-data`, status ok):**
```json
{
  "type": "peer",
  "public_key": "<64-hex>",
  "relay_addresses": ["<host:port>", ...],
  "data_status": "ok",
  "data": "<string>",
  "data_encoding": "utf8" | "hex",
  "seq": <u64>
}
```
- `data_encoding`: `"utf8"` if value is valid UTF-8; `"hex"` (`"0x..."` prefix)
  otherwise.

**NDJSON peer record (with `--with-data`, data absent):**
```json
{
  "type": "peer",
  "public_key": "<64-hex>",
  "relay_addresses": ["<host:port>", ...],
  "data_status": "none",
  "data": null,
  "seq": null
}
```

**NDJSON peer record (with `--with-data`, fetch error):**
```json
{
  "type": "peer",
  "public_key": "<64-hex>",
  "relay_addresses": ["<host:port>", ...],
  "data_status": "error",
  "data": null,
  "seq": null,
  "error": "<string>"
}
```

**NDJSON summary record (always last):**
```json
{ "type": "summary", "topic": "<64-hex>", "peers_found": <integer> }
```

**Human output format** (stderr):
```
LOOKUP blake2b("<topic_input>")      # or: LOOKUP <topic_hex> for raw-hex input
  found N peers

  @<64-hex-pubkey>
    relays: <addr1>, <addr2>         # or: relays: (direct only)
    data: "<escaped-utf8>" (seq=N)   # --with-data ok, UTF-8
    data: 0x<hex>          (seq=N)   # --with-data ok, binary
    data: (not stored)               # --with-data, no record
    data: (error: <msg>)             # --with-data, fetch error
```

**Deduplication and ordering:**
- Peers deduplicated by `public_key`. Multiple discovery responses for the same
  peer merge their relay address lists (union, string equality).
- Output order: insertion order (first-seen public key appears first).
- `mutable_get` concurrency cap: 16 (buffer_unordered).

**stdout/stderr contract:**
- `--json`: NDJSON records go to stdout; `LOOKUP` header line goes to stderr.
- Without `--json`: all output goes to stderr; stdout is empty.

**Exit codes:**
```
0    Lookup completed successfully (zero peers found is still 0)
1    Fatal error (DHT init failure, lookup error)
130  SIGINT during lookup
```

Verify every field name and type against `lookup.rs` before marking complete.
Note discrepancies in `ISSUES.md`.

---

## Phase 5 — announce Chapter

Source files: `peeroxide-cli/src/cmd/announce.rs`, `peeroxide-cli/src/cmd/mod.rs`.

### 5a. `announce/overview.md`  (end-user focus)

Cover:
- What announce does: publishes a peer's presence on a DHT topic, optionally
  storing a mutable metadata blob, and optionally serving an echo protocol for
  connectivity testing by `ping --connect`.
- Key flags in plain English: `--seed`, `--data`, `--duration`, `--ping`.
- `--seed <passphrase>`: deterministic keypair from passphrase — same seed always
  produces the same public key and pickup address. Ephemeral if omitted.
- `--data <string>`: up to 1000 bytes of UTF-8 metadata stored on the DHT under
  the peer's public key. Discoverable via `lookup --with-data`.
- `--duration <secs>`: announce for a fixed time then exit cleanly.
- `--ping`: accept incoming connections and echo back all messages (allows
  `peeroxide ping --connect @<pubkey>` to measure RTT to this process).
- Worked example: announce a topic with metadata, then look it up with
  `peeroxide lookup --with-data` to see the metadata appear.

### 5b. `announce/architecture.md`  (app developer focus)

Cover:
- How announce uses the peeroxide library:
  - Uses `peeroxide::spawn()` with `SwarmConfig` (full swarm, not DHT-only).
  - Joins the topic via `handle.join(topic, JoinOpts { server: true, client: false })`.
  - Uses `handle.dht().mutable_put(&key_pair, data.as_bytes(), seq)` for `--data`.
- Key derivation:
  - Ephemeral (no `--seed`): `KeyPair::generate()`.
  - Seeded (`--seed <s>`): `KeyPair::from_seed(discovery_key(s.as_bytes()))`.
    - Single BLAKE2b-256 invocation — not a memory-hard KDF.
    - Cross-reference `concepts/peer-identity.md` for full derivation details.
- Metadata (`--data`) lifecycle:
  - `seq` = Unix epoch seconds at put time (`SystemTime::now()`).
  - Initial `mutable_put` on startup; background refresh every 600 s.
  - Max data size: 1000 bytes. Enforced before any DHT call; exits 1 if exceeded.
  - `seq` reuse: if two refreshes occur within the same second, the seq value is
    identical. DHT nodes may reject a put with a seq ≤ existing seq — note this
    as a known edge case in `ISSUES.md`.
- Firewall mode:
  - `--public`: uses `FIREWALL_OPEN`.
  - `--firewalled`: uses `FIREWALL_CONSISTENT`.
- Output (all to stderr; stdout is unused):
  - On start: `ANNOUNCE blake2b("<topic>") as @<pk_hex>` (seeded) or
    `ANNOUNCE blake2b("<topic>") as @<pk_hex> (ephemeral)`.
  - Metadata line: `  metadata: "<data>" (<N> bytes, seq=<seq>)`.
  - On clean exit: `UNANNOUNCE blake2b("<topic>")` then `  done`.
- Exit codes:
  ```
  0    Normal exit (including SIGINT / SIGTERM)
  1    Fatal error (data too large, swarm start failure, join failure)
  ```
  Note: announce returns 0 on Ctrl-C (unlike lookup which returns 130).
- Mermaid sequence diagram: announce startup → mutable_put → refresh loop →
  SIGINT → UNANNOUNCE → clean exit.

### 5c. `announce/echo-protocol.md`  (app developer focus)

This is the **canonical definition** of the echo protocol. The ping chapter
cross-references this page rather than re-documenting it.

Cover:
- Purpose: `announce --ping` makes the process act as an echo server so that
  `peeroxide ping --connect @<pubkey>` can measure RTT to a known process.
- Transport: connections arrive as `SwarmConnection` objects (Noise XX +
  ChaCha20-Poly1305 SecretStream). Cross-reference `concepts/encrypted-streams.md`.
- Concurrency cap: `MAX_ECHO_SESSIONS = 64`. Additional incoming connections
  are dropped immediately (semaphore try-acquire; no queuing).
- Session lifecycle (server side):
  1. Accept connection → try-acquire semaphore permit.
  2. **Handshake** (5 s timeout): expect first message = exactly 4-byte
     `PING` magic (`0x50 0x49 0x4E 0x47`). Reply with 4-byte `PONG`
     (`0x50 0x4F 0x4E 0x47`). Any other first message → close ("bad handshake").
  3. **Echo loop** (30 s idle timeout per read): expect messages of exactly
     `ECHO_MSG_LEN = 16` bytes. Echo each message back unchanged.
     Any non-16-byte message → close session.
  4. On disconnect: release semaphore, log
     `[disconnected] @<pk> (<N> probes echoed)`.
- Log lines (stderr):
  - `  [connected] @<pk> (echo mode)`
  - `  [disconnected] @<pk> (<N> probes echoed)`
- Constants table:

  | Constant             | Value                          |
  |----------------------|-------------------------------|
  | `PING_MAGIC`         | `b"PING"` (4 bytes)            |
  | `PONG_MAGIC`         | `b"PONG"` (4 bytes)            |
  | `MAX_ECHO_SESSIONS`  | 64                             |
  | `HANDSHAKE_TIMEOUT`  | 5 s                            |
  | `IDLE_TIMEOUT`       | 30 s                           |
  | `ECHO_MSG_LEN`       | 16 bytes                       |

- ASCII layout of an echo probe frame (defined by the **client** / ping side):
  ```
  Offset  Size  Field
  0       8     seq (u64, little-endian)
  8       8     timestamp_nanos (u64, little-endian)
  ```
  Total: 16 bytes. Server echoes it back unchanged; client computes RTT from
  `timestamp_nanos`.
- Mermaid sequence diagram: client → PING → server → PONG → client sends 16-byte
  probe → server echoes → repeat → disconnect.

---

## Phase 6 — Ping Chapter

Source files: `peeroxide-cli/src/cmd/ping.rs`, `peeroxide-cli/src/cmd/mod.rs`,
`peeroxide-dht/src/rpc.rs`.

### 6a. `ping/overview.md`  (end-user focus)

Cover:
- What ping does: connectivity diagnostics and reachability testing.
- The four operational modes determined by the target argument:
  1. No target -> bootstrap check: probes configured bootstrap nodes, discovers
     public address, classifies NAT type.
  2. `host:port` -> direct UDP probe to a specific node.
  3. `@<64-hex-pubkey>` -> find peer by public key on DHT, probe relay addrs.
  4. `<topic>` (64-hex or plain text) -> look up peers announcing on a topic,
     then probe them.
- When to use each mode (user-facing framing).
- Key flags in plain English: `--connect`, `--count`, `--interval`, `--json`,
  `--public`.
- One worked example per mode with a typical invocation and sample output.

### 6b. `ping/architecture.md`  (app developer focus)

Cover:
- Bootstrap check as a peeroxide library pattern:
  - Uses `HyperDhtHandle` directly (not the swarm).
  - Calls `dht.ping(host, port)` per bootstrap node.
  - `PingResponse` fields: `from`, `rtt`, `id` (remote node ID), `to`
    (reflexive address — our address as seen by the remote), `closer_nodes`.
  - Reflexive address aggregation across multiple bootstrap pings to classify
    NAT behavior.
- NAT classification logic (`NatType` enum):
  - `Open`: reflexive IP is a local interface address (detected by attempting
    `UdpSocket::bind(reflexive_ip:0)`), port matches local port.
  - `Consistent`: all samples report same `host:port`.
  - `Random`: same host, varying ports.
  - `MultiHomed`: samples report different hosts.
  - `Unknown`: no reflexive samples collected.
- `CMD_FIND_NODE` usage: bootstrap ping uses `CMD_FIND_NODE` (not `CMD_PING`)
  so that bootstrap nodes return closer nodes, accelerating routing table
  population. Document why this matters for app developers building their own
  bootstrap probing.
- The `--connect` flow as a library pattern: `HyperDhtHandle::connect()` →
  `SecretStream` → echo protocol. Mermaid sequence diagram required.
- Echo protocol: **do not re-document here**. Cross-reference
  `announce/echo-protocol.md` for the canonical definition (handshake magic
  bytes, probe frame layout, timeouts, concurrency cap). This page need only
  explain the client-side perspective: how ping initiates the handshake, sends
  probes, reads echoed frames, and computes RTT from `timestamp_nanos`.

### 6c. `ping/output-formats.md`

Document every JSON (NDJSON) field for each event type, with types and
semantics. Include the human-readable output structure alongside.

Bootstrap probe event:
```json
{
  "type": "bootstrap_probe",
  "node": "<host:port>",
  "seq": <integer>,
  "status": "ok" | "timeout" | "invalid",
  "rtt_ms": <float>,
  "node_id": "<64-hex>",
  "reflexive_addr": "<ip:port>",
  "closer_nodes": <integer>
}
```

Bootstrap summary event:
```json
{
  "type": "bootstrap_summary",
  "nodes": <integer>,
  "reachable": <integer>,
  "unreachable": <integer>,
  "nat_type": "<label>",
  "closer_nodes_total": <integer>,
  "public_host": "<ip>" | null,
  "public_port": <integer> | null,
  "port_consistent": <boolean>,
  "observed_ports": [<integer>, ...] | null,
  "observed_hosts": ["<ip>", ...] | null
}
```

Probe event (direct / pubkey / topic modes):
```json
{ "type": "probe", "seq": <integer>, "status": "ok" | "timeout",
  "rtt_ms": <float>, "node_id": "<64-hex>" | null }
```

Exit codes: `0` all probes succeeded, `1` partial or total failure,
`130` SIGINT.

Verify every field name and type against `ping.rs` before marking complete.
Note discrepancies in `ISSUES.md`.

---

## Phase 7 — cp Chapter

Source files: `peeroxide-cli/src/cmd/cp.rs`, `peeroxide/src/swarm.rs`,
`peeroxide-dht/src/secret_stream.rs`.

### 7a. `cp/overview.md`  (end-user focus)

Cover:
- What cp does: encrypted P2P file transfer over the swarm.
- `send` and `recv` sub-commands and their roles.
- Topic negotiation: sender prints topic to stdout (one line, nothing else on
  stdout). Receiver uses that topic to rendezvous.
  - **stdout contract**: `cp send` prints only the topic hex on stdout; all
    status and progress goes to stderr. Intentional for scripting.
- stdin/stdout support: `-` streams stdin (send) or stdout (recv).
- `--keep-alive`: sender accepts multiple sequential transfers on the same topic.
- Typical workflow with a worked example showing both sides simultaneously.

### 7b. `cp/protocol.md`  (app developer focus)

Cover:
- How cp uses the peeroxide swarm: sender joins as server
  (`JoinOpts { server: true, client: false }`); receiver joins as client
  (`JoinOpts { server: false, client: true }`). Explain the asymmetry.
- The two-message protocol over SecretStream:

  **Message 1 — Metadata** (JSON, single stream message):
  ```json
  { "filename": "<basename>", "size": <u64> | null, "version": 1 }
  ```
  - `filename`: basename of file, or `"stdin"` for pipe input.
  - `size`: byte count, or `null` for unknown (stdin).
  - `version`: always `1`. Receivers reject any other value.
  - Sent as one `stream.write()` / received as one `stream.read()`.
    SecretStream preserves message boundaries.

  **Messages 2..N — File data** (raw bytes):
  - Chunks up to `CHUNK_SIZE = 65536` bytes, one write per chunk.
  - Transfer ends when sender calls `stream.shutdown()` (clean EOF);
    receiver sees `stream.read()` return `None`.

- Atomic write: receiver writes to temp file
  (`.peeroxide-recv-<pid>-<rand>` in destination directory), renames to final
  path only after verifying complete transfer. Temp file removed on any error.
- Filename sanitization: receiver validates and sanitizes metadata filename
  to prevent path traversal. Warns if sanitized name differs.
- Mermaid sequence diagram of the full send/recv flow required.

### 7c. `cp/reliability.md`

Cover:
- Connection timeout: receiver waits up to `--timeout` (default 60 s) for
  initial sender connection.
- Per-read timeout: each `stream.read()` during data reception is bounded by
  `--timeout`; timeout triggers "transfer stalled" and abort.
- Size validation: if `size` was in metadata and received bytes != expected,
  receiver aborts and removes temp file.
- `--force`: allows overwriting an existing destination without prompting.
- `--keep-alive` incompatibility with stdin: keep-alive requires re-readable
  file input.
- Progress reporting: `--progress` flag; known size -> progress bar; unknown
  size -> spinner. Uses `indicatif`.
- Exit codes: `0` success, `1` fatal, `130` SIGINT.

---

## Phase 8 — deaddrop Chapter

Source files: `peeroxide-cli/src/cmd/deaddrop.rs`,
`peeroxide-cli/DEADDROP_V2.md`, `peeroxide-dht/src/rpc.rs`.

### 8a. `deaddrop/overview.md`  (end-user focus)

Cover:
- What deaddrop is: anonymous store-and-forward messaging via DHT mutable
  records. Data stored on public DHT, retrievable by anyone with the pickup key.
- `leave` and `pickup` sub-commands.
- The pickup key: 64-hex string printed to stdout on `leave`.
  - **stdout contract**: `deaddrop leave` prints only the pickup key on stdout.
    All status output goes to stderr.
- Passphrase mode: both sides derive the same pickup key from a shared
  passphrase (`--passphrase`).
- Data persists approximately 20 minutes after the last refresh. The `leave`
  process stays running to refresh records (default every 10 minutes).
- `--max-pickups`: stop refreshing after N acknowledged pickups.
- `--ttl`: stop refreshing after a wall-clock duration.
- Brief security model callout (link to appendix): data is signed but not
  encrypted; pre-encrypt sensitive content before leaving.
- Worked example: leaving a message and picking it up.

### 8b. `deaddrop/architecture.md`  (app developer focus)

Cover:
- How deaddrop uses the peeroxide library: uses `HyperDhtHandle` directly
  (no swarm); calls `mutable_put` and `mutable_get`.
- Chunk key derivation (deterministic from root seed):
  - Root keypair: `KeyPair::from_seed(root_seed)`
  - Chunk i keypair: `KeyPair::from_seed(discovery_key(root_seed || i.to_le_bytes() as u16))`
  - `root_seed` source: 32 random bytes (default), or
    `discovery_key(passphrase.as_bytes())` (passphrase mode).
- Record chain: singly-linked list of mutable DHT records. Each chunk carries
  the public key of the next chunk. Last chunk carries 32-byte zero `next_pk`.
  Root chunk additionally carries total chunk count and CRC-32C over entire
  reassembled payload.
- `seq` value: current Unix timestamp (seconds), monotonically increasing with
  each refresh.
- Pickup acknowledgement: receiver announces on
  `ack_topic = discovery_key(root_public_key_bytes || b"ack")` using an
  ephemeral keypair. Leaver polls this topic every 30 seconds via
  `handle.lookup(ack_topic)` and counts unique peer public keys as pickups.
  Acks are advisory — cannot be verified as genuine.
- Mermaid sequence diagrams required: one for the leave flow (publish chain),
  one for the pickup flow (chain walk + reassemble + ack).

### 8c. `deaddrop/format.md`

Byte-level reference. Use ASCII byte-layout tables for all structures.

**Root chunk (version `0x01`):**
```
Offset  Size  Field
0       1     Version (0x01)
1       2     total_chunks (u16, little-endian)
3       4     CRC-32C of reassembled payload (u32, little-endian)
7       32    next_chunk_public_key (32 x 0x00 if N = 1)
39      <=961 Payload bytes (first 961 bytes of data)
```
Total record size <= 1000 bytes.

**Non-root chunk (version `0x01`):**
```
Offset  Size  Field
0       1     Version (0x01)
1       32    next_chunk_public_key (32 x 0x00 if final chunk)
33      <=967 Payload bytes
```
Total record size <= 1000 bytes.

**Chunking algorithm:**
- If `data_len <= 961`: 1 chunk (root only).
- Otherwise: `N = 1 + ceil((data_len - 961) / 967)`.
- Maximum chunks: `N <= 65535` (u16), giving a maximum payload of ~60 MB.

**CRC-32C:**
- Computed over fully assembled payload (before chunking).
- Algorithm: CRC-32C (Castagnoli polynomial).
- Verified after reassembly on pickup; mismatch causes abort with no output written.

**`mutable_put` record:**
- Stored under chunk-specific keypair.
- Value = encoded chunk bytes as above.
- `seq` = Unix timestamp at publish time (seconds).

### 8d. `deaddrop/operations.md`

Cover:
- Default TTL and refresh: records expire ~20 minutes after last write.
  Default refresh interval: 600 seconds. Leave at least one full refresh
  interval before pickup.
- AIMD adaptive concurrency for publish:
  - Initial concurrency: 4 simultaneous `mutable_put` operations.
  - A put is "degraded" if the DHT reports commit timeouts.
  - Every 10 put completions (window):
    - Degraded ratio > 30% -> halve concurrency (minimum 1).
    - Degraded ratio = 0% -> increment concurrency by 1.
  - Adapts to local uplink saturation and DHT responsiveness.
- Pickup fetch retry: exponential backoff starting at 1 second, doubling to
  max 30 seconds. Default pickup timeout: 1200 seconds.
- stdin/stdout usage: `deaddrop leave -` reads from stdin; `deaddrop pickup`
  without `--output` writes reassembled bytes to stdout.
- Passphrase strength note: derivation is a single BLAKE2b-256 invocation,
  not a memory-hard KDF. Use long, random passphrases.
- Exit codes: `0` success, `1` fatal, `130` SIGINT.

### 8e. `deaddrop/future-direction.md`

Summarize the v2 two-chain design from `peeroxide-cli/DEADDROP_V2.md` at a
high level. Clearly label as not yet implemented.

Cover:
- **What changes**: single linked list split into an index chain (small records
  each pointing to multiple data chunks) and a data chain (independent records
  with no pointers). Receiver walks the short index chain first, then fetches
  all data chunks in parallel.
- **Why it's better**:
  - Dramatically lower pickup latency: parallel chunk fetch vs sequential walk.
    A 100 KB file that takes 2-5 minutes in v1 takes a few seconds in v2.
  - Higher capacity: ~1.9 GB theoretical maximum vs ~60 MB for v1.
  - Lower per-byte overhead (~0.1% vs ~3-4%).
- **Compatibility**: v2 uses version byte `0x02`; v1 uses `0x01`. Pickup key
  format is unchanged.
- Do NOT include detailed byte layouts here. Keep this section high level.

---

## Phase 9 — Appendices

### 9a. `appendices/security-model.md`

Cover for both audiences:
- **Authenticity, not confidentiality**: deaddrop records are Ed25519-signed
  (DHT validates signatures). This prevents tampering but does not provide
  confidentiality — payload bytes are stored in cleartext. Anyone with the
  pickup key can read the data.
- **Pre-encryption recommendation**: encrypt data with a tool like `age` before
  leaving, if confidentiality is required.
- **Pickup key as capability**: grants read access and ability to send spoofed
  acknowledgement announces. Protect accordingly.
- **Passphrase derivation**: `discovery_key(passphrase)` is fast by design.
  Not suitable for low-entropy secrets. Use random passphrases.
- **cp transport security**: encrypted end-to-end with Noise XX +
  ChaCha20-Poly1305. Remote public key verified during handshake.
- **Topic privacy**: DHT topics are public. Anyone can look up who is announcing
  on a topic. Use `discovery_key` of a secret string for topic privacy (no
  built-in protection).

### 9b. `appendices/limits-and-performance.md`

Cover:
- **deaddrop v1**: max payload ~60 MB. Sequential fetch; latency grows
  linearly with file size and DHT RTT.
- **cp**: file size limited only by disk/memory. `CHUNK_SIZE = 65536 bytes`.
  No protocol-level maximum.
- **DHT record limit**: max record value 1000 bytes (peeroxide-dht).
  `announce --data` enforces this client-side (≤ 1000 bytes).
- **Concurrency caps**: lookup `--with-data` caps concurrent `mutable_get` at
  16. deaddrop AIMD starts at 4. announce echo server caps at 64 sessions.
- **Record lifetime**: ~20 minutes TTL. Refresh every 10 minutes (deaddrop),
  every 10 minutes (announce `--data`). Allow one full refresh interval of
  margin after `leave` or `announce` before data is reliably readable.
- **Bootstrap dependency**: all operations require at least one reachable
  bootstrap node. Default list bundled in `peeroxide` crate
  (`DEFAULT_BOOTSTRAP`). Configurable via `--bootstrap` or config file.

---

## Phase 10 — Polish & Cross-Cutting

### 10a. Diagrams

Mandatory Mermaid sequence diagrams:
- `announce/architecture.md`: announce startup → mutable_put → refresh loop → exit.
- `announce/echo-protocol.md`: PING/PONG handshake + echo probe exchange.
- `cp/protocol.md`: full send/recv flow.
- `deaddrop/architecture.md`: leave flow AND pickup flow (two diagrams).
- `ping/architecture.md`: `--connect` flow (find_peer → connect → echo).
  **Note**: the echo protocol diagram is defined in `announce/echo-protocol.md`;
  the ping architecture page must cross-reference it, not duplicate it.

ASCII byte-layout tables required for all wire format descriptions in
`deaddrop/format.md` and `ping/architecture.md` (echo probe frame).

### 10b. Cross-references

Use mdBook `[text](../concepts/chapter.md)` links. Do not duplicate
explanations across chapters — reference the Concepts chapter.

### 10c. Introduction update

After all chapters are written, revisit `introduction.md`:
- Add a "Quick Navigation" table mapping task to chapter.
- Accurate summary of book contents.
- Links to peeroxide on crates.io and docs.rs.

### 10d. Full build verification

Run `mdbook build docs/` with all content in place. Fix broken links, missing
`SUMMARY.md` entries, failed Mermaid renders. Confirm directory structure is
correct.

---

## Phase 11 — AGENTS.md Files

### `docs/AGENTS.md` (create)

Instructions for maintaining the mdBook:
- How to add a new chapter: add entry to `SUMMARY.md`, create `.md` file, run
  `mdbook build docs/` to verify.
- Diagram conventions: Mermaid for flows, ASCII tables for binary formats.
- Before merging to main: run `mdbook build docs/` and verify 0 errors. Check
  that any changes to `peeroxide-cli/src/cmd/` files are reflected in the
  corresponding chapter.
- Deployment: `.github/workflows/docs-site.yml` deploys automatically on push
  to `main`. No manual steps needed.

### `peeroxide-cli/AGENTS.md` (create)

Instructions scoped to the CLI crate:
- When `src/cmd/announce.rs` changes: review and update
  `docs/src/announce/architecture.md`, `docs/src/announce/echo-protocol.md`,
  and `docs/src/appendices/limits-and-performance.md`.
- When `src/cmd/lookup.rs` changes: review and update
  `docs/src/lookup/output-formats.md` and
  `docs/src/appendices/limits-and-performance.md`.
- When `src/cmd/deaddrop.rs` changes: review and update
  `docs/src/deaddrop/format.md`, `docs/src/deaddrop/architecture.md`, and
  `docs/src/appendices/limits-and-performance.md`.
- When `src/cmd/cp.rs` changes: review and update `docs/src/cp/protocol.md`
  and `docs/src/cp/reliability.md`.
- When `src/cmd/ping.rs` changes: review and update
  `docs/src/ping/architecture.md` and `docs/src/ping/output-formats.md`.
- Before `cargo publish` or pushing to main: verify `CHANGELOG.md` is updated
  and `ISSUES.md` has been reviewed.
- Wire format constants (magic bytes, chunk sizes, header offsets) are
  documented in `docs/src/deaddrop/format.md`, `docs/src/ping/architecture.md`,
  and `docs/src/announce/echo-protocol.md`. If these change in source, update
  those pages before publish.

### Root `AGENTS.md` (append or create)

Workspace-level section:
- `docs/` contains an mdBook with technical reference documentation for the
  peeroxide-cli tools (lookup, announce, ping, cp, deaddrop).
- Before merging PRs that change `peeroxide-cli/src/cmd/lookup.rs`,
  `announce.rs`, `deaddrop.rs`, `cp.rs`, or `ping.rs`: check `docs/AGENTS.md`
  for which doc chapters need updating.
- See `docs/AGENTS.md` for mdBook maintenance instructions.

---

## Completion Criteria

The loop is done when all of the following are true:

- [ ] `DOCS_PLAN.md` exists and every checkbox is `[X]`
- [ ] `mdbook build docs/` exits 0 with no warnings or broken links
- [ ] Every chapter has been verified against its source files
- [ ] All Mermaid sequence diagrams render correctly in the built HTML
- [ ] All ASCII byte-layout tables are present for deaddrop frame formats and the echo probe frame
- [ ] Echo protocol is defined exactly once (`announce/echo-protocol.md`); ping chapter cross-references it
- [ ] `AGENTS.md` files exist at `docs/`, `peeroxide-cli/`, and workspace root
- [ ] `ISSUES.md` exists and has been reviewed (may be empty)
- [ ] All commits are clean — one per completed work item, no WIP commits
- [ ] Nothing has been pushed

---

## Canonical Reference Data

Use these values verbatim. Verify each against source before writing into docs.
Note any discrepancy in `ISSUES.md`.

### lookup constants
```
mutable_get concurrency cap  = 16 (buffer_unordered)
Deduplication key            = public_key ([u8;32])
Relay dedup                  = string equality on "host:port"
Output order                 = insertion order (first-seen pubkey)
SIGINT exit code             = 130
```

### announce constants
```
Max --data size              = 1000 bytes
seq strategy                 = Unix epoch seconds (SystemTime::now())
Data refresh interval        = 600 s
PING_MAGIC                   = b"PING" (0x50 0x49 0x4E 0x47)
PONG_MAGIC                   = b"PONG" (0x50 0x4F 0x4E 0x47)
MAX_ECHO_SESSIONS            = 64
HANDSHAKE_TIMEOUT            = 5 s
IDLE_TIMEOUT                 = 30 s
ECHO_MSG_LEN                 = 16 bytes
Echo probe frame             = [seq: u64 LE][timestamp_nanos: u64 LE]
SIGINT exit code             = 0  (note: unlike lookup, announce exits 0 on Ctrl-C)
```

### deaddrop v1 constants```
MAX_PAYLOAD              = 1000 bytes
ROOT_HEADER_SIZE         = 39 bytes
ROOT_PAYLOAD_MAX         = 961 bytes
NON_ROOT_HEADER_SIZE     = 33 bytes
NON_ROOT_PAYLOAD_MAX     = 967 bytes
VERSION                  = 0x01
MAX_CHUNKS               = 65535
Default refresh_interval = 600 s
Default pickup timeout   = 1200 s
AIMD initial concurrency = 4
AIMD window_size         = 10 puts
AIMD degrade threshold   = 30%
Fetch backoff: start 1 s, max 30 s
Ack poll interval        = 30 s
Ack topic derivation     = discovery_key(root_public_key_bytes || b"ack")
```

### cp constants
```
CHUNK_SIZE             = 65536 bytes
Metadata version field = 1
Default recv timeout   = 60 s
```

### ping constants
```
PING magic   = 0x50 0x49 0x4E 0x47  ("PING")
PONG magic   = 0x50 0x4F 0x4E 0x47  ("PONG")
Echo probe   = 16 bytes: [seq: u64 LE][timestamp_nanos: u64 LE]
ECHO_TIMEOUT = 5 s
SIGINT_EXIT  = 130
```

### NAT classification constants (from peeroxide-dht)
```
FIREWALL_UNKNOWN    = 0
FIREWALL_OPEN       = 1
FIREWALL_CONSISTENT = 2
FIREWALL_RANDOM     = 3
```

### stdout/stderr contracts (state explicitly in each tool's overview)
- `lookup`: stdout empty without `--json`; with `--json` NDJSON peer/summary records on stdout. `LOOKUP` header always stderr.
- `announce`: stdout always empty. All output → stderr.
- `cp send`: only topic hex on stdout. All other output -> stderr.
- `deaddrop leave`: only pickup key (64-hex) on stdout. All other -> stderr.
- `deaddrop pickup` without `--output`: reassembled bytes on stdout.
  Status messages -> stderr.
- `peeroxide node`: listen address (`host:port`) on stdout on startup (not
  via logging).

### Exit codes (all commands)
```
0   Success
1   Fatal error / partial failure
130 SIGINT (Ctrl-C)
```
