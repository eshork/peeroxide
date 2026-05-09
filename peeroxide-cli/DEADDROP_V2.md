# Dead Drop v2: Two-Chain Storage Protocol

This document describes the v2 dead-drop wire protocol shipped in `peeroxide-cli`. It supersedes the v1 single linked-list design with a two-chain architecture that enables parallel data fetch while preserving read-only get semantics.

## Motivation

The v1 format stores a payload as a single linked list of mutable signed records. Each record carries a small payload and the public key of the next record, so a receiver must walk the chain strictly in order. A 100 KB payload requires roughly 107 sequential DHT round-trips, taking minutes on the public network even when individual queries are fast.

v2 separates the responsibilities of the chain. A short index chain — small mutable signed records — enumerates the data chunks. The data chunks themselves are immutable, content-addressed records that can be fetched in parallel as soon as their content hashes are known. The index chain is walked sequentially because each index record names the next; data chunks are scheduled concurrently as content hashes are discovered.

## Architecture

```
Index chain (mutable, sequential fetch, small):
  [root idx] → [idx 1] → [idx 2] → ... → [idx K]  (next=zeros at end)
      │            │           │
      ▼            ▼           ▼
Data chain (immutable, content-addressed, parallel fetch):
  [d0..d28]    [d29..d58]   [d59..d88] ...
```

- The **index chain** is a singly linked list of mutable signed records. The root index record is published under the root keypair (its public key is the pickup key); each non-root index record is published under a keypair derived from the root seed. Each index record carries `next_pk` (the public key of the next index record, or 32 zero bytes if it is the final index record) and a sequence of 32-byte content hashes naming data chunks.
- The **data chain** is a flat collection of immutable, content-addressed records. Each data chunk is stored at a DHT address equal to the BLAKE2b-256 hash of the chunk's encoded bytes. The DHT verifies on every read that the returned bytes hash to the requested address, so data chunks are self-verifying. There are no pointers between data chunks.

## Frame Formats

### Data chunk

```
Offset  Size   Field
0       1      Version (0x02)
1       ...    Payload (raw file bytes, up to 999 bytes)
```

Header overhead: 1 byte. Maximum payload: 999 bytes.
DHT address: `discovery_key(encoded_chunk)` (BLAKE2b-256 of the full encoded bytes including the version prefix).
Stored via `immutable_put`. No keypair, no signature, no metadata, no chain pointer.

### Root index chunk

```
Offset  Size   Field
0       1      Version (0x02)
1       4      Total file size in bytes (u32 LE)
5       4      CRC-32C (Castagnoli) of fully assembled payload (u32 LE)
9       32     Next index chunk public key (32 zeros if single index chunk)
41      ...    Data chunk content hashes (32 bytes each, up to 29 per root)
```

Header overhead: 41 bytes. 29 data chunk content hashes per root.
Stored via `mutable_put` (signed by the root keypair).

### Non-root index chunk

```
Offset  Size   Field
0       1      Version (0x02)
1       32     Next index chunk public key (32 zeros if final index chunk)
33      ...    Data chunk content hashes (32 bytes each, up to 30 per chunk)
```

Header overhead: 33 bytes. 30 data chunk content hashes per non-root index chunk.
Stored via `mutable_put` (signed by the index keypair derived for that position).

### Need-list record

```
Offset  Size   Field
0       1      Version (0x02)
1       ...    Packed entries (variable-length)
```

Total record size ≤ 1000 bytes. Useful capacity: 999 bytes after the version byte.

Entry types:

- `0x00` Index range: `[0x00][start_u16_le][end_u16_le]` = **5 bytes**. Inclusive index chunk positions. Capacity: up to 199 entries per record.
- `0x01` Data range: `[0x01][start_u32_le][end_u32_le]` = **9 bytes**. Inclusive data chunk positions. Capacity: up to 111 entries per record.

An empty payload (the record value is zero bytes, with no version byte) is the receiver-done sentinel.

Decoding requirements: a non-empty first byte MUST be `0x02`; tag bytes MUST be `0x00` or `0x01`; entries MUST NOT be truncated. Decoders reject any record that violates these rules.

