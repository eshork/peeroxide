# DHT Operation Reference — Working Note

> **Status**: working / historical internal cheat-sheet used while designing the chat subsystem. **Not user-facing documentation.** This file is proposed for removal — see the PR description's Working Files table. The canonical concept-level DHT documentation lives in [`docs/src/concepts/`](../docs/src/concepts/) and the chat-specific protocol pages live in [`docs/src/chat/`](../docs/src/chat/).

## Appendix A: DHT Operation Reference

### A.1 — `immutable_put` / `immutable_get` — Content-addressed storage

Stores arbitrary bytes on DHT nodes, addressed by the hash of the value
itself (BLAKE2b-256). Content-addressed: you can only retrieve it if you
already know the hash.

- **`immutable_put(value: &[u8])`** — computes `target = hash(value)`,
  queries the K closest nodes to that target, commits the raw bytes.
  Returns the 32-byte hash.
- **`immutable_get(target: [u8; 32])`** — queries nodes closest to target;
  any node that has the value returns it. Client verifies
  `hash(returned_value) == target`.

| Property | Detail |
|----------|--------|
| Data stored | Raw `Vec<u8>` — arbitrary bytes, no signing, no keys, no seq |
| Addressing | `hash(value)` — immutable; changing value = different address |
| Max payload | ~900–1000 bytes (UDP framing; no explicit code constant) |
| Wire commands | `IMMUTABLE_PUT = 8`, `IMMUTABLE_GET = 9` |
| Discoverability | Reader must already know the hash (given out-of-band or via a mutable pointer) |

### A.2 — `mutable_put` / `mutable_get` — Signed, updateable storage

Stores arbitrary bytes signed by an Ed25519 keypair, addressed by
`hash(public_key)`. The owner can update the value by incrementing a
sequence number.

- **`mutable_put(key_pair, value: &[u8], seq: u64)`** — computes
  `target = hash(public_key)`, signs `(seq, value)` with the secret key,
  sends `MutablePutRequest { public_key, seq, value, signature }` to
  closest nodes.
- **`mutable_get(public_key: &[u8; 32], seq: u64)`** — queries with
  `target = hash(public_key)` and requested minimum seq. Nodes return
  stored value only if `stored.seq >= requested_seq`. Client verifies
  signature.

| Property | Detail |
|----------|--------|
| Data stored | `{ public_key: [u8;32], seq: u64, value: Vec<u8>, signature: [u8;64] }` |
| Addressing | `hash(public_key)` — one mutable slot per keypair |
| Max payload (value) | **~1002 bytes** (token present, seq ≤ 252; derived from `libudx MAX_PAYLOAD=1180` minus wire overhead) |
| Seq semantics | Strictly monotonic. `SEQ_REUSED (16)` if equal; `SEQ_TOO_LOW (17)` if lower |
| Salt support | ❌ Not implemented — no salt field; one record per keypair |
| Wire commands | `MUTABLE_PUT = 6`, `MUTABLE_GET = 7` |

### A.3 — `announce` / `lookup` — Peer discovery

Peer discovery primitives. Store structured peer records (public key +
relay addresses) under a topic hash. **Not general value storage.**

- **`announce(target: [u8;32], key_pair, relay_addresses)`** — queries
  closest nodes for the topic, sends a signed `AnnounceMessage` containing
  `HyperPeer { public_key, relay_addresses }`. Multiple peers can announce
  under the same topic simultaneously.
- **`lookup(target: [u8;32])`** — queries closest nodes; they return
  `LookupRawReply { peers: Vec<HyperPeer>, bump }` — all peers that have
  announced on that topic (up to 20 per node).

| Property | Detail |
|----------|--------|
| Data stored | `HyperPeer { public_key: [u8;32], relay_addresses: Vec<Ipv4Peer> }` |
| Multi-writer | ✅ Yes — up to 20 announcers per topic per node |
| IP in stored record | ❌ No — source IP is NOT stored in `HyperPeer`; only pubkey + relay_addresses |
| Announce with no addresses | ✅ Yes — `relay_addresses = []` is valid |
| `MAX_RECORDS_PER_LOOKUP` | 20 per node (per-node cap; total across all queried nodes can exceed 20) |
| `MAX_RELAY_ADDRESSES` | 3 (truncated on store) |
| Wire commands | `LOOKUP = 3`, `ANNOUNCE = 4`, `FIND_PEER = 2` |

**Key difference from put/get:**
- `lookup`/`announce` is multi-writer — many peers announce under one topic.
- `put`/`get` is single-writer — one value per address.
- Announce stores structured peer connection info; put stores opaque bytes.

### A.4 — TTL (Time-To-Live)

All stored values are ephemeral — they expire from node storage.

| Storage type | TTL (default) |
|---|---|
| Announcement records (`RecordCache`) | 20 minutes (`max_record_age`) |
| Mutable/Immutable LRU cache | 20 minutes (`max_lru_age`) |
| Router forward entries | 20 minutes (`DEFAULT_FORWARD_TTL`) |

Clients must periodically re-announce / re-put to keep data alive.
The 20-minute default matches the Node.js reference implementation.

### A.5 — Practical Size Budget for Chat Messages

Starting from `libudx MAX_PAYLOAD = 1180 bytes` and subtracting wire
overhead for a `mutable_put` with token present and `seq ≤ 252`:

```
1180  libudx MAX_PAYLOAD
 - 75  outer RPC Request fixed fields (type, flags, tid, to, token, command, target)
 -  3  outer compact-encoding length prefix for put_bytes
 - 32  public_key field
 -  1  seq compact-encoding (1 byte for seq ≤ 252)
 -  3  inner compact-encoding length prefix for value
 - 64  signature
─────
1002  bytes available for message value payload
```

For the chat message envelope (author pubkey 32 + timestamp 8 + type 1 +
signature 64 + framing ~10 ≈ 115 bytes overhead), a single-frame message
has approximately **~887 bytes** for actual text content.

Messages exceeding ~900 bytes use linked mutable record chains, using
`MAX_PAYLOAD = 1000` and `ROOT_HEADER_SIZE = 39` / `NON_ROOT_HEADER_SIZE = 33`.
