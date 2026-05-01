# DHT and Routing

Peeroxide uses a Kademlia-based Distributed Hash Table (DHT) for peer discovery and coordination. This DHT is wire-compatible with the HyperDHT network used by Hyperswarm.

## Kademlia Basics

The DHT operates on several core Kademlia principles:

- **XOR Distance**: The "distance" between two nodes or a node and a key is calculated using the bitwise XOR of their 32-byte IDs. This metric defines the topology of the network.
- **Routing Table & k-buckets**: Each node maintains a routing table organized into buckets (k-buckets). Each bucket covers a specific range of distances from the node's own ID.
- **Iterative Lookup**: Finding a value or node involves querying the closest known nodes to the target key, which then return even closer nodes, eventually converging on the target.

Peeroxide relies on the [`pkarr`](https://docs.rs/pkarr) and [`mainline`](https://docs.rs/mainline) crates for much of its underlying DHT logic.

## Bootstrap Nodes

A DHT is a decentralized network, but new nodes need an entry point to join. These entry points are called **bootstrap nodes**.

- **Public Network**: By default, `peeroxide` uses a set of stable public bootstrap nodes to connect to the global HyperDHT network.
- **Configuration**: You can specify custom bootstrap nodes using the `--bootstrap` flag or the `network.bootstrap` setting in your config file.
- **Isolated Mode**: If no bootstrap nodes are provided and the `--public` flag is not set, the node runs in isolated mode. In this mode, discovery is only possible if peers connect to each other directly by address.

## Connectivity

The DHT doesn't just store peer records; it also facilitates connectivity:

- **Holepunching**: The DHT helps two firewalled peers coordinate a direct UDP connection.
- **Relaying**: If a direct connection is impossible, the DHT can help set up a relayed connection through other nodes.

For more details on how these primitives are used in practice, see the [lookup](../lookup/overview.md) and [announce](../announce/overview.md) command documentation.
