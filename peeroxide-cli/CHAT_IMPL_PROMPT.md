# peeroxide-chat Implementation Prompt

> **Status**: working / historical implementation prompt used while building the chat subsystem. **Not user-facing documentation.** This file is proposed for removal — see the PR description's Working Files table. The canonical user-facing documentation lives in [`docs/src/chat/`](../docs/src/chat/).

## Context

Implement the `peeroxide chat` subcommand — an anonymous, verifiable P2P chat system built entirely on existing peeroxide DHT primitives (no protocol changes, no C dependencies). The full protocol spec is in `peeroxide-cli/CHAT.md` and the CLI design is in `peeroxide-cli/CHAT_CLI.md`. Read both thoroughly before starting.

The system uses per-participant mutable feeds as message pointers, immutable_put for message storage, epoch-rotating announce topics for discovery, and XSalsa20-Poly1305 encryption with Ed25519 signatures. All crypto uses pure Rust crates already in the dependency tree.

---
CRITICAL NOTE: Use `df -h` before every `cargo build` to monitor free space on volume /System/Volumes/Data and ALWAYS perform `cargo clean` when usage is above ~87%.
---

## Ordered Task List

### Phase 1: Foundation (no dependencies between tasks within phase)

#### Task 1.1: Profile Storage
**What**: Implement profile directory management (`~/.config/peeroxide/chat/profiles/<name>/`)
**Files**:
- Create `peeroxide-cli/src/cmd/chat/profile.rs`
**Build**:
- Read/write `seed` (32 bytes), `name`, `bio` files
- Derive `id_keypair` from seed (Ed25519)
- Create profile directory + generate random seed on first use
- List/create/delete profiles
- Read/write `friends` file (tab-separated: `pubkey\talias\tcached_name\tcached_bio_line`)
- Read/write `known_users` file (append-only: `pubkey\tscreen_name`)
- Dedup on read (latest entry per pubkey wins)
**Acceptance**: `cargo test` — unit tests for profile CRUD, file format parsing, dedup logic
**Dependencies**: None

#### Task 1.2: Key Derivation
**What**: Implement all KDF functions from CHAT.md Track 2
**Files**:
- Create `peeroxide-cli/src/cmd/chat/crypto.rs`
**Build**:
- `channel_key(name, salt)` — BLAKE2b-256 with `len4()` length prefixes
- `announce_topic(channel_key, epoch, bucket)` — keyed BLAKE2b
- `msg_key(channel_key)` — keyed BLAKE2b
- `dm_channel_key(id_a, id_b)` — sorted pubkeys
- `dm_msg_key(ecdh_secret, channel_key)` — keyed BLAKE2b
- `inbox_topic(recipient_id_pubkey, epoch, bucket)` — keyed BLAKE2b
- `invite_key(ecdh_secret, invite_feed_pubkey)` — keyed BLAKE2b
- Ed25519↔X25519 conversion (public: birational map; private: SHA-512 first 32 bytes, clamped)
- `ownership_proof(id_sk, feed_pubkey, channel_key)` — Ed25519 sign
- `len4(x)` = `(x.len() as u32).to_le_bytes()`
**Acceptance**: Unit tests with known test vectors; round-trip verify for ownership proofs; ECDH shared secret matches between two keypairs
**Dependencies**: None

#### Task 1.3: Wire Formats
**What**: Serialize/deserialize all record types from CHAT.md §7.1–§7.5
**Files**:
- Create `peeroxide-cli/src/cmd/chat/wire.rs`
**Build**:
- `MessageEnvelope` — plaintext struct (id_pubkey, prev_msg_hash, timestamp, content_type, screen_name, content, signature) + serialize/deserialize per §7.1 layout
- `FeedRecord` — struct (id_pubkey, ownership_proof, next_feed_pubkey, summary_hash, msg_count, msg_hashes) + serialize/deserialize per §7.2
- `SummaryBlock` — struct + serialize/deserialize per §7.3
- `NexusRecord` — struct (name, bio) + serialize/deserialize per §7.4
- `InviteRecord` — plaintext struct (id_pubkey, ownership_proof, next_feed_pubkey, invite_type, payload) + serialize/deserialize per §7.5
- Encryption/decryption wrappers: `encrypt_message(key, plaintext) -> ciphertext`, `decrypt_message(key, ciphertext) -> plaintext`
- Invite encryption: `encrypt_invite(invite_feed_sk, recipient_pubkey, plaintext)`, `decrypt_invite(my_sk, invite_feed_pubkey, ciphertext)`
- Size validation (reject > 1000 bytes before put)
- Malformed record handling: return `Result`, never panic on bad input
**Acceptance**: Round-trip tests for all record types; size budget tests (max content fits in 1000 bytes); malformed input returns Err
**Dependencies**: Task 1.2 (crypto functions)

