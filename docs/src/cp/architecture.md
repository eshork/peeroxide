# Architecture

The `cp` tool is built on top of the `peeroxide` swarm, leveraging the DHT for peer discovery and UDX for efficient data transport.

## Data Flow

The transfer process involves two main stages: discovery and streaming.

### 1. Discovery

- **Sender**: Joins the topic on the DHT as a server (`JoinOpts { client: false, .. }`). It announces its presence and waits for incoming connections.
- **Receiver**: Joins the topic on the DHT as a client (`JoinOpts { server: false, .. }`). It looks for peers announcing the topic and attempts to connect to them.

### 2. Protocol Handshake

Once a connection is established, the peers perform a simple JSON-based handshake:

1. **Metadata**: The sender transmits a JSON object containing file information:
   - `filename`: The name of the file being sent.
   - `size`: The total size in bytes (if known).
   - `version`: The protocol version (currently `1`).
2. **Acceptance**: The receiver validates the metadata and (unless `--yes` is used) prompts the user for confirmation.

### 3. Streaming

Data is streamed in chunks (default `CHUNK_SIZE = 65536`) over the established SecretStream.

```mermaid
sequenceDiagram
    participant S as Sender
    participant DHT as DHT / Bootstrap
    participant R as Receiver

    S->>DHT: Announce Topic
    R->>DHT: Lookup Topic
    DHT-->>R: Peer Address (Sender)
    R->>S: Establish Connection (UDX + Noise)
    S->>R: Send Metadata (JSON)
    Note over R: Prompt User (if not -y)
    loop Data Streaming
        S->>R: Data Chunk (64KB)
    end
    S->>R: Shutdown Stream
    R-->>S: Connection Closed
```

## Implementation Details

### Chunking
While the underlying UDX protocol handles packetization, `cp` reads and writes data in 64KB blocks. This balances memory usage with throughput efficiency.

### File Handling
- **Temporary Files**: During a receive operation, data is written to a hidden temporary file in the destination directory.
- **Atomic Rename**: Once the transfer is complete and the received size matches the expected size, the temporary file is renamed to the final destination name. This prevents leaving partial or corrupted files if a transfer is interrupted.
- **Sanitization**: Filenames provided by the sender are sanitized to prevent path traversal attacks (e.g., removing `..` or leading slashes).

### Network Configuration
The `cp` command respects global peeroxide configuration for bootstrap nodes and firewall settings.
- **Public Mode**: If `--public` is set, the swarm attempts to use public bootstrap nodes and sets the firewall to open.
- **Firewalled Mode**: If the node is detected as being behind a NAT, it will attempt hole-punching to establish the connection.