## Topics & Records

- **Pickup key**: the public key of the root keypair, `KeyPair::from_seed(root_seed).public_key`. The root index record is the mutable record stored at this public key.
- **Non-root index records**: stored as mutable records at the public key of `derive_index_keypair(root_seed, i)` for `i ∈ [1, 65535]`.
- **Data chunks**: stored as immutable records, addressed by `discovery_key(encoded_chunk)`. Self-verifying on every fetch.
- **Need topic**: `discovery_key(root_pk || b"need")`. Receivers announce on this topic and store need-list records under their own ephemeral keypair.
- **Ack topic**: `discovery_key(root_pk || b"ack")`. Receivers announce on this topic with an ephemeral keypair and no payload.

## Key Derivation

```
root_seed:         32 bytes (random or discovery_key(passphrase))
root_keypair:      KeyPair::from_seed(root_seed)                         // index chunk 0 (root)
index_keypair[i]:  KeyPair::from_seed(discovery_key(root_seed || b"idx" || i_as_u16_le))
                                                                          // i ∈ [1, 65535]
```

The 3-byte ASCII domain separator `b"idx"` prevents key collisions with other derivations from the same root seed. The pickup key is the root public key. The receiver never learns `root_seed`, so it cannot derive any private key in the index chain and cannot forge index records.

Data chunks have no derived keypair — they are addressed solely by content hash. Anyone in possession of a data chunk's content hash can fetch the chunk and verify it; the DHT validates `discovery_key(value) == target` on every `immutable_get` response.

## Fetch Protocol (Receiver)

A receiver begins with the pickup key (the root public key) and proceeds:

1. Has the pickup key.
2. `mutable_get(root_pubkey, 0)` retrieves the root index record. Parse it to learn `file_size`, the stored CRC-32C, the first batch of data content hashes, and the next index pointer.
3. Walk the index chain: while `next_pk != [0u8; 32]`, `mutable_get(next_pk, 0)` to retrieve the next non-root index record, parse it, and accumulate its data content hashes. Track every `next_pk` already visited; if `next_pk` repeats, abort (loop detection).
4. As each index chunk parses, schedule `immutable_get(content_hash)` for each data hash through a shared concurrency budget capped at 64 permits. Pipelining is an implementation choice; conformant receivers may serialize.
5. Each `immutable_get(target)` is self-verifying: the DHT checks `discovery_key(value) == target` before returning a value.
6. Reassemble in index order — concatenate each chunk's payload (strip the leading version byte). Verify that the total reassembled length equals the stored `file_size`.
7. Compute CRC-32C of the reassembled payload. If it does not match the stored CRC, abort.
8. Write the output (file or stdout).
9. Optionally announce on the ack topic (see the Pickup Acknowledgement Channel section).

Receivers MUST: reject any chunk whose first byte is not `0x02`; detect index-chain loops; verify CRC-32C; abort on size mismatch.
Receivers SHOULD (implementation choices): pipeline data fetches; use frontier-probing retry on per-chunk timeout; publish need-list records on no-progress cycles.

## Write Protocol (Sender)

A sender begins with input bytes and a root seed (random or derived from a passphrase):

1. Read the input; validate that its length does not exceed `MAX_FILE_SIZE` (1,964,112,921 bytes).
2. Compute CRC-32C of the entire payload.
3. Split the payload into chunks of at most 999 bytes. Encode each chunk as `[0x02][payload_bytes]`. Compute `discovery_key(encoded)` for each — that hash is its DHT address.
4. Distribute content hashes across the index chain: the first 29 hashes go in the root index chunk; the next 30 in non-root index chunk 1; and so on.
5. Derive the index keypairs (the root from `root_seed`; each non-root via `derive_index_keypair(root_seed, i)`). Encode each index chunk.
6. Publish: data chunks via `immutable_put`; index chunks via `mutable_put`, each signed by its derived keypair with `seq` set to the current Unix timestamp. Both fan out concurrently.
7. Print the pickup key (the root public key, 64-character hex) to stdout.
8. Enter a refresh loop, monitoring the ack topic and the need topic until terminated.

