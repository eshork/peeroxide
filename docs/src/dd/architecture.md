# Dead Drop Architecture

The `dd` command implements two distinct protocol architectures for storing and retrieving data on the DHT. Both protocols are built on the DHT primitives documented in [DHT Primitives](../concepts/dht-primitives.md) (`mutable_put` / `mutable_get` / `immutable_put` / `immutable_get` / `announce`).

## Protocol V1: Linear Chain

The V1 protocol is a simple linked list of mutable DHT records. Each record contains a portion of the file and the public key of the next chunk in the chain.

### V1 Flow

```mermaid
sequenceDiagram
    participant S as Sender
    participant DHT as DHT Nodes
    participant R as Receiver

    Note over S: Chunking + Key Derivation
    S->>DHT: mutable_put(root_pk)
    S->>DHT: mutable_put(chunk_1_pk)
    S->>DHT: ...
    S->>DHT: mutable_put(chunk_N_pk)

    Note over R: Get root_pk
    R->>DHT: mutable_get(root_pk)
    DHT-->>R: root record
    loop Sequential Fetch
        R->>DHT: mutable_get(next_pk)
        DHT-->>R: chunk record
    end
```

V1 features sequential fetching with exponential retry logic (1s to 30s) per chunk, bounded by the global timeout.

## Protocol V2: Merkle Tree

V2 uses a hierarchical tree structure to enable massive file support and parallel fetching.

### V2 Flow

```mermaid
sequenceDiagram
    participant S as Sender
    participant DHT as DHT Nodes
    participant R as Receiver

    Note over S: Canonical Tree Build
    S->>DHT: immutable_put(data_chunks)
    S->>DHT: mutable_put(index_chunks)
    S->>DHT: mutable_put(root_pk)

    Note over R: BFS Parallel Fetch
    R->>DHT: mutable_get(root_pk)
    DHT-->>R: root (metadata + top slots)
    
    rect rgb(240, 240, 240)
        Note over R: Parallel BFS Loop
        R->>DHT: mutable_get(index_pk)
        R->>DHT: immutable_get(data_hash)
    end

    Note over R: Need-list Cycle
    R->>DHT: announce(need_topic)
    R->>DHT: mutable_put(need_topic, ranges)
    DHT-->>S: watch(need_topic)
    S->>R: Republish missing chunks
```

### AIMD Congestion Control

V2 employs an Additive Increase / Multiplicative Decrease (AIMD) controller to manage concurrency:
- **EWMA-based:** Smoothes sample noise with an alpha of 0.1.
- **Decision interval:** 20 samples.
- **Fast-trip:** Shrinks immediately if 10 degraded samples occur within a window.
- **Shrink:** 0.75x current (minimum 1).
- **Grow:** +2 permits.

### Robustness Mechanisms

- **Stall Watchdog:** Checks every 5s. If no put resolves for 30s, it forces AIMD to a recovery floor.
- **Sliding-window Timeout:** `get` operations abort only if no chunk decodes for `--timeout` seconds.
- **Graceful Shutdown:** First Ctrl-C triggers a sticky cancel signal that enqueues cleanups (like empty need-list sentinels). A second double-press force-exits.
- **Need-list Lifecycle:** Receivers publish the encoded missing-range need-list via `mutable_put` every 20s and announce keepalive on the need topic every 60s. Senders poll the need topic every 5s and prioritize enqueuing the full path (index + data) for any newly-listed chunks.

## DHT Wire Monitoring

The `dd` command monitors raw network overhead by reading atomic counters from the underlying DHT handle.

| Method | Return |
|--------|--------|
| `wire_stats()` | `(u64, u64)` (sent, received) |
| `wire_counters()` | `WireCounters` (shared atomic handles) |

These counters allow the progress UI to calculate "wire amplification" — the ratio of total bytes sent/received versus actual payload bytes delivered.
