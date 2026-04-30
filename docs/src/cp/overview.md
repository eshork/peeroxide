# Overview

The `cp` command provides secure, point-to-point file transfers between peers over the Hyperswarm network. It allows you to send and receive files using a shared topic, which serves as a rendezvous point on the DHT.

## Key Features

- **Direct Transfers**: Files are streamed directly between peers using UDX, ensuring high performance and reliable delivery.
- **Topic-Based Discovery**: Use human-readable strings or 64-character hex keys to coordinate transfers without needing to know the other peer's IP address.
- **Piping Support**: Full support for `stdin` (`-`) as a source and `stdout` (`-`) as a destination, making it easy to integrate with other command-line tools.
- **Progress Tracking**: Real-time progress bars and transfer statistics provided via `stderr`.
- **End-to-End Encryption**: All transfers are secured using Noise handshakes via SecretStream.

## Basic Usage

### Sending a File

To send a file, specify the file path and a topic. The command will output the topic (useful if you let it generate a random one) and wait for a receiver.

```bash
peeroxide cp send my-file.txt "shared-topic"
```

### Receiving a File

To receive a file, use the same topic. You can specify a destination path or a directory.

```bash
peeroxide cp recv "shared-topic" ./downloads/
```

### Streaming with Pipes

You can pipe data directly through `cp`:

```bash
# Sender
cat data.tar.gz | peeroxide cp send - "backup-topic"

# Receiver
peeroxide cp recv "backup-topic" - > restored.tar.gz
```

## Command Options

### `send` Options

- `file`: Path to the file to send, or `-` for `stdin`.
- `topic`: Optional topic name or 64-char hex key.
- `--name`: Override the filename sent in the metadata.
- `--keep-alive`: Keep the sender running for multiple sequential transfers.
- `--progress`: Show a transfer progress bar.

### `recv` Options

- `topic`: The shared topic from the sender.
- `dest`: Optional destination path or directory, or `-` for `stdout`.
- `--yes`: Skip the confirmation prompt.
- `--force`: Overwrite existing files without asking.
- `--timeout`: Seconds to wait for a sender (default: 60).
- `--progress`: Show a transfer progress bar.
