# Dead Drop v2: Tree-Indexed Storage Protocol

> **Status**: working / historical design document. The wire format described below matches the shipped v2 protocol structurally, but some implementation-level details have diverged. Notably, the per-deaddrop salt described below as `root_seed[0]` is currently **forced to `0x00`** in the shipped implementation (`peeroxide-cli/src/cmd/deaddrop/v2/keys.rs::salt`); the salt slot in the data-chunk header is reserved for future per-deaddrop randomization but is not derived per-deaddrop yet. The canonical, current `dd` documentation lives in [`docs/src/dd/`](../docs/src/dd/) (overview, architecture, format, operations). This file is proposed for removal — see the PR description's Working Files table.

This document describes the v2 dead-drop wire protocol shipped in `peeroxide-cli`. v2 uses version byte `0x02` and supersedes the simpler v1 single-chain design (`0x01`), which is retained as a minimal reference implementation.

> **Lineage note.** An earlier draft of v2 used a singly linked list of index records over a separately content-addressed data layer. That draft was never published to the public DHT; the current spec replaces it in place under the same wire byte. Where references to "linked-list v2", "v2-original", or "the earlier v2 draft" appear below, they describe that retired draft and exist only to motivate design choices in the current spec.

## Motivation

The retired v2 draft separated the index and data layers, making data fetch fully parallel. But the index itself remained a singly linked list: each index record named the next, so a receiver had to walk the index chain strictly in order. For a 1 GB payload, that was roughly 35,800 sequential `mutable_get` round trips on the critical path, even though every data chunk could be fetched in parallel once its content hash was known. Empirically the data fetcher consistently caught up to the index walk and starved waiting for the next index hop.

v2 turns the index layer into a tree. Each non-root index chunk holds slots of a single kind: either child *index* pubkeys (a non-leaf chunk) or *data* chunk content hashes (a leaf chunk). The kind is not encoded on the wire — instead, the canonical construction algorithm is normative, so the receiver derives the tree's depth from `file_size` and tracks "remaining depth" as it descends. The receiver fetches the root, learns its children, fetches all children in parallel, and recurses. The number of sequential round trips on the critical path drops from `O(N/31)` to `O(log₃₁ N)` — for 1 GB, that is **6 round trips total** (5 sequential index waves plus one data wave) instead of ~35,800.

Data chunks remain immutable and content-addressed; the change is confined to the index layer's shape. A 1-byte per-deaddrop salt is added to every data chunk header so that two unrelated deaddrops with identical content do not share a DHT address-space.

## Architecture

```
                            Index tree (mutable, BFS-fetchable, parallel)
                                          [root idx]
                                       /      |      \
                                      /       |       \
                                  [L1.0]   [L1.1]   [L1.2]   ...   (up to 30 children)
                                  /  \     /  \    /  \
                              [L2.0]...                      ...   (up to 31 children each)
                                  ...
                              [leaf]  [leaf]  [leaf]  ...          (final index level)
                                │       │       │
                                ▼       ▼       ▼
                            Data layer (immutable, content-addressed, parallel)
                            [d0..d30] [d31..d61] [d62..d92] ...    (up to 31 per leaf)
```

- The **index tree** is a tree of mutable signed records. Every index chunk holds a sequence of 32-byte slots — either all data content hashes (a leaf-index chunk) or all child index pubkeys (a non-leaf index chunk). The wire format does not mark which type a chunk is; both senders and receivers derive each chunk's slot kind from its tree position, which is itself computed from `file_size` via the canonical tree-shape rule (see Tree Construction). The root is published under the root keypair (its public key is the pickup key); every non-root index chunk is published under a keypair derived deterministically from the root seed.
- The **data layer** is a flat collection of immutable, content-addressed records. Each data chunk is stored at a DHT address equal to the BLAKE2b-256 hash of the chunk's encoded bytes, including a 1-byte per-deaddrop salt. The DHT verifies on every read that the returned bytes hash to the requested address, so data chunks are self-verifying.

Round-trip cost on the critical path is bounded by tree depth plus one (for the final data-chunk wave). Data fetches at every tree level overlap with index fetches at deeper levels.

## Frame Formats

All v2 frames begin with version byte `0x02`. Maximum encoded chunk size is 1000 bytes; the DHT enforces this on `mutable_put` and `immutable_put`.

### Data chunk

```
Offset  Size   Field
0       1      Version (0x02)
1       1      Salt (per-deaddrop, see Key Derivation)
2       ...    Payload (raw file bytes, up to 998 bytes)
```

Header overhead: 2 bytes. Maximum payload: 998 bytes.
DHT address: `discovery_key(encoded_chunk)` (BLAKE2b-256 of the full encoded bytes including the version and salt prefix).
Stored via `immutable_put`. No keypair, no signature, no chain pointer.

