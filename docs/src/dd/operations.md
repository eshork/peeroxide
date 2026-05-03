# Dead Drop Output Formats

The `dd` command supports both human-readable terminal output and machine-readable JSON output for integration with other tools.

## Human-Readable Output (Default)

By default, `dd` prints status messages to `stderr` and the resulting data (for `get`) or key (for `put`) to `stdout`.

### `put` status output
```text
DD PUT 5 chunks (4500 bytes)
  published chunk 1/5
  published chunk 2/5
  ...
  published to DHT (best-effort)
  pickup key printed to stdout
  refreshing every 600s, monitoring for acks...
```

### `get` status output
```text
DD GET @a1b2c3d4...
  fetching chunk 1/5...
  fetching chunk 2/5...
  ...
  reassembled 4500 bytes
  ack sent (ephemeral identity)
  done
```

## Machine-Readable Output (`--json`)

Using the `--json` flag changes the output to a single-line JSON object per event or result.

### `put` result
When data is successfully published, the pickup key is returned:

```json
{
  "type": "result",
  "pickup_key": "a1b2c3d4...",
  "chunks": 5,
  "bytes": 4500
}
```

### `get` result
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

