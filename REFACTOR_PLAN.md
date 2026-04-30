# Ralph Loop: UdxSocket Handle Refactor

## Mission

Refactor `UdxSocket` in `libudx` from an owned type into a cheap-clone handle
(internally `Arc<UdxSocketInner>`) to eliminate three breaking public API changes
introduced on `feat/peeroxide-cli` vs `main`:

1. `UdxSocket::close(self)` → `close(&self)` — restore consuming signature
2. `PeerConnection.socket: Arc<UdxSocket>` → `UdxSocket` — restore original field type
3. `server_socket()`/`listen_socket()` returning `Arc<UdxSocket>` → return `UdxSocket`

The third breaking change (`establish_stream` extra param) is handled separately
via an additive compatibility wrapper — NOT in this loop.

## Hard Constraints

- All existing tests must pass: `cargo test --workspace`
- All originally surfaced functionality must be preserved
- No functional regressions — only the internal representation changes
- Git commit after each completed phase checkpoint
- Do not push to remote

## Checkpoint Convention

```
[ ]  not started
[/]  in progress
[X]  complete and tested
```

---

## Phase 1 — Understand current UdxSocket internals

- [ ] Read `libudx/src/native/socket.rs` in full
- [ ] Read `libudx/src/lib.rs` to understand what is re-exported
- [ ] Inventory all fields on `UdxSocket` that need to move to `UdxSocketInner`
- [ ] Identify the `Drop` impl (if any) and `recv_task` ownership
- [ ] Commit checkpoint: `refactor: [1/5] inventory UdxSocket fields`

## Phase 2 — Refactor UdxSocket into handle type

- [ ] Create `UdxSocketInner` struct with all current `UdxSocket` fields
- [ ] Change `UdxSocket` to `pub struct UdxSocket { inner: Arc<UdxSocketInner> }`
- [ ] Move `Drop` logic to `UdxSocketInner` (recv_task abort on last drop)
- [ ] Implement `Clone` for `UdxSocket` (cheap Arc clone)
- [ ] Move `close_impl(&self)` body; restore `pub async fn close(self)`
- [ ] Fix all internal `self.field` → `self.inner.field` references in socket.rs
- [ ] `cargo build -p libudx` must pass
- [ ] Commit checkpoint: `refactor: [2/5] UdxSocket as Arc handle type`

## Phase 3 — Fix libudx callers inside libudx

- [ ] Fix `async_stream.rs` — any direct field access or Arc wrapping
- [ ] Fix stream.rs — PendingWrite / StreamInner references
- [ ] Fix UdxRuntime / create_socket return type if needed
- [ ] `cargo test -p libudx` must pass (all tests green)
- [ ] Commit checkpoint: `refactor: [3/5] fix libudx internal callers`

## Phase 4 — Fix peeroxide-dht callers

- [ ] Replace all `Arc<UdxSocket>` fields/locals with `UdxSocket` (use `.clone()`)
- [ ] Revert `PeerConnection.socket` to `pub socket: UdxSocket`
- [ ] Fix `server_socket()`/`listen_socket()` to return `UdxSocket` not `Arc<UdxSocket>`
- [ ] Fix `io.rs`, `rpc.rs`, `hyperdht.rs`, `socket_pool.rs`, `holepuncher.rs`
- [ ] `cargo build -p peeroxide-dht` must pass
- [ ] Commit checkpoint: `refactor: [4/5] fix peeroxide-dht callers`

## Phase 5 — Fix peeroxide + peeroxide-cli callers, full test suite

- [ ] Fix any `Arc<UdxSocket>` in `peeroxide/src/`
- [ ] Fix any `Arc<UdxSocket>` in `peeroxide-cli/src/`
- [ ] `cargo test --workspace` — all tests must pass
- [ ] Verify `UdxSocket::close(self)` signature is restored
- [ ] Verify `PeerConnection.socket: UdxSocket` is restored
- [ ] Verify `server_socket()`/`listen_socket()` return `UdxSocket`
- [ ] Update `API_CHANGES.md` — mark breaks as resolved
- [ ] Commit checkpoint: `refactor: [5/5] full workspace green, breaking APIs restored`

---

## Recovery

If interrupted, read this file and the git log to find the last completed phase,
then continue from the next `[ ]` item. Run `cargo build --workspace` to assess
current state before resuming.
