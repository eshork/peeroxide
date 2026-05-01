# Future Direction (Not Yet Implemented)

**Note: The following features and protocol changes describe Deaddrop v2 and are not yet implemented.**

The current Deaddrop v1 protocol uses a single linked-list of chunks. While functional, this requires sequential fetching where the receiver must download each chunk to discover the address of the next. For large files, this leads to high latency due to sequential round-trips.

## Deaddrop v2: Two-Chain Storage Protocol

Deaddrop v2 introduces a "two-chain" architecture to enable parallel data fetching while preserving anonymity and read-only pickup semantics.

### Index Chain vs. Data Chain

Instead of a single list, the protocol separates metadata and pointers from the actual data:

- **Index Chain:** A small linked-list of records containing public keys (pointers) to data chunks.
- **Data Chain:** Independently addressable data chunks stored at random DHT coordinates.

```text
Index chain (sequential fetch, small):
  [root idx] → [idx 1] → [idx 2] → ... → [idx K]
      │            │           │
      ▼            ▼           ▼
Data chain (parallel fetch, bulk):
  [d0..d29]    [d30..d59]   [d60..d89] ...
```

### Benefits of v2

| Property | v1 (Sequential) | v2 (Parallel) |
|----------|-----------------|---------------|
| **Fetch Pattern** | Entirely sequential | Index sequential + Data parallel |
| **Overhead** | ~3.4-3.9% | ~0.1% |
| **Max File Size** | ~60 MB | ~1.9 GB |
| **1MB Fetch Time** | ~1000 round-trips (15-50 min) | ~34 index + ~1000 parallel (~1 min) |

### Key Derivation in v2

Keypairs are derived deterministically from the `root_seed` using domain separation:
- `index_keypair[i] = blake2b(root_seed || "idx" || i)`
- `data_keypair[i] = blake2b(root_seed || "dat" || i)`

This ensures the sender can refresh the entire structure from a single seed while preventing address correlation between the index and data chains for third parties.

### Frame Formats (v2)

- **Data Chunk (0x02):** 1-byte version tag + up to 999 bytes of raw payload.
- **Root Index Chunk (0x02):** Metadata (size, CRC), `next` index pointer, and up to 29 data chunk pointers.
- **Non-Root Index Chunk (0x02):** `next` index pointer and up to 30 data chunk pointers.