The salt byte makes the DHT address unique per deaddrop even when two deaddrops contain identical file content; see Key Derivation below.

### Non-root index chunk

```
Offset  Size   Field
0       1      Version (0x02)
1       ...    Slot payload: N × 32 bytes
```

Header overhead: 1 byte. Maximum slot count: `(1000 - 1) / 32 = 31` slots (`N ≤ 31`).
A chunk with fewer than 31 slots is permitted (typically the trailing chunk of a partially filled level).
Stored via `mutable_put`, signed by the index keypair derived for that position.

A non-root index chunk holds *either* child index pubkeys *or* data content hashes — never a mix. The receiver determines which by computing this chunk's `remaining_depth` from the tree-shape rule (see Tree Construction below): if `remaining_depth == 0`, slots are 32-byte data content hashes; if `remaining_depth > 0`, slots are 32-byte child index chunk public keys.

### Root index chunk

```
Offset  Size   Field
0       1      Version (0x02)
1       8      Total file size in bytes (u64 LE)
9       4      CRC-32C (Castagnoli) of fully assembled payload (u32 LE)
13      ...    Slot payload: N × 32 bytes
```

Header overhead: 13 bytes. Maximum slot count: `(1000 - 13) / 32 = 30` slots (`N ≤ 30`).
Stored via `mutable_put`, signed by the root keypair (pickup key).

The root carries `file_size` and `crc32c` so the receiver can size the output buffer and verify integrity once reassembly completes. The root has 30 slots (vs 31 for non-root) because of the larger header.

Like non-root chunks, the root holds *either* child index pubkeys *or* data content hashes. Slot kind is derived from `file_size`: if the canonical tree-shape rule (see Tree Construction below) yields `tree_depth == 0` (i.e., the file is small enough that all data chunks fit directly in the root, `N ≤ 30`), root slots are data hashes; otherwise root slots are child index pubkeys. The empty-file case (`file_size == 0`) yields zero slots.

### Need-list record

```
Offset  Size   Field
0       1      Version (0x02)
1       2      count (u16 LE, number of NeedEntry records that follow)
3       N×8    NeedEntry × count
```

Each `NeedEntry` is 8 bytes:

```
Offset  Size   Field
0       4      start (u32 LE, inclusive data chunk index in DFS order)
4       4      end   (u32 LE, exclusive)
```

Total record size ≤ 1000 bytes. With a 3-byte header, the record can carry up to 124 entries. An entry must satisfy `start < end ≤ ceil(file_size / 998)`.

An empty record value (zero bytes, no version byte) is the receiver-done sentinel.

Decoders MUST reject any record whose first byte is non-zero but not `0x02`, whose declared count does not match the trailing byte length, or whose entries violate `start < end`.

## Topics & Records

- **Pickup key**: the public key of the root keypair, `KeyPair::from_seed(root_seed).public_key`. The root index record is the mutable record stored at this public key.
- **Non-root index records**: stored as mutable records at the public key of `derive_index_keypair(root_seed, i)` for `i ∈ [0, 2³²−1]`. The sender numbers index chunks 0, 1, 2, … in any consistent order (canonical: bottom-up build order). Tree position is *not* encoded in the keypair index; the receiver learns each chunk's pubkey from its parent's slot.
- **Data chunks**: stored as immutable records, addressed by `discovery_key(encoded_chunk)`. Self-verifying on every fetch.
- **Need topic**: `discovery_key(root_pk || b"need")`. Receivers announce on this topic and store need-list records under their own ephemeral keypair.
- **Ack topic**: `discovery_key(root_pk || b"ack")`. Receivers announce on this topic with an ephemeral keypair and no payload.

## Key Derivation

```
root_seed:        32 bytes (random or discovery_key(passphrase))
root_keypair:     KeyPair::from_seed(root_seed)                                  // root index chunk
salt:             root_seed[0]                                                    // u8
index_keypair[i]: KeyPair::from_seed(discovery_key(root_seed || b"idx" || i_le)) // i ∈ [0, 2³²−1]
```

`i_le` is `i` encoded as 4 bytes little-endian.

The 3-byte ASCII domain separator `b"idx"` prevents key collisions with other derivations from the same root seed. The pickup key is the root public key. The receiver never learns `root_seed`, so it cannot derive any private key in the index tree and cannot forge index records.

The **salt** is a per-deaddrop byte taken from the root seed. It is included in every data chunk's header so that two unrelated deaddrops storing identical file content end up at distinct DHT addresses (~256× isolation, sufficient given that content variation already dominates collision probability). The salt is deterministic across refresh cycles, so a refreshing sender always re-publishes to the same address. The receiver does not need to know the salt independently; it lives in the chunk bytes and is included automatically when the receiver hashes the returned chunk to verify content addressing.

