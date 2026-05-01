# cp — Output Formats

The `cp` command separates data, status, and progress across output streams.

## Standard Output (stdout)

### `send` subcommand

Prints the transfer topic to stdout so it can be captured or piped to the receiver:

```
a3f2b1...  (64-char hex topic key)
```

### `recv` subcommand (streaming mode)

When destination is `-`, file data is written directly to stdout:

```bash
peeroxide cp recv <topic> - > received_file.bin
```

## Standard Error (stderr)

All status messages, progress, and connection info go to stderr, keeping stdout clean for data.

**Sender output:**
```
CP SEND example.bin (1.2 MB)
topic: a3f2b1...
connected from @deadbeef...
done: 1.2 MB in 2.3s (540 KB/s)
```

**Receiver output:**
```
CP RECV topic: a3f2b1...
looking up sender...
connected to @deadbeef...
Incoming file: example.bin (1.2 MB)
Save to: ./example.bin
done: 1.2 MB in 2.3s (540 KB/s)
```

## Progress Display

When transfer size is known, `cp` displays a progress bar on stderr showing percentage, bytes transferred, speed, and ETA. When size is unknown (stdin source), a spinner with running byte count is shown instead.

## Reliability

`cp` does not implement retransmission or resumption at the application layer. Reliability is provided by the UDX transport layer (BBR congestion control, retransmission, ordering). If a transfer fails mid-stream:

- The temporary file is left in place (not renamed to final destination).
- Re-running `cp recv` with the same topic will restart the transfer from the beginning.
- The sender must still be running and on the same topic.

For large files over unreliable networks, consider compressing the payload before transfer to reduce exposure to interruptions.
