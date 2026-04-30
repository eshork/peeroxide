# Lookup Overview

The `lookup` command queries the Distributed Hash Table (DHT) to discover peers announcing a specific topic. It provides a way to find connection points (relay addresses and public keys) for any given service or dataset.

## Topic Resolution

Topics can be provided as either a plaintext string or a raw 64-character hexadecimal key.

- **Raw Key**: If the input is exactly 64 hex characters, it is treated as a raw 32-byte key.
- **Plaintext**: Otherwise, the input string is hashed using BLAKE2b-256 via the `discovery_key` function to derive the target topic key.

## Usage

```bash
peeroxide lookup <TOPIC> [FLAGS]
```

### Flags

| Flag | Description |
|------|-------------|
| `--with-data` | Fetch metadata stored on the DHT for each discovered peer. |
| `--json` | Output results as Newline Delimited JSON (NDJSON) to stdout. |

## Peer Discovery and Deduplication

The `lookup` command identifies unique peers by their 32-byte public key. If multiple DHT records are found for the same public key, their relay addresses are merged into a single union set, ensuring no duplicate addresses are displayed for a single peer.

Peers are displayed in the order they were first seen during the lookup process.

## Metadata Retrieval

When the `--with-data` flag is used, `peeroxide` performs a `mutable_get` for each discovered peer's public key (using `seq=0`). This process runs concurrently with a concurrency limit of 16 to ensure high performance even when many peers are found.

The status of the data retrieval is reported for each peer, indicating whether data was found, missing, or if an error occurred during retrieval.

## See Also

- [Output Formats](output-formats.md) for details on human-readable and JSON output.