### Phase 2: Core Operations

#### Task 2.1: Message Posting (§8.2)
**What**: Build → encrypt → immutable_put → update feed → announce
**Files**:
- Create `peeroxide-cli/src/cmd/chat/post.rs`
**Build**:
- Sign message (sign-then-encrypt per §8.2 steps 1-8)
- Encrypt with msg_key (or dm_msg_key)
- `immutable_put` encrypted envelope
- Prepend msg_hash to feed record
- Eviction check: if msg_count reaches 20, trigger summary block publish first (§8.4)
- `mutable_put` updated feed record
- `announce` on current epoch topic
- All ordering constraints enforced (immutable_put completes before mutable_put)
**Acceptance**: Integration test — post a message, verify immutable_put contains valid encrypted envelope, feed record updated with new hash
**Dependencies**: Tasks 1.2, 1.3

#### Task 2.2: Message Reading
**What**: Discover feeds → poll → fetch → decrypt → verify → display
**Files**:
- Create `peeroxide-cli/src/cmd/chat/reader.rs`
**Build**:
- Cold-start scan: 20 epochs × 4 buckets (80 parallel lookups)
- Cascading fan-out: as feed_pubkeys discovered → spawn mutable_get → as msg_hashes found → spawn immutable_get
- Feed record validation: verify ownership_proof against channel_key
- Message decryption + signature verification
- `prev_msg_hash` chain validation (per-feed)
- `next_feed_pubkey` following (rotation handoff)
- `summary_hash` following (history fetch, cap 100 blocks)
- Adaptive polling: back off quiet feeds after 3 unchanged cycles
- Known-feed set management (max ~100, deprioritize stale, remove after TTL expiry)
- Never mark hash "seen" until decrypt+verify succeeds
- Steady-state: scan current + previous epoch (8 lookups, every 5-8s)
**Acceptance**: Integration test — two instances on same channel, one posts, other receives and decrypts correctly
**Dependencies**: Tasks 1.2, 1.3, 2.1

#### Task 2.3: Feed Management
**What**: Feed refresh, rotation, summary blocks
**Files**:
- Create `peeroxide-cli/src/cmd/chat/feed.rs`
**Build**:
- Feed refresh: re-mutable_put every ~8 min (even if unchanged, increment seq)
- Feed rotation (§8.3): generate new keypair → publish new feed record (seq=0) → update old feed with next_feed_pubkey → overlap one refresh cycle
- Summary block publish (§8.4): evict oldest 15 when count reaches 20, immutable_put summary, update feed record
- Configurable lifetime with ±50% random wobble
- Bucket permutation: random [0,1,2,3] per feed keypair, cycle sequentially
**Acceptance**: Unit test for rotation logic; integration test verifying readers follow next_feed_pubkey handoff
**Dependencies**: Tasks 1.2, 1.3

### Phase 3: DM & Invite System

#### Task 3.1: DM Channel
**What**: Deterministic DM channel + ECDH encryption
**Files**:
- Create `peeroxide-cli/src/cmd/chat/dm.rs`
**Build**:
- Derive DM channel_key from sorted pubkeys
- Derive dm_msg_key via X25519 ECDH
- Reuse reader/poster from Phase 2 with dm_msg_key
- Always create invite_feed_keypair on DM start (regardless of --message)
- Publish real DM feed record BEFORE invite (pointer-before-target rule)
**Acceptance**: Integration test — two DM instances derive same channel, exchange encrypted messages
**Dependencies**: Tasks 2.1, 2.2, 2.3

#### Task 3.2: Invite Inbox
**What**: Send and receive invites (DM nudges + group invites)
**Files**:
- Create `peeroxide-cli/src/cmd/chat/inbox.rs`
**Build**:
- **Sending** (DM startup nudge):
  - Build invite record per §7.5
  - Encrypt under recipient's X25519 pubkey using invite_feed_keypair for ECDH
  - mutable_put + announce on recipient's inbox topic
  - Re-announce across epochs (default 20 min / one TTL cycle)
- **Sending** (per-message nudge):
  - Same invite_feed_keypair, increment seq
  - Payload = triggering message text (truncated to fit)
  - Max once per epoch
- **Receiving**:
  - Poll inbox topics (current + previous epoch × 4 buckets, every 15-30s)
  - Decrypt with own X25519 private key + invite_feed_pubkey
  - Verify ownership proof (try DM key first, then group key from payload)
  - Dedup: track seen invite_feed_pubkeys; higher seq = refresh, don't re-display
  - Sender name resolution: nexus lookup → known_users → shortkey fallback
