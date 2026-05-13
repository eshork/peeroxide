# Announce Architecture

The `announce` command manages a long-running swarm session, coordinating DHT presence and optionally handling incoming connections.

## Initialization Flow

1. **Identity Generation**: A `KeyPair` is either generated randomly or derived from a seed.
2. **Swarm Setup**: A `SwarmConfig` is constructed with the identity and DHT configuration. The `--public` / `network.public` setting drives bootstrap node selection (see [init/overview.md → Global CLI Flags](../init/overview.md#global-cli-flags)); it does not change firewall semantics.
3. **Joining Topic**: The node joins the topic using `JoinOpts { client: false }`. This instructs the DHT to act as a server for this topic, making the node discoverable to lookup queries.
4. **Flushing**: The node waits for the join operation to flush, ensuring at least one announcement has reached the DHT.

## Metadata Persistence

If the `--data` flag is provided, the node performs an initial `mutable_put` to the DHT.

- **Storage**: The data is signed by the node's private key and stored at the node's public key address on the DHT.
- **Sequence**: The `seq` field is set to the current Unix epoch in seconds.
- **Lifecycle**: A background task triggers every 600 seconds to re-put the data, preventing expiration and keeping the DHT record fresh.

## Connection Management

When a peer discovers this node via `lookup`, they may attempt to open a direct UDX connection.

- **Accepting**: The `announce` loop listens on the `conn_rx` channel for incoming connections.
- **Modes**:
    - **Default**: Incoming connections are immediately dropped to minimize resource usage.
    - **Echo Mode (`--ping`)**: Connections are accepted and passed to the [Echo Protocol](echo-protocol.md) handler.

## Shutdown Sequence

The command remains active until it receives a termination signal (SIGINT or SIGTERM) or the `--duration` timer expires.

1. **Unannounce**: The node sends a `leave` request to the DHT for the active topic.
2. **Cleanup**: The background refresh task is aborted, and the swarm handle is destroyed.
3. **Exit**: The process exits with code 0.
