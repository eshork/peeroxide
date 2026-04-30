# Security Model

This appendix describes the threat model and security properties of the `peeroxide-cli` tools.

## Transport Security

All peer-to-peer connections established by `peeroxide-cli` use the **Noise XX** handshake protocol followed by a **SecretStream** encrypted channel. This provides:

- **Mutual authentication**: Both peers authenticate with their Ed25519 public keys during handshake.
- **Forward secrecy**: Session keys are ephemeral and derived per-connection.
- **Confidentiality and integrity**: All application data is encrypted and authenticated.

The Noise XX pattern is the same used by the Node.js Hyperswarm stack, ensuring interoperability.

## Peer Identity

A peer's identity is its **Ed25519 public key** (32 bytes). There is no central authority — identity is self-sovereign. Anyone who controls a private key controls that identity.

- **Ephemeral identities** (default for `announce`) are generated fresh on each run and leave no persistent trace.
- **Seeded identities** (`--seed` flag) are deterministic: `KeyPair::from_seed(discovery_key(seed.as_bytes()))`. The seed string is a secret — anyone who knows it can derive the same keypair.

> **Warning**: Treat `--seed` values like passwords. They are not hashed with a KDF — raw BLAKE2b is fast, making brute-force of short seeds feasible.

## DHT Trust Model

The DHT is **untrusted infrastructure**. Any node can relay packets, and routing table entries are not authenticated. Mitigations:

- Mutable DHT values are **Ed25519-signed** by the originating keypair. Verifiers (including `lookup --with-data`) confirm the signature before using the data. Forging data requires the private key.
- Immutable DHT values (used by `cp`) are addressed by the SHA-256 hash of their content. Content is verified on retrieval.
- Topic keys are not secret — anyone who knows the topic can look up its peer list. Do not treat topic confidentiality as a security property.

## `deaddrop` Threat Model

`deaddrop` uses **mutable DHT storage** addressed by `(public_key, topic)`. Security properties:

- Only the holder of the private key can write to a slot (signatures enforced by the DHT).
- Anyone who knows `(public_key, topic)` can read the slot — there is no access control on reads.
- Data is signed but **not encrypted** at the DHT layer. For sensitive payloads, encrypt the application data before using `deaddrop`.
- `deaddrop` is designed for asynchronous communication where sender and receiver share a topic out-of-band.

## `cp` Threat Model

`cp` data is **immutable and content-addressed**. Anyone who knows the `cp://` key can retrieve the data. There is no access control. Do not use `cp` for sensitive data without prior encryption.

## Echo Protocol Security

The echo protocol (see [Echo Protocol](../announce/echo-protocol.md)) is intentionally minimal — it echoes arbitrary 16-byte payloads after a `PING`/`PONG` handshake. Session concurrency is capped at `MAX_ECHO_SESSIONS = 64` to limit resource exhaustion. The handshake uses a 5-second timeout to prevent slowloris-style attacks.

Because `announce` connections go through the Noise XX handshake, all echo traffic is authenticated and encrypted. An unauthenticated party cannot reach the echo server.

## Denial of Service Considerations

- Bootstrap nodes are configurable. An attacker controlling all bootstrap nodes can eclipse a node from the DHT.
- The DHT is susceptible to Sybil attacks in the general case — this is a known limitation of permissionless DHTs.
- `announce` data refresh runs every 600 seconds. If a node is offline, its announcement expires naturally according to DHT TTL policies.
