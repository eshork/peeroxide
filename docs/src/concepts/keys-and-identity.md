# Keys and Identity

Identity in `peeroxide` is anchored in cryptography. Every peer is identified by an Ed25519 keypair.

## Peer Identity

A peer's **Public Key** is its stable identity on the network. This key is 32 bytes long and is typically represented as a 64-character hexadecimal string, often prefixed with an `@` symbol in the CLI (e.g., `@ab12cd34...`).

## Keypair Types

When running `peeroxide` commands, you can use different types of keypairs:

- **Ephemeral Keypairs**: Generated randomly for a single session. These are used by default when no seed is provided. Once the process exits, the identity is lost.
- **Seeded Keypairs**: Derived deterministically from a secret seed string using the `--seed` flag. The seed is hashed to produce a 32-byte secret, which is then used to generate the Ed25519 keypair. This allows a peer to maintain a stable identity across multiple runs.

## Secure Connections

Peeroxide uses the **Noise XX** handshake protocol to establish authenticated, end-to-end encrypted connections between peers.

- **Authentication**: During the handshake, peers exchange and verify their public keys. This ensures that you are communicating with exactly the peer you intended to, and that no man-in-the-middle can impersonate them.
- **Encryption**: Once the handshake is complete, all data is sent over a `SecretStream`, which provides confidentiality and integrity.

For more information on the cryptographic protocols, see the documentation for the [`ed25519-dalek`](https://docs.rs/ed25519-dalek) and [`noise-protocol`](https://docs.rs/noise-protocol) crates.
