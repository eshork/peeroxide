# Dead Drop Overview

The `dd` command provides an anonymous, asynchronous store-and-forward mechanism using the DHT. It allows a sender to "put" data on the network that a receiver can later "get" using a unique key, without requiring both parties to be online at the same time.

Unlike the `cp` command, which establishes a direct peer-to-peer connection, `dd` uses DHT records to store data. This makes it ideal for scenarios where the sender and receiver have intermittent connectivity or want to avoid direct IP discovery.

## Key Features

- **Asynchronous Delivery:** Data is stored on DHT nodes. The receiver picks it up whenever they're ready.
- **Protocol Versions:** Supports both the original v1 linked-list protocol and the high-performance v2 tree-indexed protocol.
- **Passphrase Support:** Pickup keys can be derived from human-readable passphrases.
- **Anonymity:** No direct connection is established between the sender and receiver.
- **Acknowledgements:** Optional pickup notifications (acks) let the sender know when data was retrieved.
- **Progress Control:** Use `--no-progress` for silent operation or `--json` for machine-readable event streams.

## Protocol Selection

The `dd` command supports two protocol versions:

| Version | Characteristics | Selection |
|---------|-----------------|-----------|
| **V1** | Simple linked-list of mutable records. Limited to 64MB. Sequential fetches. | Explicit via `--v1` on `put`. Auto-detected on `get`. |
| **V2** | Merkle-tree indexed. Massive capacity. Parallel fetching with need-lists and AIMD congestion control. | Default on `put`. Auto-detected on `get`. |

### Dispatch Rules

- **Putting:** `dd put` defaults to v2. Use the `--v1` flag to force the legacy protocol.
- **Getting:** `dd get` automatically dispatches based on the first byte of the fetched root record (`0x01` for v1, `0x02` for v2).

## Quick Start

### Putting Data

Put a message using a passphrase (v2 by default):

```bash
echo "Hello from the void" | peeroxide dd put - --passphrase "my secret drop"
```

Put a file using a raw key (generated randomly):

```bash
peeroxide dd put my-file.dat
```

Force v1 for compatibility with older clients:

```bash
peeroxide dd put my-file.dat --v1
```

### Getting Data

Retrieve data using a passphrase:

```bash
peeroxide dd get --passphrase "my secret drop"
```

Retrieve data using a 64-character hex pickup key:

```bash
peeroxide dd get 7215c9...82a3
```

Write to a file while suppressing progress bars:

```bash
peeroxide dd get 7215c9...82a3 --output saved-file.dat --no-progress
```

## How it Differs from `cp`

| Feature | `cp` | `dd` |
|---------|------|------|
| **Connection** | Direct P2P (UDX) | Mediated via DHT storage |
| **Online Requirement** | Both must be online | Asynchronous |
| **Discovery** | Topic-based | Key-based (Public Key) |
| **Speed** | High (Direct) | Moderate (DHT round-trips) |
| **Metadata** | Filename, size | Sequential or Tree chunks |
