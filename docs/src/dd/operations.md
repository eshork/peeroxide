# Dead Drop Output Formats

The `dd` command supports both human-readable terminal output and machine-readable JSON output for integration with other tools.

## Human-Readable Output (Default)

By default, `dd` prints status messages and progress indicators to `stderr`, while result data (for `get`) or keys (for `put`) are handled based on the output configuration.

### Progress Indicators

When running in a TTY, `dd` displays a dynamic progress bar using `indicatif`. For non-TTY environments, it prints a periodic status line approximately every 2 seconds.

- **v2 Protocol**: The bar displays separate counters for the index and data tiers: `filename I[idx/total] D(data/total) [bar] % @ rate ETA`.
- **v1 Protocol**: Since v1 lacks an index tier, only the data counter is shown: `D(data/total) [bar] % @ rate ETA`.

You can suppress all progress output with the `--no-progress` flag. Lifecycle messages, such as DHT refresh status, acknowledgements, and final write confirmations, are preserved on `stderr` regardless of progress flags.

### Status Examples

**put status output**
```text
DD PUT 5 chunks (4500 bytes)
  published to DHT (best-effort)
  pickup key printed to stdout
  refreshing every 600s, monitoring for acks...
  [ack] received from e5f6g7h8...
```

**get status output**
```text
DD GET @a1b2c3d4...
  ack sent (ephemeral identity)
  written to out.bin
  done
```

## Machine-Readable Output (`--json`)

The `--json` flag enables a stream of JSON Lines on **stdout**. Human-readable status messages continue to be sent to **stderr**.

When using `dd get --json`, you must provide a file path via `--output FILE`. This prevents the binary payload from corrupting the JSON stream on `stdout`.

> **Note**: The `progress` event shape was updated from previous documentation to expose per-tier (index/data) counters and rate/ETA fields. The previous schema was not implemented.

### Event Schema

Each JSON object contains a `type` field to discriminate between event types.

#### `start`
Emitted when the operation begins.

```json
{"type":"start","phase":"put","version":2,"filename":"foo.bin","bytes_total":10485760,"indexes_total":4,"indexes_done":0,"data_total":160,"data_done":0,"ts":"2026-05-09T12:00:00Z"}
```

#### `progress`
Emitted periodically during data transfer. `eta_seconds` is omitted if the rate has not yet stabilized.

```json
{"type":"progress","phase":"put","version":2,"filename":"foo.bin","bytes_done":5242880,"bytes_total":10485760,"indexes_done":2,"indexes_total":4,"data_done":80,"data_total":160,"rate_bytes_per_sec":1048576.0,"eta_seconds":5.0,"elapsed_seconds":5.0,"ts":"2026-05-09T12:00:05Z"}
```

#### `result`
Emitted when the primary objective is completed (data published or retrieved).

**PUT result:**
```json
{"type":"result","phase":"put","version":2,"pickup_key":"aabbcc...","bytes":10485760,"chunks":164,"ts":"2026-05-09T12:00:10Z"}
```

**GET result:**
```json
{"type":"result","phase":"get","version":2,"bytes":10485760,"crc":"aabbccdd","output":"out.bin","ts":"2026-05-09T12:00:20Z"}
```

#### `ack`
Emitted by the sender when a recipient acknowledges receipt.

```json
{"type":"ack","pickup_number":1,"peer":"aabbcc...","ts":"2026-05-09T12:00:30Z"}
```

#### `done`
Emitted when the entire operation (including cleanup or final waiting) is finished.

```json
{"type":"done","phase":"put","version":2,"filename":"foo.bin","bytes_done":10485760,"bytes_total":10485760,"indexes_done":4,"indexes_total":4,"data_done":160,"data_total":160,"elapsed_seconds":10.0,"ts":"2026-05-09T12:00:10Z"}
```

### Protocol Version 1 Convention

For `dd` protocol version 1 (single-linked-list of chunks), `indexes_total` and `indexes_done` are always `0` in all events. There is no index/data tier split in v1; all chunks contribute to `data_total`/`data_done` and `bytes_total`/`bytes_done`.
