# Output Formats

The `cp` command uses different output streams to separate data, status information, and progress tracking.

## Standard Output (stdout)

### `send` Command
When running `cp send`, the tool prints the topic to `stdout`. This allows you to capture the topic for use in other scripts or to pass it to the receiver.

```bash
# Example output
my-shared-topic
```

### `recv` Command (Streaming Mode)
When running `cp recv` with the destination set to `-`, the actual file data is written to `stdout`.

```bash
peeroxide cp recv shared-topic - > received_file.bin
```

## Standard Error (stderr)

All status messages, connection information, and progress bars are written to `stderr`. This ensures that they do not interfere with file data when streaming via pipes.

### Sender Progress
The sender provides details about the file, the topic, and the connected receiver:
- `CP SEND <filename> (<size>)`
- `topic: <hex-key>`
- `connected from @<remote-id>`
- `done: <total-size> in <time>s (<speed>/s)`

### Receiver Progress
The receiver provides details about the lookup and transfer:
- `CP RECV topic: <hex-key>`
- `looking up sender...`
- `connected to @<remote-id>`
- `Incoming file: <filename> (<size>)`
- `Save to: <path>`

## Machine-Readable Output

While the current implementation of `cp` primarily focuses on human-readable status via `stderr`, the underlying protocol uses JSON for metadata exchange.

### Protocol Metadata
During the handshake, the following JSON structure is exchanged:

```json
{
  "filename": "example.txt",
  "size": 1024,
  "version": 1
}
```

*Note: If `size` is unknown (e.g., when streaming from `stdin`), it will be `null`.*

## Progress Bar Styles

When the `--progress` flag is used, a dynamic progress bar is displayed on `stderr`.

- **Known Size**: Displays a percentage bar, bytes transferred, total bytes, transfer speed, and estimated time of arrival (ETA).
- **Unknown Size**: Displays a spinner and total bytes transferred with current speed.
