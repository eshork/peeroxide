# Deaddrop Output Formats

The `deaddrop` command supports both human-readable terminal output and machine-readable JSON output for integration with other tools.

## Human-Readable Output (Default)

By default, `deaddrop` prints status messages to `stderr` and the resulting data (for `pickup`) or key (for `leave`) to `stdout`.

### `leave` status output
```text
DEADDROP LEAVE 5 chunks (4500 bytes)
  published chunk 1/5
  published chunk 2/5
  ...
  published to DHT (best-effort)
  pickup key printed to stdout
  refreshing every 600s, monitoring for acks...
```

### `pickup` status output
```text
DEADDROP PICKUP @a1b2c3d4...
  fetching chunk 1/5...
  fetching chunk 2/5...
  ...
  reassembled 4500 bytes
  ack sent (ephemeral identity)
  done
```

## Machine-Readable Output (`--json`)

Using the `--json` flag changes the output to a single-line JSON object per event or result.

### `leave` result
When data is successfully published, the pickup key is returned:

```json
{
  "type": "result",
  "pickup_key": "a1b2c3d4...",
  "chunks": 5,
  "bytes": 4500
}
```

### `pickup` result
When data is successfully retrieved:

```json
{
  "type": "result",
  "bytes": 4500,
  "crc": "f3b2a100",
  "output": "stdout"
}
```

### Progress Events
Intermediate progress can also be tracked via JSON:

```json
{
  "type": "progress",
  "chunk": 3,
  "total": 5,
  "action": "fetch"
}
```

### Acknowledgement Events
When the sender detects a pickup via an ack:

```json
{
  "type": "ack",
  "pickup_number": 1,
  "peer": "e5f6g7h8..."
}
```

