# Dead Drop Overview

The `dd` command provides an anonymous, asynchronous store-and-forward mechanism using the DHT. It allows a sender to "put" data on the network that a receiver can later "get" using a unique key, without requiring both parties to be online at the same time.

Unlike the `cp` command, which establishes a direct peer-to-peer connection between a sender and receiver, `dd` uses mutable DHT values to store data. This makes it ideal for scenarios where the sender and receiver have intermittent connectivity or want to avoid direct IP discovery.

## Key Features

- **Asynchronous Delivery:** Data is stored on DHT nodes. The receiver picks it up whenever they're ready.
- **Mutable DHT Storage:** Uses the HyperDHT `mutable_put` and `mutable_get` operations.
- **Chunked Transfers:** Large files are automatically split into multiple chunks, linked together in a chain.
- **Passphrase Support:** Pickup keys can be derived from human-readable passphrases.
- **Anonymity:** No direct connection is established between the sender and receiver.
- **Acknowledgements:** Optional pickup notifications (acks) let the sender know when data was retrieved.

## Basic Usage

### Putting Data

To put a message or file at a dead drop on the DHT:

```bash
echo "Hello from the void" | peeroxide dd put - --passphrase "my secret drop"
```

The tool will print a 64-character hexadecimal pickup key (unless a passphrase is used). It will then continue to run, refreshing the data on the DHT to ensure it doesn't expire.

### Getting Data

To retrieve data:

```bash
peeroxide dd get --passphrase "my secret drop"
```

The receiver fetches each chunk sequentially, reassembles the original data, and verifies its integrity using a CRC-32C checksum.

## How it Differs from `cp`

| Feature | `cp` | `dd` |
|---------|------|------|
| **Connection** | Direct P2P (UDX) | Mediated via DHT storage |
| **Online Requirement** | Both must be online | Asynchronous |
| **Discovery** | Topic-based | Key-based (Public Key) |
| **Speed** | High (Direct) | Moderate (DHT round-trips) |
| **Metadata** | Filename, size | Sequential chunks |