- Display: `[INVITE #N] DM from name (shortkey)` with lure text + copy-paste command
**Acceptance**: Integration test — Alice sends DM invite, Bob's inbox receives and displays correct copy-paste command
**Dependencies**: Tasks 1.2, 1.3, 3.1

#### Task 3.3: Nexus & Friends
**What**: Personal nexus publishing + friend refresh loop
**Files**:
- Create `peeroxide-cli/src/cmd/chat/nexus.rs`
**Build**:
- Nexus record: serialize name+bio per §7.4, mutable_put under id_keypair
- Seq = unix_time_secs (u64)
- Refresh every ~8 min
- Friend refresh: round-robin mutable_get of friends' nexus records, one per poll cycle
- Update cached screen names/bios in friends file
- `--no-nexus` disables publishing; `--no-friends` disables refresh
- `nexus --lookup <pubkey>` one-shot fetch
- `nexus --set-name` / `--set-bio` write to profile files
- `nexus --daemon` long-running: publish own + refresh friends
**Acceptance**: Unit test for nexus serialization; integration test for publish + lookup round-trip
**Dependencies**: Tasks 1.1, 1.2, 1.3

### Phase 4: CLI Integration

#### Task 4.1: Command Dispatch & Main Loops
**What**: Wire everything into the `peeroxide chat` subcommand tree
**Files**:
- Create `peeroxide-cli/src/cmd/chat/mod.rs` — subcommand dispatch
- Create `peeroxide-cli/src/cmd/chat/join.rs` — `chat join` main loop
- Create `peeroxide-cli/src/cmd/chat/dm_cmd.rs` — `chat dm` main loop
- Create `peeroxide-cli/src/cmd/chat/inbox_cmd.rs` — `chat inbox` main loop
- Modify `peeroxide-cli/src/main.rs` — add `chat` subcommand
**Build**:
- `chat join`: setup → cold-start scan → display backlog → "*** — live —" → main loop (discovery + polling + stdin + refresh + rotation)
- `chat dm`: same as join but DM channel + invite_feed_keypair + nudge logic
- `chat inbox`: polling loop + display + dedup
- `chat whoami`: print profile info (full 64-char pubkey)
- `chat profiles`: list/create/delete
- `chat friends`: list/add/remove/refresh
- `chat nexus`: set/lookup/publish/daemon
- Flag handling: `--read-only` (skip feed creation, disable posting), `--stealth` (= --no-nexus --read-only --no-friends), `--no-nexus`, `--no-friends`
- `--group` / `--keyfile` mutual exclusivity (error if both)
- `--message` silently ignored in `--read-only` mode
- EOF on stdin → read-only mode; Ctrl-C → exit
**Acceptance**: `cargo build` succeeds; `peeroxide chat --help` shows all subcommands; `peeroxide chat join test-channel` connects and enters main loop
**Dependencies**: All Phase 2 and 3 tasks

#### Task 4.2: Display Formatting
**What**: Message display, trust indicators, system messages
**Files**:
- Create `peeroxide-cli/src/cmd/chat/display.rs`
**Build**:
- Timestamp formatting: `YYYY-MM-DD HH:MM:SS` (date omitted if today)
- Display name resolution: friend alias → screen_name from message → shortkey fallback
- Trust brackets: `[()]` for friends, `[<>]` for non-friends
- Friend without alias: `[(screen_name)]`
- `!` prefix for recent name changes (10 min cooldown)
- Identity system messages (full pubkey, >10 min since last shown for that user)
- Friends with alias: no identity lines. Friends without alias: identity lines on schedule.
- System events: join, rotation, connection, name change, `*** — live —` separator
- Cold-start backlog: sort by timestamp, display chronologically, then separator
**Acceptance**: Unit tests for display formatting with all trust combinations
**Dependencies**: Task 1.1 (profile/friends data)

### Phase 5: Testing

#### Task 5.1: Unit Tests
**What**: Comprehensive unit tests for all modules
**Files**: Test modules within each source file + `peeroxide-cli/tests/chat_unit.rs`
**Build**:
- Crypto: test vectors for all KDFs, ECDH, Ed25519↔X25519 conversion
- Wire: round-trip for all record types, boundary sizes, malformed input
- Profile: CRUD, file format, concurrent append simulation
- Display: all trust bracket combinations, name change cooldown
**Acceptance**: `cargo test -p peeroxide-cli` all green
**Dependencies**: All Phase 1-4 tasks

