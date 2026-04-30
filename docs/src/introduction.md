# Introduction

`peeroxide-cli` is a command-line toolkit for interacting with the peeroxide P2P networking stack. It provides a set of tools for peer discovery, connectivity diagnostics, and decentralized data transfer, all while maintaining full wire-compatibility with the existing Hyperswarm and HyperDHT networks.

The binary is named `peeroxide`.

## Core Tools

The toolkit consists of five primary commands:

- **[lookup](./lookup/overview.md)**: Query the DHT to find peers announcing a specific topic.
- **[announce](./announce/overview.md)**: Announce your presence on a topic so others can discover you.
- **[ping](./ping/overview.md)**: Diagnose reachability through bootstrap checks, NAT classification, or targeted peer pings.
- **[cp](./cp/overview.md)**: Transfer files directly between peers over an encrypted swarm connection.
- **[deaddrop](./deaddrop/overview.md)**: Perform anonymous store-and-forward messaging via the DHT.

## Key Concepts

To use `peeroxide` effectively, it helps to understand the underlying architecture:

- **[DHT and Routing](./concepts/dht-and-routing.md)**: How the Kademlia-based Distributed Hash Table handles peer discovery and routing.
- **[Keys and Identity](./concepts/keys-and-identity.md)**: How Ed25519 keypairs define peer identity and secure connections.
- **[Topics and Discovery](./concepts/topics-and-discovery.md)**: How peers group together using 32-byte topic keys.
