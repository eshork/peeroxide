# Ping Overview

The `ping` tool is a multi-purpose diagnostic utility for the Peeroxide network. It allows you to verify connectivity to bootstrap nodes, classify your local NAT type, and perform targeted probes of peers discovered on the DHT.

## Usage

```sh
peeroxide ping [target] [flags]
```

### Targets

The `ping` tool behaves differently depending on the provided target:

- **No target**: Performs a **bootstrap check**. It probes all configured bootstrap nodes to populate the local routing table, collect reflexive addresses, and classify your NAT type.
- **`host:port`**: Performs a direct UDP probe to a specific network address.
- **`@<64-char-hex-pubkey>`**: Resolves the peer's relay addresses via `find_peer` and probes them.
- **`<topic>`**: Resolves up to 20 peers announcing on the given topic (specified as a plain string or 64-char hex) and probes their relay addresses.

## Operational Modes

### Bootstrap Check
When run without a target, `ping` sends `CMD_FIND_NODE` requests to bootstrap nodes. This process:
1. Verifies reachability to the core network.
2. Discovers your reflexive (public) IP and port as seen by multiple nodes.
3. Classifies your **NAT Type** based on the consistency of these reflexive addresses.
4. Populates your local DHT routing table with closer nodes.

### UDP Probing (Direct, PubKey, Topic)
Standard probes use the DHT `CMD_PING` RPC. This is a lightweight UDP-based check that verifies the target node is online and responding at the network level.

### Connection Probing (`--connect`)
When the `--connect` flag is used with a PubKey or Topic target, the tool performs a full Noise XX handshake and establishes a `SecretStream`. After the secure connection is established, it executes the [Echo Protocol](../announce/echo-protocol.md) to measure end-to-end encrypted latency.

## Flags

- `--count <N>`: Number of probes to send (default: 1). Set to `0` for infinite probing.
- `--interval <seconds>`: Delay between probes (default: 1.0s).
- `--connect`: Attempt a full Noise handshake and Echo protocol test.
- `--json`: Output results as newline-delimited JSON (NDJSON) for machine consumption.
- `--public`: Use the public bootstrap network (shorthand for adding default bootstrap nodes).

## Exit Codes

- `0`: All probes succeeded.
- `1`: Partial or total failure (e.g., timeouts, resolution errors).
- `130`: Interrupted by SIGINT (Ctrl+C).

