# Lookup Output Formats

The `lookup` command supports two output modes: human-readable (default) and NDJSON.

## Human-Readable Output

By default, `lookup` writes diagnostic information and peer records to **stderr**. The **stdout** stream remains empty.

### Header
The output begins with the resolved topic:
- `LOOKUP <hex>` (if raw hex was provided)
- `LOOKUP blake2b("topic")` (if plaintext was provided)
- `  found <N> peers`

### Peer Record
For each peer found:
- `@<hex_public_key>`
- `    relays: host:port, ...` (or `(direct only)` if no relays are registered)

### Metadata (with `--with-data`)
If data is successfully retrieved:
- `    data: "string"` (escaped UTF-8) or `0x<hex>` (if binary)
- `    seq: <u64>`

Statuses for missing or failed data:
- `    data: (not stored)`
- `    data: (error: <message>)`

## JSON Output (NDJSON)

When the `--json` flag is used, `lookup` emits Newline Delimited JSON to **stdout**. Diagnostic logs may still appear on stderr.

### Peer Schema
Each discovered peer is emitted as a separate JSON object:

```json
{
  "type": "peer",
  "public_key": "<hex>",
  "relay_addresses": ["host:port", ...]
}
```

### Peer Schema (with `--with-data`)
If metadata is requested, the peer object includes status fields:

- **Success**:
  ```json
  {
    "type": "peer",
    "public_key": "...",
    "relay_addresses": [...],
    "data_status": "ok",
    "data": "<string>",
    "data_encoding": "utf8"|"hex",
    "seq": <u64>
  }
  ```
  Note: The `data` field contains raw hexadecimal characters (without a `0x` prefix) when the encoding is `"hex"`.

- **Missing**:
  ```json
  {
    "type": "peer",
    "data_status": "none",
    "data": null,
    "seq": null,
    ...
  }
  ```

- **Error**:
  ```json
  {
    "type": "peer",
    "data_status": "error",
    "data": null,
    "seq": null,
    "error": "<message>",
    ...
  }
  ```

### Summary Schema
A final summary object is emitted after all peers have been processed:

```json
{
  "type": "summary",
  "topic": "<hex>",
  "peers_found": <int>
}
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Fatal error (e.g., DHT failure, invalid arguments) |
| 130 | Terminated by SIGINT (Ctrl+C) |