#### Task 5.2: Integration Tests
**What**: Multi-instance tests using local DHT
**Files**: `peeroxide-cli/tests/chat_integration.rs`
**Build**:
- Two instances join same channel, exchange messages
- DM between two instances (both directions)
- Invite send + receive
- Feed rotation with reader following handoff
- Summary block eviction + history fetch
- `--read-only` mode (no writes observed)
- Nexus publish + lookup
**Acceptance**: `cargo test -p peeroxide-cli --test chat_integration` all green
**Dependencies**: All Phase 1-4 tasks

---

## Constraints & Gotchas

1. **API Breaking Change Policy**: Do NOT modify any existing public API in `libudx`, `peeroxide-dht`, or `peeroxide`. All chat code lives in `peeroxide-cli`. If you need something from the library crates, add a NEW non-breaking method or use existing APIs creatively.

2. **Ordering invariant**: ALWAYS complete pointer-target writes before publishing records containing those pointers. Specifically:
   - `immutable_put(message)` must complete before `mutable_put(feed_record)` referencing it
   - `immutable_put(summary_block)` must complete before `mutable_put(feed_record)` with new summary_hash
   - `mutable_put(new_feed_record)` must complete before `mutable_put(old_feed_record)` with next_feed_pubkey
   - `mutable_put(real_dm_feed)` must complete before `mutable_put(invite_feed)` pointing to it

3. **1000-byte budget**: The DHT library does NOT enforce this — it's a client-side convention. Validate all record sizes before put. libudx transport MAX_PAYLOAD ≈ 1200 - header, so 1000 is conservative and correct.

4. **Encryption**: XSalsa20-Poly1305 with random 24-byte nonce. Wire format: `nonce(24) || tag(16) || ciphertext`. Use `xsalsa20poly1305` crate (already a dep via `peeroxide-dht/src/secure_payload.rs` pattern).

5. **Ed25519↔X25519**: Public key conversion via `curve25519_dalek` (Edwards → Montgomery birational map). Private key: SHA-512(seed)[0..32], clamped. This matches libsodium's `crypto_sign_ed25519_*_to_curve25519`.

6. **Feed records are plaintext** — `id_pubkey` visible to DHT nodes. This is an accepted trade-off documented in the threat model.

7. **Invite records are encrypted** — entire mutable_put value is ciphertext. ECDH uses invite_feed_keypair (not identity keypair).

8. **Screen name lives in encrypted message payload** (not feed record). Added as a field in §7.1 wire format. Overhead is 180 bytes, max screen_name + content = 820 bytes.

9. **Summary eviction**: Trigger at 20 hashes, evict oldest 15, keep newest 5. Eviction operates on existing hashes; new message prepended afterward.

10. **No persistent state**: All runtime state (known feeds, message cache, seq numbers) is in-memory only. Profile files on disk are the only persistence.

11. **Nexus seq = unix_time_secs**: Multi-device collision is acceptable (same-second = same content, harmless). Clock skew = lower seq silently dropped.

12. **`hash_batch` has no internal framing**: It concatenates slices. Use `len4(x)` before variable-length inputs to prevent ambiguity.

13. **MSRV**: Rust 1.85 (2024 edition). Use `tokio` for async.

---

## Test Strategy

1. **Unit tests** (Phase 5.1): Pure logic — crypto, wire formats, display formatting. No network. Fast.
2. **Integration tests** (Phase 5.2): Spin up local DHT nodes (use existing test infrastructure from `peeroxide-dht`), run multiple chat instances, verify end-to-end message flow.
3. **Both test suites must pass**: `cargo test --workspace` (includes chat tests) before marking complete.
4. **Clippy clean**: `cargo clippy --workspace` with no warnings.

---

## File Structure (Final)

```
peeroxide-cli/src/cmd/chat/
├── mod.rs          — subcommand dispatch (join, dm, inbox, whoami, profiles, nexus, friends)
├── crypto.rs       — KDF, ECDH, Ed25519↔X25519, ownership proofs
├── wire.rs         — serialize/deserialize all record types + encryption wrappers
├── profile.rs      — profile directory management, friends, known_users
├── post.rs         — message posting (build → encrypt → publish)
├── reader.rs       — discovery + polling + fetch + decrypt + verify
├── feed.rs         — feed refresh, rotation, summary blocks
├── dm.rs           — DM channel derivation + ECDH
├── inbox.rs        — invite send/receive
├── nexus.rs        — personal nexus + friend refresh
├── join.rs         — `chat join` main loop
├── dm_cmd.rs       — `chat dm` main loop
├── inbox_cmd.rs    — `chat inbox` main loop
├── display.rs      — message formatting, trust indicators, system messages
```
