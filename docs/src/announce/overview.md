# Announce Overview

The `announce` command makes your node discoverable on the DHT for a specific topic. It allows other peers using `lookup` to find your public key and connection details.

## Identity and Keypairs

By default, `announce` generates a random ephemeral keypair for each session.

- **Ephemeral**: A new identity is created on startup and lost when the process exits.
- **Seeded**: Using the `--seed <string>` flag, you can derive a deterministic keypair. The seed is hashed using the `discovery_key` function to produce the 32-byte secret seed.

## Usage

```bash
peeroxide announce <TOPIC> [FLAGS]
```

### Flags

| Flag | Description |
|------|-------------|
| `--seed <string>` | Use a seed to maintain a stable identity across restarts. |
| `--data <string>` | Store metadata (max 1000 bytes) on the DHT. |
| `--duration <sec>` | Exit automatically after the specified number of seconds. |
| `--ping` | Enable the [Echo Protocol](echo-protocol.md) to accept and respond to connectivity probes. |

## Metadata

The `--data` flag allows you to attach a small payload (up to 1000 UTF-8 bytes) to your DHT record. This is useful for sharing service versioning, protocol capabilities, or small state updates.

- **Sequence Numbers**: Metadata is stored with a sequence number (`seq`) based on the current Unix epoch in seconds.
- **Refresh**: To ensure your record does not expire, `announce` automatically refreshes the metadata every 600 seconds (10 minutes) while the process is running.

## Output

All output from `announce` is written to **stderr**. The **stdout** stream is always empty.

### Startup
- `ANNOUNCE blake2b("topic") as @<pk_hex>`
- `  announced to closest nodes`
- `  metadata: "..." (<N> bytes, seq=<u64>)` (if applicable)

### Shutdown
- `UNANNOUNCE blake2b("topic")` (or `UNANNOUNCE <hex>` if raw hex was provided)
- `  done`

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success (including exit via SIGINT or SIGTERM) |
| 1 | Fatal error (e.g., DHT failure, invalid data size) |

## See Also

- [Architecture](architecture.md) for internal implementation details.
- [Echo Protocol](echo-protocol.md) for details on the `--ping` mode.
