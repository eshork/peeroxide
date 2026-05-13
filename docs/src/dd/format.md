# Dead Drop Wire Format

The `dd` command supports two versioned wire formats for DHT records. All multi-byte integers are encoded in **little-endian** (LE) byte order.

## Version 1 Wire Format

V1 records are limited to 1000 bytes total and form a linear linked list of mutable records.

### V1 Constants

- `MAX_CHUNKS`: 65,535
- `MAX_PAYLOAD`: 1,000 (total record limit)
- `ROOT_HEADER_SIZE`: 39
- `NON_ROOT_HEADER_SIZE`: 33
- `ROOT_PAYLOAD_MAX`: 961
- `NON_ROOT_PAYLOAD_MAX`: 967
- `VERSION`: `0x01`

### V1 Layouts

**Root Chunk (V1)**

```text
[ver: 1][total_chunks: 2 LE][crc32c: 4 LE][next_pk: 32][payload: up to 961]
```

**Non-root Chunk (V1)**

```text
[ver: 1][next_pk: 32][payload: up to 967]
```

## Version 2 Wire Format

V2 records use a tree structure. Data chunks are stored in immutable records, while index and root chunks are stored in mutable records.

### V2 Constants

- `VERSION`: `0x02`
- `MAX_CHUNK_SIZE`: 1,000
- `DATA_HEADER_SIZE`: 2
- `DATA_PAYLOAD_MAX`: 998
- `NON_ROOT_INDEX_HEADER_SIZE`: 1
- `NON_ROOT_INDEX_SLOT_CAP`: 31
- `ROOT_INDEX_HEADER_SIZE`: 13
- `ROOT_INDEX_SLOT_CAP`: 30
- `NEED_LIST_HEADER_SIZE`: 3
- `NEED_ENTRY_SIZE`: 8
- `NEED_LIST_ENTRY_CAP`: 124

### V2 Tree Structure

The tree is constructed bottom-up. Leaf layers pack 31 data hashes per index chunk. Higher layers pack 31 index pubkeys per chunk. The root holds the top-layer keys directly.

| Depth | Max Data Chunks | Capacity (approx) |
|-------|-----------------|-------------------|
| 0 | 30 | 29 KB |
| 1 | 930 | 928 KB |
| 2 | 28,830 | 28 MB |
| 3 | 893,730 | 891 MB |
| 4 | 27,705,630 | 27 GB |

**Note:** The implementation enforces a `SOFT_DEPTH_CAP` of 4.

### V2 Layouts

**Data Chunk (V2)**

Stored via `immutable_put`. The salt is reserved for randomization but currently fixed at `0x00`.

```text
[0x02][salt: 0x00][payload: up to 998]
```

**Non-root Index Chunk (V2)**

Stored via `mutable_put`. Contains 32-byte slots (either data hashes or child index pubkeys).

```text
[0x02][slots: 31 x 32]
```

**Root Index Chunk (V2)**

The entry point. Contains file metadata and top-level slots.

```text
[0x02][file_size: 8 LE][crc32c: 4 LE][slots: 30 x 32]
```

**Need-list Record (V2)**

Published by the receiver on the need topic to request missing data.

```text
[0x02][count: 2 LE][entries: count x {start: 4 LE, end: 4 LE}]
```

Each entry is a half-open range `[start, end)` of data-chunk indices in the canonical DFS file order (chunk 0 is the first chunk of the file, chunk 1 is the next, etc.). The sender consults the need-list and republishes every data chunk in any listed range, plus the full index-tree path required to make those data chunks reachable.

When the receiver has no missing chunks, it publishes a "receiver done" sentinel: a raw empty byte string at the need topic. The decoder treats a zero-byte value as the sentinel (it is not the same as the encoded need-list with `count = 0`).

### Salt Situation

While the V2 format reserves a byte for a per-deaddrop salt to randomize data chunk addresses, the current implementation enforces `salt(...) -> 0x00`. All V2 data chunk headers are currently prefixed with `[0x02][0x00]`.
