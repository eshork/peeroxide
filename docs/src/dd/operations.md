# Dead Drop Operations

The `dd` command supports both human-readable terminal output and machine-readable JSON output for integration with other tools.

## Command Line Flags

In addition to the dd-specific flags shown below, both `dd put` and `dd get` accept the inherited top-level global flags: `--config <FILE>`, `--no-default-config`, `--public`, `--no-public`, `--bootstrap <ADDR>` (repeatable), and `-v` / `--verbose`. These control config file loading, DHT bootstrap node selection, and tracing verbosity; see [init/overview.md → Global CLI Flags](../init/overview.md#global-cli-flags) for the bootstrap-resolution algorithm.

### `dd put` Flags

| Flag | Default | Description |
|------|---------|-------------|
| `<file>` | required | Input file path. Use `-` for stdin. |
| `--max-speed <S>` | none | Limit transfer speed. Parses `k`/`m` suffixes (base-10, case-insensitive). |
| `--refresh-interval <secs>` | `600` | Seconds between refresh cycles (must be > 0). |
| `--ttl <secs>` | none | Stop refreshing after N seconds (must be > 0). |
| `--max-pickups <N>` | none | Exit after N unique pickup acks (must be > 0). |
| `--passphrase <S>` | none | Deterministic root seed from `discovery_key(passphrase)`. |
| `--interactive-passphrase` | none | TTY prompt for passphrase with hidden input. |
| `--no-progress` | `false` | Suppress progress UI. |
| `--json` | `false` | Emit JSON-Lines progress on stdout. |
| `--v1` | `false` | Force legacy v1 protocol. |

### `dd get` Flags

| Flag | Default | Description |
|------|---------|-------------|
| `<key>` | required* | 64-character hex pickup key or passphrase text. |
| `--passphrase <S>` | none | Derive pickup key from passphrase. |
| `--interactive-passphrase` | none | TTY prompt for passphrase with hidden input. |
| `--no-progress` | `false` | Suppress progress UI. |
| `--output <PATH>` | `stdout` | Write payload to file instead of stdout. |
| `--json` | `false` | Emit JSON-Lines progress. **Requires** `--output`. |
| `--timeout <secs>` | `1200` | Sliding no-progress timeout in seconds (must be > 0). |
| `--no-ack` | `false` | Suppress pickup acknowledgement announce. |

*\*Key is required unless a passphrase flag is provided.*

## Key Derivation and Passphrases

- **Passphrase Derivation:** When a passphrase is used, the root seed is derived via `discovery_key(passphrase)`.
- **Interactive Fallback:** The `--interactive-passphrase` flag attempts to open `/dev/tty` for hidden input, falling back to stdin if unavailable.
- **Key vs Passphrase:** If a positional argument is exactly 64 characters of valid hex, it is treated as a raw 32-byte pickup key. Otherwise, it is treated as passphrase text and hashed via `discovery_key`.

## Progress UX

The mode is selected automatically:
1. `--json` -> JSON Lines on stdout.
2. `--no-progress` -> Progress disabled.
3. stderr is TTY -> Interactive bars.
4. else -> Periodic log line (every 2s).

### Bar Layouts

- **V1 Put:** `↑ filename D(bytes/total) [bar] pct rate ETA`
- **V2 Put:** `↑ filename I[idx/total] D(bytes/total) [bar] pct rate ETA`
- **V2 Get (4-bar multi):**
  - **index:** `I[idx/total] rate`
  - **data:** `D(bytes/total) [bar] pct rate ETA`
  - **wire:** `W ↑ rate ↓ rate (x amplification)`
  - **overall:** `filename bytes/total pct`

*Wire amplification (`wire_total / bytes_done`) is omitted until the first payload byte is received.*

## Machine-Readable Output (`--json`)

The `--json` flag enables a stream of JSON Lines on **stdout**. Events use `type` as a discriminator and RFC3339 timestamps.

### Event Schema

| Type | Description |
|------|-------------|
| `start` | Operation initiated. Includes `version`, `filename`, `bytes_total`, `indexes_total`, `data_total`. |
| `progress` | Periodic update. Includes `bytes_done`, `rate_bytes_per_sec`, `eta_seconds`, `elapsed_seconds`. |
| `result` | Objective achieved. `put` returns `pickup_key` and `chunks`. `get` returns `crc` and `output`. |
| `ack` | Sender-only. Emitted when a recipient acknowledges receipt. Includes `peer` and `pickup_number`. |
| `done` | Operation completed. Includes final counters and `elapsed_seconds`. |

**V1 Convention:** `indexes_total` and `indexes_done` are always `0` in V1 events.

## Acknowledgement (Ack) Mechanism

When a `get` operation completes (unless `--no-ack` is set), the receiver announces on the ack topic:
`ack_topic = discovery_key(root_pk || b"ack")`

The sender polls this topic every 30s and counts unique announcer public keys.
