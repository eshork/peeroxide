# Deaddrop v2: Two-Chain Storage Protocol

Future revision of the deaddrop frame format. Supersedes the v1 single linked-list design with a two-chain architecture that enables parallel data fetch while preserving read-only pickup semantics.

## Motivation

v1 uses a single linked-list of chunks. The receiver must fetch sequentially (each chunk contains the `next` pointer). For a 100KB file (~107 chunks), this means ~107 round-trips taking 2-5 minutes. The format also wastes bytes on redundant fields (next pointer in every frame).

v2 separates concerns: a small **index chain** (linked-list of pointer records) and a large **data chain** (independently addressable chunks fetched in parallel).

## Architecture

```
Index chain (sequential fetch, small):
  [root idx] → [idx 1] → [idx 2] → ... → [idx K]  (next=zeros)
      │            │           │
      ▼            ▼           ▼
Data chain (parallel fetch, bulk):
  [d0..d29]    [d30..d59]   [d60..d89] ...
```

- **Index chain:** Linked-list of records containing data chunk public keys (pointers). Sequential fetch — but each record holds ~30 pointers, so the index is ~30× shorter than the data.
- **Data chain:** Independent records at random DHT coordinates. Once the receiver knows all pubkeys (from the index), it fetches all data chunks in parallel.

## Frame Formats

### Data chunk (version 0x02)

```
Offset  Size   Field
0       1      Version (0x02)
1       ...    Payload (raw file bytes, up to 999 bytes)
```

Header overhead: **1 byte.** Max payload: **999 bytes.**

Data chunks have no pointers, no metadata, no index. Just a version tag and raw bytes. Their ordering is defined by their position in the index chain.

### Root index chunk (version 0x02)

```
Offset  Size   Field
0       1      Version (0x02)
1       4      Total file size in bytes (u32 LE)
5       4      CRC-32C of fully assembled payload (Castagnoli)
9       32     Next index chunk public key (32 zeros if single index chunk)
41      ...    Data chunk public keys (32 bytes each, up to 29 per root)
```

Header overhead: **41 bytes.** Remaining: 959 bytes → **29 data chunk pointers** per root index.

### Non-root index chunk (version 0x02)

```
Offset  Size   Field
0       1      Version (0x02)
1       32     Next index chunk public key (32 zeros if final index chunk)
33      ...    Data chunk public keys (32 bytes each, up to 30 per chunk)
```

Header overhead: **33 bytes.** Remaining: 967 bytes → **30 data chunk pointers** per non-root index.

## Key Derivation

Sender derives all keypairs deterministically from `root_seed` (enables refresh after restart):

```
root_keypair       = KeyPair::from_seed(root_seed)              // chunk 0 of index chain
index_keypair[i]   = KeyPair::from_seed(blake2b(root_seed || "idx" || i_as_u16_le))
data_keypair[i]    = KeyPair::from_seed(blake2b(root_seed || "dat" || i_as_u16_le))
```

- `root_seed`: 32 bytes (random or BLAKE2b of passphrase)
- Index chunk 0 uses `root_keypair` directly
- `"idx"` and `"dat"` are literal ASCII byte prefixes (domain separation)

**Pickup key = root public key** (derived from root_seed). The receiver never learns root_seed and cannot derive any private keys. Read-only capability preserved.

## Fetch Protocol (Receiver)

1. Has pickup key (root public key)
2. `mutable_get(root_pubkey, 0)` → parse root index → learn file size, CRC, first batch of data pubkeys, next index pointer
3. Walk index chain sequentially: fetch each `next` index chunk, accumulate data pubkeys
4. Once all data pubkeys collected: fire all `mutable_get` calls in parallel (batch, capped at e.g. 64 concurrent)
5. Reassemble data in index order (first pointer = first chunk of file)
6. Verify CRC-32C of assembled payload against root's stored value
7. Write output

## Write Protocol (Sender)

1. Read file, compute CRC-32C
2. Split into data chunks of ≤ 999 bytes
3. Derive all keypairs
4. Write data chunks (any order, parallel OK)
5. Build index chain with data chunk public keys (in file order)
6. Write index chain in reverse (last index chunk first, root last) — root-last = "ready" signal
7. Print root public key to stdout

Refresh: re-put all data chunks and all index chunks with `seq = current Unix timestamp`.

## Comparison to v1

| Property | v1 (single linked-list) | v2 (two-chain) |
|----------|------------------------|----------------|
| Data payload per chunk | 961 (root) / 967 (non-root) | **999** |
| Fetch pattern | All sequential | Index sequential + data **parallel** |
| Read/write separation | ✓ (pickup key = root pubkey) | ✓ (same) |
| Forgery protection | ✓ (each chunk signed by unique key) | ✓ (same) |
| Format max file size | ~60 MB (u16 chunk count) | **~1.9 GB** (65535 idx × 30 ptrs × 999 B) |
| 100KB fetch time | ~107 sequential queries (2-5 min) | ~4 index + ~107 parallel (seconds) |
| 1MB fetch time | ~1000 sequential queries (15-50 min) | ~34 index + ~1000 parallel (~1 min) |
| Overhead per data byte | 3.4-3.9% | **0.1%** |
| Complexity | Simple | Moderate (two derivation domains, two frame types) |

## Practical Limits (v2)

- **Data chunk payload:** 999 bytes
- **Pointers per index chunk:** 29 (root) / 30 (non-root)
- **Format maximum:** 65535 index chunks × ~30 pointers × 999 bytes ≈ 1.9 GB
- **Refresh is concurrent:** All puts (data + index) fire in parallel per cycle. Bottleneck is outbound bandwidth: each put commits ~1.1 KB to ~20 nodes ≈ 22 KB outbound per chunk. Refresh interval = 10 minutes (DHT record TTL = 20 min).
- **Practical ceiling:** Limited by upload bandwidth. At 1 MB/s upload with 10-min refresh: ~27,200 chunks (~27 MB). At 5 MB/s: full format max is achievable.
- **1MB example:** ~1000 data chunks + 34 index chunks = 1034 puts × 22 KB = ~22 MB outbound per cycle. Refreshes in ~22 seconds at 1 MB/s upload — trivial.

## Security Properties (unchanged from v1)

- Pickup key = root public key (read-only capability)
- Each chunk (data and index) signed by a unique keypair derived from root_seed
- Receiver cannot derive private keys → cannot forge records
- DHT nodes can read plaintext (same as v1 — encrypt before dropping for confidentiality)
- Malicious DHT nodes cannot forge chunks (signature verification)
- Data chunk locations are opaque to anyone who hasn't walked the index chain

## Migration Notes

- Version byte 0x02 distinguishes v2 frames from v1 (0x01)
- A v2-aware `pickup` client can detect the version from the root chunk and handle both formats
- `leave` would default to v2 but could support `--format v1` for compatibility during transition
- The pickup key format is unchanged (64-char hex root public key)
- Passphrase mode works identically (passphrase → blake2b → root_seed → root_keypair → root_pubkey)

## Open Questions for Implementation

- **Parallel fetch concurrency cap:** 64? 128? Depends on UDP socket limits and network conditions.
- **Index chain refresh order:** Any order is safe (data chunks are already written). Could refresh data and index in parallel.
- **Partial index walk + streaming fetch:** Could the receiver start fetching data chunks as soon as the first index record is parsed, while continuing to walk the index? This would pipeline index walking with data fetching for faster perceived latency.
- **Error handling for partial data fetch:** If 95/100 data chunks succeed but 5 timeout, should the receiver retry those 5 before aborting? How many retries?
