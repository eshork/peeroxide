# Dead Drop Wire Format

The dead drop uses a versioned binary format for its DHT records. Each record consists of a header followed by the payload.

## Constants

- `MAX_PAYLOAD`: 1000 bytes (total record size)
- `VERSION`: `0x01`
- `ROOT_HEADER_SIZE`: 39 bytes
- `NON_ROOT_HEADER_SIZE`: 33 bytes

## Root Chunk (v1)

The root chunk is the entry point of the dead drop. Its public key is the "pickup key".

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 1 | Version | Set to `0x01` |
| 1 | 2 | Total Chunks | Number of chunks in the chain (u16 LE) |
| 3 | 4 | CRC-32C | Checksum of the full reassembled payload |
| 7 | 32 | Next PK | Public key of the next chunk (32 zeros if single chunk) |
| 39 | ... | Payload | Data bytes (up to 961 bytes) |

## Continuation Chunk (v1)

All subsequent chunks use a smaller header.

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 1 | Version | Set to `0x01` |
| 1 | 32 | Next PK | Public key of the next chunk (32 zeros if last chunk) |
| 33 | ... | Payload | Data bytes (up to 967 bytes) |

## Implementation Details

### Byte Order
All multi-byte integers (Total Chunks, CRC-32C) are encoded in **little-endian** byte order.

### Integrity Verification
The CRC-32C checksum uses the Castagnoli polynomial. It is computed over the *entire* reassembled payload, not per-chunk. Receivers must fetch all chunks and reassemble them before verifying the checksum.

### Chain Termination
The chain is considered terminated when a chunk (root or continuation) contains a `Next PK` field consisting of 32 null bytes (`0x00`).