Data chunks have no derived keypair — they are addressed solely by content hash. Anyone in possession of a data chunk's content hash can fetch the chunk and verify it; the DHT validates `discovery_key(value) == target` on every `immutable_get` response.

## Tree Construction & Reassembly Order

### Tree Shape (normative)

The shape of the index tree is fully determined by `file_size`. Both senders and receivers compute it deterministically:

```
N = ceil(file_size / 998)                     // total data chunk count

canonical_depth(N):
    if N == 0:        return 0
    if N <= 30:       return 0
    layer_count = ceil(N / 31)
    depth = 1
    while layer_count > 30:
        layer_count = ceil(layer_count / 31)
        depth += 1
    return depth

tree_depth = canonical_depth(N)
```

The wire format encodes neither `N` nor `tree_depth` directly; both are derived from `file_size` via this formula. There is no per-chunk slot-kind marker.

Senders MUST produce the canonical tree shape. Specifically:

1. If `N == 0`: root has zero slots.
2. If `N ≤ 30`: root carries `N` data content hashes directly (no non-root index chunks exist).
3. Otherwise: pack data hashes 31-at-a-time into leaf-index chunks; pack each layer's pubkeys 31-at-a-time into the next layer up; repeat until the top layer has ≤ 30 chunks; the root holds those top-layer pubkeys directly.

This procedure is uniquely defined for every value of `N`. There is no encoding for any other tree shape — alternative constructions (mixed slot kinds in a single chunk, deeper-than-canonical trees, pre-canonical-edge filling tricks) are not expressible in the v2 wire format.

### Reassembly Order (normative)

The DFS reassembly rule defines the file-byte order of data chunks across the tree. For each index chunk, the receiver consults the chunk's `remaining_depth` (root: `tree_depth`; child of any index chunk: `parent_remaining_depth - 1`):

> If `remaining_depth == 0`, the chunk's slots are data content hashes; emit them in slot order at the file positions assigned to this chunk by its parent.
> If `remaining_depth > 0`, the chunk's slots are child index pubkeys; recurse into each child in slot order, assigning each child a contiguous file-position range sized by that subtree's data-chunk count.

The slot kind is therefore unambiguous from tree position; the receiver never needs to inspect chunk content to disambiguate.

This rule is canonical: receivers and senders MUST produce identical file-order indices for the same tree structure derived from the same `file_size`.

#### Worked example

Consider a 70-data-chunk file (`d_0` through `d_69`).

- `N = 70`, so `tree_depth = 1` (since 30 < N ≤ 930).
- Pack 70 data hashes 31-at-a-time into leaf-index chunks: `leaf_0` (31 hashes for `d_0..d_30`), `leaf_1` (31 hashes for `d_31..d_61`), `leaf_2` (8 hashes for `d_62..d_69`).
- 3 ≤ 30, so the root holds the three leaf pubkeys directly.

```
                              root
                       (3 child index pubkeys,
                       remaining_depth = 1)
                       /        |        \
                      /         |         \
                  leaf_0     leaf_1     leaf_2
                  (31 data   (31 data   (8 data
                   hashes,    hashes,    hashes,
                   r_d=0)     r_d=0)     r_d=0)
                d_0..d_30  d_31..d_61  d_62..d_69
```

Applying the DFS rule from the root:

1. At **root**: `remaining_depth = 1`, so slots are child index pubkeys. Recurse into each in slot order.
2. Recurse into **leaf_0**: `remaining_depth = 0`, so slots are data hashes. Emit `d_0..d_30` at file positions `0..30`.
3. Recurse into **leaf_1**: `remaining_depth = 0`. Emit `d_31..d_61` at file positions `31..61`.
4. Recurse into **leaf_2**: `remaining_depth = 0`. Emit `d_62..d_69` at file positions `62..69`.

Final file-byte order: `d_0` through `d_69` at file positions `0..69`. Total chunks: 4 index (`root` plus 3 leaves) plus 70 data = 74 chunks. Critical-path RTT: 4 (root → 3 leaves → 70 data).

In a deeper tree (e.g., a 1 GB file with `tree_depth = 4`), the rule recurses uniformly: every internal node has `remaining_depth > 0` and just visits its child index pubkeys in slot order; every leaf-index node has `remaining_depth = 0` and emits its data hashes in slot order at the position assigned by its parent. The receiver never inspects a chunk's content to determine whether it is a leaf — the answer is always derivable from tree position.

### Sizing Math