Senders MUST: sign each index record with its associated derived keypair; use a monotonically increasing `seq` (the current Unix timestamp is the canonical choice) on every `mutable_put`.
Senders SHOULD (implementation choices): pipeline publishes through a shared concurrency budget; honor rate limits; poll the ack topic; service need-list requests.

## Refresh Protocol

DHT records expire after roughly 20 minutes on the public network. The sender keeps the dead drop alive by republishing:

- **Index chunks** are re-published via `mutable_put` with `seq` set to the current Unix timestamp (or any monotonically increasing value).
- **Data chunks** are re-published via `immutable_put` with the same encoded bytes. Immutable records have no `seq`; re-storage refreshes the DHT TTL.
- The refresh interval is implementation-defined. This implementation defaults to 600 seconds, which is well within the DHT's ~20-minute TTL.

## Need-List Feedback Channel

### Purpose

The need-list channel lets a receiver tell the sender which chunk ranges are still missing, so the sender can prioritize re-publishing them.

### Topic

`need_topic = discovery_key(root_pk || b"need")`.

### Receiver behavior

- Once per session, generate an ephemeral `need_kp = KeyPair::generate()`.
- Announce on the need topic: `announce(need_topic, &need_kp, &[])`.
- When stuck on missing chunks: encode the missing ranges as a need-list record and publish via `mutable_put(&need_kp, &encoded, seq)`, with `seq` strictly greater than any previous value used for `need_kp`.
- On exit (success or failure): publish an empty record via `mutable_put(&need_kp, &[], seq+1)`. The empty payload signals "done".

### Sender behavior

- Periodically `lookup(need_topic)` to discover announced need-list publishers.
- For each peer returned: `mutable_get(peer.public_key, 0)`, then `decode_need_list(value)`.
- For each `NeedEntry::Index { start, end }`: re-publish the named index chunks AND every data chunk those indices reference.
- For each `NeedEntry::Data { start, end }`: re-publish the named data chunks.

### Validation requirements (both sides)

- An empty record is an empty list (and the receiver-done sentinel).
- A non-empty first byte MUST be `0x02`.
- Tag bytes MUST be `0x00` (index range) or `0x01` (data range).
- Truncated entries → reject the entire record.

## Pickup Acknowledgement Channel

### Purpose

The ack channel allows senders to detect that one or more pickups have occurred, enabling early-exit policies.

### Topic

`ack_topic = discovery_key(root_pk || b"ack")`.

### Receiver behavior

On successful reassembly, CRC verification, and output write: generate an ephemeral `ack_kp = KeyPair::generate()` and call `announce(ack_topic, &ack_kp, &[])` — announce only, no payload. Receivers may suppress this announcement.

### Sender behavior

Periodically `lookup(ack_topic)` and count unique announcer public keys via a set. The sender may exit early once the count reaches a target threshold.

### Soundness note

The ack channel does NOT prove successful reassembly — only that some peer announced. Treat ack counts as an optimization signal, never as a correctness check.

## Conformance Requirements

### Required (wire protocol invariants)

- All v2 frame and record types use version byte `0x02` as the first byte.
- Index chunks are stored via `mutable_put`, signed by their derived keypair.
- Data chunks are stored via `immutable_put`; their address is `discovery_key(encoded_chunk)`.
- Index records contain 32-byte data **content hashes**, not data chunk public keys.
- Root index header layout: `[0x02][file_size_u32_le][crc_u32_le][next_pk_32B][hashes...]`.
- Non-root index header layout: `[0x02][next_pk_32B][hashes...]`.
- Need-list records are mutable, formatted as defined in the Frame Formats section.
- Receivers MUST detect index-chain loops, validate version bytes on every parsed record, and verify CRC-32C of the reassembled payload.
- Senders MUST sign every `mutable_put` with the keypair associated with that record's position and use a monotonically increasing `seq`.

### Optional (implementation choices, documented for context)

- Pipelining index walks and data fetches under a shared concurrency budget.
- AIMD-controlled rate limiting on the sender side.
- Frontier-probing retry on missing data chunks.
- Ack channel announcement on successful pickup (receivers MAY suppress).
- Sender polling cadence for the need and ack topics.

