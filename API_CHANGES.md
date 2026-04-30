# Public API Changes — feat/peeroxide-cli vs main

> Generated during PR review. Documents all public interface additions, modifications,
> and removals across the three library crates on this branch.

---

## Breaking Changes (semver impact)

Three changes break backwards compatibility. All were driven by the `Arc<UdxSocket>`
sharing requirement introduced to allow stream multiplexing over the same socket.

### 1. `libudx` — `UdxSocket::close(self)` → `close(&self)`

**File**: `libudx/src/native/socket.rs`

```rust
// before
pub async fn close(self) -> Result<()>
// after
pub async fn close(&self) -> Result<()>
```

**Why**: `UdxSocket` is now held in `Arc<UdxSocket>` by the DHT layer. A consuming
receiver cannot be called on an `Arc<T>`. The change was unavoidable.

**Practical impact**: Low. No existing call site in the workspace relies on consuming
semantics — all callers drop the socket immediately after `close()`. External callers
who previously relied on `close()` consuming the socket would get a compile error but
a trivial fix (just drop manually).

**Re-exported**: Yes — `UdxSocket` is part of `libudx`'s public API on crates.io.

---

### 2. `peeroxide-dht` — `PeerConnection.socket` type changed

**File**: `peeroxide-dht/src/hyperdht.rs`

```rust
// before
pub socket: libudx::UdxSocket,
// after
pub socket: Arc<UdxSocket>,
```

**Why**: Same Arc-sharing requirement as above.

**Practical impact**: Any caller that moved `socket` out of `PeerConnection` or
relied on the concrete `UdxSocket` type would break.

**Re-exported**: Not re-exported from `peeroxide_dht` crate root — only reachable
via `peeroxide_dht::hyperdht::PeerConnection`.

---

### 3. `peeroxide-dht` — `establish_stream` signature extended

**File**: `peeroxide-dht/src/hyperdht.rs`

```rust
// before
pub async fn establish_stream(
    result: &ConnectResult,
    runtime: &UdxRuntime,
) -> Result<PeerConnection, HyperDhtError>

// after
pub async fn establish_stream(
    result: &ConnectResult,
    runtime: &UdxRuntime,
    shared_socket: Option<Arc<UdxSocket>>,
) -> Result<PeerConnection, HyperDhtError>
```

**Why**: Enables passing a pre-existing socket for stream multiplexing.

**Practical impact**: All existing call sites must add `None` as the third argument
to preserve previous behaviour.

**Re-exported**: Not re-exported from `peeroxide_dht` crate root.

---

## `peeroxide` (1.1.0 → 1.2.0)

### Added

- `SwarmHandle::dht(&self) -> &HyperDhtHandle`
- `SwarmHandle::key_pair(&self) -> &KeyPair`

### Modified

- `pub use` re-exports expanded to also include `HyperDhtHandle`, `MutablePutResult`,
  `MutableGetResult`, `ImmutablePutResult` (previously only `KeyPair`, `DEFAULT_BOOTSTRAP`)

---

## `peeroxide-dht` (1.1.0 → 1.2.0)

### Added — `HyperDhtHandle` methods

- `table_id(&self) -> Result<Option<NodeId>, DhtError>`
- `server_socket(&self) -> Result<Option<Arc<UdxSocket>>, DhtError>`
- `listen_socket(&self) -> Result<Option<Arc<UdxSocket>>, DhtError>`
- `persistent_stats(&self) -> Result<PersistentStats, HyperDhtError>`

### Added — `DhtHandle` methods

- `table_id(&self) -> Result<Option<NodeId>, DhtError>`
- `server_socket(&self) -> Result<Option<Arc<UdxSocket>>, DhtError>`
- `listen_socket(&self) -> Result<Option<Arc<UdxSocket>>, DhtError>`

### Added — `PersistentStats` (new pub struct)

```rust
pub struct PersistentStats {
    pub records: usize,
    pub record_topics: usize,
    pub mutables: usize,
    pub immutables: usize,
    pub router_entries: usize,
}
```

### Added — `PingResponse` fields

- `to: Option<Ipv4Peer>` — reflexive address as seen by remote
- `closer_nodes: Vec<Ipv4Peer>` — closer nodes returned by remote

### Added — other

- `RoutingTable::rebuild_with_id(&mut self, new_id: NodeId)`
- `SecretStream::shutdown(&mut self) -> Result<(), SecretStreamError>`
- `MutablePutResult::commit_timeouts: u32`
- `should_direct_connect(relayed, firewall, remote_holepunchable, same_host) -> bool`
- `Io::server_socket(&self) -> Arc<UdxSocket>`
- `Io::primary_socket(&self) -> Arc<UdxSocket>`

### Breaking (see above)

- `PeerConnection.socket`: `UdxSocket` → `Arc<UdxSocket>`
- `establish_stream(...)`: added `shared_socket: Option<Arc<UdxSocket>>` parameter

---

## `libudx` (1.1.0 → 1.2.0)

### Breaking (see above)

- `UdxSocket::close(self)` → `UdxSocket::close(&self)`

### Changed (internal, no external API impact)

- `UdxSocket` held as `Arc<UdxSocket>` internally for socket multiplexing
- `UdxAsyncStream`: FIN/shutdown now queues a FIN packet and waits for ACK

---

## Version Impact Summary

| Crate | Current | Strict semver | Notes |
|---|---|---|---|
| `libudx` | 1.2.0 | **2.0.0** | `close` receiver change |
| `peeroxide-dht` | 1.2.0 | **2.0.0** | `PeerConnection.socket` type + `establish_stream` sig |
| `peeroxide` | 1.2.0 | 1.2.0 ✅ | additive only |

The two `peeroxide-dht` breaking symbols (`PeerConnection`, `establish_stream`) are
not re-exported at the crate root. Whether this constitutes a "public" API break is
a policy decision.