At 998 bytes per data chunk, the canonical algorithm yields the following capacities:

| Tree depth | Max data chunks | Max file size | Critical-path RTT |
|---:|---:|---:|---:|
| 0 | 30 | 29.94 KB | 2 |
| 1 | 930 | 928.1 KB | 3 |
| 2 | 28,830 | 28.2 MB | 4 |
| 3 | 893,730 | 851.4 MB | 5 |
| 4 | 27,705,630 | 25.78 GB | 6 |
| 5 | 858,874,530 | 798.13 GB | 7 |
| 6 | 26,625,110,430 | 24.2 TB | 8 |

Depth `d` capacity is `30 × 31^d` data chunks (root has 30 slots; each non-root has 31).

### Worked example: 1 GB

1,073,741,824 bytes / 998 bytes per chunk = 1,075,894 data chunks (last chunk holds 610 bytes).

| Layer | Role | Count |
|-------|------|-------|
| 4 | leaf-index (31 data hashes each, last partial) | 34,707 |
| 3 | index-of-leaves (31 leaf pubkeys each) | 1,120 |
| 2 | index-of-L3 (31 L3 pubkeys each) | 37 |
| 1 | index-of-L2 (31 L2 pubkeys each) | 2 |
| 0 | root (2 L1 pubkeys) | 1 |

Total non-root index chunks: 35,866. Plus root = **35,867 index chunks total** (~3.33% overhead).

Critical path: root fetch (1) → 2 L1 fetches in parallel (1) → 37 L2 fetches in parallel (1) → 1,120 L3 fetches in parallel (1) → 34,707 leaf fetches in parallel (1) → 1,075,894 data fetches in parallel (1) = **6 round trips total**.

Compare v2-original (unpublished, linked-list index): roughly 35,863 sequential `mutable_get` round trips. v2 collapses that to 6 — a ~6,000× improvement on the critical path.

## Fetch Protocol (Receiver)

A receiver begins with the pickup key (the root public key) and proceeds:

1. Has the pickup key.
2. `mutable_get(root_pubkey, 0)` retrieves the root index record. Parse it to learn `file_size` and `crc32c`. Compute `N = ceil(file_size / 998)` and `tree_depth = canonical_depth(N)`. Compute the slot count from chunk length (`(chunk_len - 13) / 32`); slot kind is derived from `tree_depth` (data hashes if `tree_depth == 0`, child index pubkeys otherwise).
3. Compute `expected_data_count = ceil(file_size / 998)`. Validate that the root's data hashes plus all subtree contributions will cover `[0, expected_data_count)`.
4. **Schedule fetches** for every pubkey/hash discovered so far through a shared concurrency budget (default: 64 permits):
   - Each child index pubkey → `mutable_get(child_pk, 0)`.
   - Each data hash → `immutable_get(hash)`.
5. **As each index chunk arrives**, parse it. Compute its slot count from chunk length (`(chunk_len - 1) / 32` for non-root chunks). Determine slot kind from the chunk's `remaining_depth` (which the parent knows because it placed this chunk's pubkey in the appropriate slot position): if `remaining_depth == 0`, slots are data hashes; otherwise slots are child index pubkeys. Assign DFS file-order positions to children per the Reassembly Order rule, and schedule fetches for newly discovered pubkeys/hashes.
6. **As each data chunk arrives**, verify its content addressing (the DHT validates `discovery_key(value) == target` automatically), strip the 2-byte header, and place the payload at its DFS-order file offset.
7. **Loop detection**: track every index chunk pubkey already visited. If the same pubkey appears more than once, abort.
8. **Completion**: when all `expected_data_count` data chunks have been received, compute CRC-32C of the reassembled payload. Abort if it does not match the stored CRC.
9. Write the output (file or stdout); see *Output Strategies* below.
10. Optionally announce on the ack topic (see Pickup Acknowledgement Channel).
11. Publish an empty need-list record to clear any in-flight requests (`mutable_put(&need_kp, &[], seq)`).

Receivers MUST: reject any chunk whose first byte is not `0x02`; detect index-tree loops; verify CRC-32C; abort on size mismatch; abort on a data chunk whose content does not hash to its expected address.

Receivers SHOULD (implementation choices): pipeline index fetches with data fetches under a shared concurrency budget; use frontier-probing retry on per-chunk timeout; publish need-list records on no-progress cycles; choose an output strategy appropriate to the destination.

### Output Strategies (informative)

The wire format does not dictate how the receiver buffers reassembled bytes. Three strategies the reference implementation uses:

- **`--output <path>`**: open the output file, preallocate it to `file_size` (sparse if the filesystem supports it), `mmap` it as `MmapMut`, and write each data chunk directly to its DFS-order byte offset as it arrives. No reassembly buffer in user-space RAM. Finalize with `msync` and an atomic temp+rename.
- **stdout**: stream. The receiver prioritizes left-DFS index fetches (root → leftmost child → ... → leftmost leaf), then fans out left-to-right. It maintains an `emit_pos: u32` cursor; when data chunk `i == emit_pos` arrives, the receiver emits its bytes to stdout, advances `emit_pos`, and drains any contiguous successors held in a small reorder buffer. Reorder buffer size is bounded by `PARALLEL_FETCH_CAP × 998 B` (≈ 64 KB at the default cap), independent of file size. CRC-32C is computed streaming; mismatch is reported at end, but bytes already written are downstream. Per-chunk content addressing protects against mid-stream corruption.
- **fall-through (in-RAM)**: a conformant receiver MAY accumulate chunks in memory and write at the end. This is appropriate for small payloads but pays linear RAM cost in `file_size`.

The day-1 reference implementation uses mmap for `--output` and streaming for stdout; in-RAM is never the default.

## Write Protocol (Sender)

A sender begins with input bytes and a root seed (random or derived from a passphrase):