## Practical Limits

- Data chunk payload: 999 bytes.
- Pointers per index chunk: 29 (root) / 30 (non-root).
- Index chain length: up to 65,535 non-root chunks (the u16 bound on the derivation index), plus the root.
- Format maximum: `29 + 65535 × 30 = 1,966,079` data chunks → `≈ 1.83 GB` (1,964,112,921 bytes precisely; the value of `PARALLEL_FETCH_CAP` is 64 permits at the receiver).
- DHT record TTL on the public network: ~20 minutes; the refresh interval should be ≤ TTL/2.
- An empty input file is valid: it produces 0 data chunks and 1 root index with `file_size = 0`, `crc = 0`, and no hashes.

## Security Properties

- The pickup key is the root public key — a read-only capability for the index chain root.
- Each index chunk is signed by a unique keypair derived from `root_seed` via the `b"idx"` domain separator. A receiver, knowing only the pickup key, cannot derive any private key and cannot forge index records.
- Each data chunk is content-addressed: the DHT validates `discovery_key(value) == target` on every `immutable_get` response, so a malicious DHT node cannot return forged data without being detected.
- DHT nodes can read plaintext payloads. Encrypt before dropping if confidentiality is required.
- Data chunk content addresses are opaque to anyone who has not walked the index chain.
- The need-list channel uses an ephemeral receiver keypair: only that receiver can write to or clear its own need list.
- The ack channel is announce-only and unauthenticated; ack counts are a heuristic, not a correctness signal.

## Comparison to v1

| Property | v1 (single linked-list) | v2 (two-chain) |
|----------|------------------------|----------------|
| Data payload per chunk | 961 (root) / 967 (non-root) | **999** |
| Data chain mutability | Mutable signed records | **Immutable, content-addressed** |
| Data chain key derivation | Per-chunk derived keypair | None — `discovery_key(encoded)` |
| Fetch pattern | All sequential | Index sequential + data **parallel, pipelined** |
| Receiver→sender feedback | None | **Need-list channel** |
| Pickup acknowledgement | Ack topic | Ack topic (same scheme) |
| Read/write separation | ✓ (pickup key = root pubkey) | ✓ (same) |
| Forgery protection | Per-chunk signature | Index: signature; Data: content-hash self-verification |
| Format max file size | ~60 MB (u16 chunk count) | **≈ 1.83 GB** (29 + 65535×30 chunks × 999 B) |
| 100 KB fetch time | ~107 sequential queries (2-5 min) | ~4 index + ~107 parallel (seconds) |
| 1 MB fetch time | ~1000 sequential queries (15-50 min) | ~34 index + ~1000 parallel (~1 min) |
| Overhead per data byte | 3.4-3.9% | **0.1%** |
| Complexity | Simple | Moderate |

## Migration Notes

- The version byte `0x02` distinguishes v2 frames from v1 (`0x01`) at the root chunk and all downstream records.
- The receiver auto-detects the format by reading the version byte of the root chunk. No flag is required to read either format.
- The pickup key format is unchanged from v1 (a 64-character hex root public key).
- Passphrase mode works identically: `passphrase → discovery_key(passphrase) → root_seed → root_keypair → root_pubkey`.

## Resolved Decisions

- **Parallel-fetch concurrency cap**: 64 permits (`PARALLEL_FETCH_CAP` in `v2.rs`). The same semaphore is shared between index-walk fetches and data-chunk fetches.
- **Pipelined index walk + data fetch**: yes. As each index chunk parses, its data content hashes are immediately scheduled for `immutable_get` through the shared semaphore; the index walk continues without waiting for data results.
- **Partial data-fetch error handling**: frontier-probing retry. The receiver identifies contiguous missing ranges; the retry queue prioritizes the first chunk of each range and then fills the remaining budget with the rest of each range concurrently. On a no-progress cycle, the receiver publishes a need-list record and waits before retrying. The whole process is bounded by a per-chunk timeout.
- **Initial publish ordering**: no constraint beyond data content addresses being known once the index chain has been built. Both index (`mutable_put`) and data (`immutable_put`) operations fan out concurrently through one shared scheduler.