1. Read the input via `mmap` (file) or `read_to_end` (stdin). Validate that `file_size` does not exceed the configured soft cap (see Practical Limits).
2. Compute CRC-32C of the entire payload (streaming over chunks if mmap'd).
3. Compute `salt = root_seed[0]`.
4. Split the payload into chunks of at most 998 bytes. Encode each chunk as `[0x02][salt][payload_bytes]`. Compute `discovery_key(encoded)` for each — that hash is its DHT address.
5. **Build the index tree** using the canonical bottom-up algorithm (see Tree Shape; this construction is normative — no other tree shape is valid v2). Number index chunks `0, 1, 2, …` in bottom-up build order; derive each non-root index keypair as `KeyPair::from_seed(discovery_key(root_seed || b"idx" || i_le))`. Encode each index chunk as `[0x02][slot bytes]`.
6. **Publish with the root last**: every non-root chunk (all data chunks via `immutable_put` and all index chunks at every layer via `mutable_put`) is published in any order through a shared concurrency budget. Once they have all completed, the root is published. The root is the only discoverable entry point — until it exists, no receiver can derive any other pubkey in the drop, so a partial publish is not discoverable. Senders MAY interleave data and index publishes to balance progress reporting and concurrency utilization; they MUST NOT publish the root before every other chunk has been written.
7. Print the pickup key (the root public key, 64-character hex) to stdout.
8. Enter a refresh loop, monitoring the ack topic and the need topic until terminated.

Senders MUST: produce the canonical tree shape implied by `file_size`; sign each index record with its associated derived keypair; use a monotonically increasing `seq` (the current Unix timestamp is the canonical choice) on every `mutable_put`; publish the root last on initial publish; include the per-deaddrop salt in every data chunk header.

Senders SHOULD (implementation choices): pipeline publishes through a shared concurrency budget; honor rate limits (AIMD); poll the ack topic; service need-list requests; use mmap on the input file when reading from disk.

## Refresh Protocol

DHT records expire after roughly 20 minutes on the public network. The sender keeps the dead drop alive by republishing:

- **Index chunks** are re-published via `mutable_put` with `seq` set to the current Unix timestamp (or any monotonically increasing value). Signature uses the same per-position derived keypair.
- **Data chunks** are re-published via `immutable_put` with the same encoded bytes. Immutable records have no `seq`; re-storage refreshes the DHT TTL.
- The refresh interval is implementation-defined. The reference implementation defaults to 600 seconds, well within the DHT's ~20-minute TTL.
- Refresh re-publishes the entire tree and data layer through the same concurrency budget. It is acceptable for a refresh cycle to overlap or be interrupted by a need-list response cycle.

## Need-List Feedback Channel

### Purpose

The need-list channel lets a receiver tell the sender which chunk ranges are still missing, so the sender can prioritize re-publishing them. v2 expresses missing pieces as ranges of *data chunk indices* in DFS order; the sender translates these into the index nodes that must be re-published to make the data chunks reachable.

### Topic

`need_topic = discovery_key(root_pk || b"need")`.

### Receiver behavior

- Once per session, generate an ephemeral `need_kp = KeyPair::generate()`.
- Announce on the need topic: `announce(need_topic, &need_kp, &[])`.
- When stuck on missing chunks for longer than a no-progress threshold: encode the missing data-chunk-index ranges as a need-list record and publish via `mutable_put(&need_kp, &encoded, seq)`, with `seq` strictly greater than any previous value used for `need_kp`.
- The receiver MAY post need-list records at any time after the root index has been fetched; it is not required to have completed (or attempted) the full tree fetch first.
- On exit (success or failure): publish an empty record via `mutable_put(&need_kp, &[], seq+1)`. The empty payload signals "done".

The receiver computes missing data-chunk-index ranges from its `expected_data_count` (derived from `file_size`) minus the set of file-order positions it has successfully fetched. Coalesce contiguous missing positions into `[start, end)` ranges before encoding.

### Sender behavior (normative)

For each non-empty need-list record received from a peer, for each `NeedEntry { start, end }`, the sender MUST republish:

1. Every data chunk in the range `[start, end)`.
2. Every leaf-index chunk that contains any data hash in `[start, end)`.
3. Every ancestor of those leaf-index chunks, up to (but not including) the root.

The root is re-published on the regular refresh tick, not on need-list response. This avoids thrashing the most-watched record on every receiver request.

Senders MUST NOT attempt to elide any of the three categories above based on inference about receiver state. Conformant senders republish the full path on every need-list entry.

### Validation requirements (both sides)

- An empty record value is an empty list and the receiver-done sentinel.
- A non-empty record's first byte MUST be `0x02`.
- The 16-bit `count` field MUST equal `(value_len - 3) / 8`. Any mismatch → reject.
- Each entry MUST satisfy `start < end ≤ expected_data_count`. Any violation → reject the entire record.
- Truncated records → reject.

## Pickup Acknowledgement Channel

### Purpose

The ack channel allows senders to detect that one or more pickups have occurred, enabling early-exit policies.

### Topic

`ack_topic = discovery_key(root_pk || b"ack")`.

### Receiver behavior

On successful reassembly, CRC verification, and output write: generate an ephemeral `ack_kp = KeyPair::generate()` and call `announce(ack_topic, &ack_kp, &[])` — announce only, no payload. Receivers MAY suppress this announcement (e.g., a `--no-ack` flag).

### Sender behavior

Periodically `lookup(ack_topic)` and count unique announcer public keys via a set. The sender may exit early once the count reaches a target threshold (`--max-pickups N`).

### Soundness note

The ack channel does NOT prove successful reassembly — only that some peer announced. Treat ack counts as an optimization signal, never as a correctness check.

## Conformance Requirements

### Required (wire protocol invariants)

- All v2 frame and record types use version byte `0x02` as the first byte.
- Index chunks are stored via `mutable_put`, signed by their position-derived keypair.
- Data chunks are stored via `immutable_put`; their address is `discovery_key(encoded_chunk)`, where the encoded chunk includes the 1-byte salt prefix.
- Root index header layout: `[0x02][file_size_u64_le][crc_u32_le][N×32_byte_slots]`, with `N ≤ 30`.
- Non-root index header layout: `[0x02][N×32_byte_slots]`, with `N ≤ 31`.
- Slot kind (data hash vs child index pubkey) is derived from the chunk's `remaining_depth`, computed from `file_size` via the canonical tree-shape rule. There is no per-chunk slot-kind marker.
- Senders MUST produce the canonical tree shape defined by `canonical_depth(N)`. No alternative tree shapes are expressible in the v2 wire format.
- Data chunk header layout: `[0x02][salt_u8][payload]`, with payload ≤ 998 bytes.
- The salt byte is `root_seed[0]` and is constant across refresh cycles.
- Index keypair derivation uses 4-byte little-endian `i` with the `b"idx"` domain separator.
- The DFS reassembly rule (data slots first in slot order, then index slots recursively in slot order) is canonical and MUST be applied identically by senders and receivers.
- Receivers MUST detect index-tree loops, validate version bytes on every parsed record, verify CRC-32C of the reassembled payload, and abort on size mismatch.
- Senders MUST sign every `mutable_put` with the keypair associated with that record's position, use a monotonically increasing `seq`, and publish the root last on initial publish.
- Need-list records MUST be formatted as defined in the Frame Formats section. Empty values are the receiver-done sentinel.

### Optional (implementation choices, documented for context)

- BFS scheduling of index fetches under a shared concurrency budget.
- Left-DFS prioritization for streaming output.
- AIMD-controlled rate limiting on the sender side.
- Frontier-probing retry on missing data chunks.
- mmap-based input on the sender side.
- mmap-based preallocated output on the receiver side.
- Streaming stdout output via emit-as-contiguous bookkeeping.
- Ack channel announcement on successful pickup (receivers MAY suppress).
- Sender polling cadence for the need and ack topics.

## Practical Limits

- Data chunk payload: 998 bytes.
- Slots per index chunk: 30 (root) / 31 (non-root). Trailing chunks of a partially filled level may have fewer slots; the slot count of any chunk is `(chunk_len - header_size) / 32`.
- Index-keypair derivation index: u32 (up to 2³² − 1 non-root index chunks per deaddrop).
- Format maximum file size: bounded only by `file_size` (u64) — no protocol cap.
- Reference implementation soft cap: tree depth ≤ 4 (≈ 25.78 GB at 998 B/chunk). Override available via flag (`--allow-deep` or equivalent) on the sender. The receiver imposes no depth cap; it handles any depth that fits in u32 keypair indices.
- DHT record TTL on the public network: ~20 minutes; the refresh interval should be ≤ TTL/2.
- Default parallel fetch cap: 64 permits, shared between index and data fetches.
- Reorder buffer for streaming stdout: bounded by `parallel_fetch_cap × 998 B` (~64 KB at default).
- An empty input file is valid: `file_size = 0`, `crc = 0`, root has zero slots (13-byte chunk).

## Security Properties

- The pickup key is the root public key — a read-only capability for the index tree root.
- Each index chunk is signed by a unique keypair derived from `root_seed` via the `b"idx"` domain separator. A receiver, knowing only the pickup key, cannot derive any private key and cannot forge index records.
- Each data chunk is content-addressed: the DHT validates `discovery_key(value) == target` on every `immutable_get` response, so a malicious DHT node cannot return forged data without being detected.
- The per-deaddrop salt provides DHT address-space isolation — two unrelated deaddrops with identical content store at distinct addresses. The salt is not a secret; its purpose is to avoid lifecycle-coupling with strangers' chunks at shared addresses.
- DHT nodes can read plaintext payloads. Encrypt before dropping if confidentiality is required.
- Data chunk content addresses are opaque to anyone who has not walked at least part of the index tree.
- The need-list channel uses an ephemeral receiver keypair: only that receiver can write to or clear its own need list.
- The ack channel is announce-only and unauthenticated; ack counts are a heuristic, not a correctness signal.
- The salt is derived from `root_seed[0]` and is therefore not independently secret if the seed is known. It is not intended to be — its only role is address-space namespacing.

## Comparison

### v2 vs prior protocols

| Property | v1 | v2 — earlier linked-list draft (unpublished) | v2 — current spec (ships as wire byte 0x02) |
|----------|----|--------------------------------|------------------------------------------|
| Data payload per chunk | 961 (root) / 967 (non-root) | 999 | **998** |
| Data chunk header | 39 / 33 B | 1 B | **2 B (version + salt)** |
| Index chunk header (root / non-root) | n/a | 41 / 33 B | **13 / 1 B** |
| Per-chunk slot-kind marker | n/a | implicit (chain) | **none — derived from tree position** |
| Data layer mutability | Mutable signed | Immutable, content-addressed | Immutable, content-addressed |
| Index layer shape | Linked list (data carries pointers) | Linked list of index chunks | **Tree of index chunks** |
| Address-space isolation | per-chunk derived keypair | none (raw content hash) | **per-deaddrop salt** |
| Receiver fetch shape | Fully sequential | Index sequential + data parallel | **Index BFS + data parallel** |
| Index walk RTT (1 GB) | ~1,000,000 sequential | ~35,800 sequential | **6 round trips total** |
| Need-list format | none | Index-range + data-range entries | **Data-chunk-index ranges only (8 B/entry)** |
| File size field | u16 chunk count | u32 bytes | **u64 bytes** |
| Format max file size | ~60 MB | ~1.83 GB | **u64 (no protocol cap)** |
| Reference soft cap | n/a | n/a | **depth 4 (~25.78 GB)** |
| Pickup key | root public key (hex) | root public key (hex) | root public key (hex) |
| Streaming output | not supported | not supported | **wire-compatible; reference impl streams to stdout** |

### RTT improvement across file sizes

| File size | Data chunks | v2 tree depth | v2 RTT | v2-linked-list RTT |
|----------|---:|---:|---:|---:|
| 100 KB | 103 | 1 | 3 | 5 |
| 1 MB | 1,051 | 2 | 4 | 37 |
| 10 MB | 10,507 | 2 | 4 | 352 |
| 100 MB | 105,068 | 3 | 5 | 3,504 |
| 1 GB | 1,075,894 | 4 | 6 | 35,865 |
| 10 GB | 10,758,937 | 4 | 6 | 358,633 † |
| 100 GB | 107,589,362 | 5 | 7 | 3,586,314 † |

† Architectural RTT only. v2-original used a `u32` `file_size` field, which caps at 4 GB; 10 GB and 100 GB rows are not representable in v2-original's wire format and are shown for architectural comparison only.

v2 RTT = `tree_depth + 2` (root fetch, then `tree_depth` sequential index waves, then one parallel data wave). v2-linked-list RTT = `1 + index_chain_length`, where `index_chain_length = 1 + ceil((N - 29) / 30)` (root with 29 hashes, non-root with 30).

## Migration Notes

- The version byte `0x02` distinguishes v2 frames from v1 (`0x01`) at the root chunk and all downstream records.
- Wire byte `0x02` is the canonical v2 byte. The earlier linked-list draft of v2 was never published to the public DHT, so there is no migration concern — no records produced by it exist anywhere to interop with.
- The receiver auto-detects v1 vs v2 by reading the version byte of the root chunk. No flag is required to read either format.
- The pickup key format is unchanged from v1 (a 64-character hex root public key).
- Passphrase mode works identically: `passphrase → discovery_key(passphrase) → root_seed → root_keypair → root_pubkey`.
- Implementations of the earlier linked-list v2 draft must be updated to the current spec; the root header layout, index header layout, and need-list encoding are all incompatible.

## Resolved Decisions

- **Tree shape**: fully determined by `file_size`. Every chunk's slots are either all data content hashes (leaf) or all child index pubkeys (non-leaf); the wire format does not encode which, and the receiver derives slot kind from the chunk's tree position via the `canonical_depth` rule. Mixed slot kinds within a single chunk are not expressible in v2.
- **Canonical algorithm is normative**: senders MUST produce exactly the bottom-up greedy tree shape implied by `file_size`. No alternative constructions are valid v2.
- **N ≤ 30 special case**: the root holds data hashes directly, bypassing the leaf-index level entirely. Saves one round trip on small files. (This is just `tree_depth == 0` in the `canonical_depth` formula; not a separate codepath in the receiver.)
- **No inline payload**: even for files small enough to fit in the root chunk's slot region, files always go through the data layer as a separate `immutable_put`. Single canonical encoding per file size; 2 RTT minimum.
- **Salt byte**: `root_seed[0]`. Deterministic across refreshes (preserving idempotent re-publish). Provides ~256× DHT address isolation between unrelated deaddrops with identical content.
- **Need-list format**: `(u32 start, u32 end)` data-chunk-index ranges, 8 bytes per entry. No separate Index/Data variants — all reconciliation is expressed in terms of data chunk file-order indices, with the sender translating to required index chunks.
- **Need-list response policy**: senders MUST republish the full path (data chunks + leaf-index + every ancestor up to root). Sub-tree-aware republish elision is explicitly disallowed — every need-list entry produces a full-path response.
- **File-size field**: u64 LE in the root header. No protocol cap; sender soft cap configurable.
- **Index-keypair derivation index width**: u32 LE (up from v2-original's u16). Supports trees deep enough for u32 file-order chunk indices.
- **Reassembly order**: implicit DFS (data slots first, then index slots recursively, in slot order). No per-chunk file-order index in the data chunk header.
- **CRC scope**: CRC-32C over the reassembled file bytes (not over encoded chunks). Matches v2-original.
- **Initial publish ordering**: every non-root chunk is published in any order through a shared concurrency budget; the root is published last. The root-last requirement (and only the root-last requirement) ensures the pickup key is not discoverable until every chunk it transitively references has been written. The reference sender interleaves data and index puts so that progress is observable on both counters from the start of the publish.
- **Refresh interval default**: 600 seconds (well under DHT TTL/2).
- **Concurrency cap default**: 64 permits, shared between index and data fetches on both sides.
- **mmap I/O**: required for the reference implementation. Sender mmaps input files (`memmap2::Mmap`); receiver mmaps preallocated output files (`memmap2::MmapMut`) for `--output`. Stdin (sender) is buffered in RAM; small payload usage is implicit. Stdout (receiver) uses streaming.
- **Streaming stdout**: receiver prioritizes left-DFS index fetches and emits data chunks as they arrive in file-order. Reorder buffer bounded by `PARALLEL_FETCH_CAP × 998 B`. CRC computed streaming; mismatch reported at end (already-emitted bytes are downstream).
- **Sender soft cap default**: tree depth ≤ 4 (~25.78 GB). Override flag for deeper trees. Receiver enforces no cap.
- **No streaming for `--output`**: the file mmap path writes chunks to their final byte offsets as they arrive but does not commit until reassembly completes (atomic temp+rename). CRC is verified before the final rename.

## Open Questions

None blocking implementation. Possible future iterations:

- A `--no-ack` mode is wire-compatible (receiver simply does not announce). Spec requires no change.
- A future v4 could trade the per-deaddrop salt for a per-chunk derivable address (using the existing index-keypair derivation scheme) to enable receiver-side speculative prefetch of data chunks before their parent index arrives. This is a wire-format change and would bump the version byte.
