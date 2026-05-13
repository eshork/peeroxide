# peeroxide-chat: Design Notes

> **Status**: working / historical design document used while building the chat subsystem. **Not user-facing documentation.** This file is proposed for removal — see the PR description's Working Files table. The canonical, current chat documentation (covering the shipped wire format, key derivation, protocol, TUI, CLI, and reference constants) lives in [`docs/src/chat/`](../docs/src/chat/).

> Working design document for an anonymous, verifiable P2P chat system built
> entirely on top of the existing peeroxide DHT stack — no protocol changes,
> no custom relay work, no cooperation required from arbitrary peers.

---

## Core Requirements

1. **Source IP anonymity** — no message is traceable back to the sender's IP
   from a network perspective. The adversary can be at any point in transit,
   including being a chat participant.
2. **Verifiable authorship** — every message is signed; recipients can prove
   a message came from a specific identity key.
3. **Content confidentiality** — all messages are encrypted. Only intended
   participants can decrypt. Author identity is hidden inside ciphertext.
4. **Ephemeral by default** — no permanent network storage. DHT TTL (~20 min)
   is acceptable. Local clients maintain in-memory message caches for session
   continuity (no persistent on-disk cache in v1).
5. **Pure DHT transport** — uses only existing peeroxide DHT operations
   (`announce`, `lookup`, `mutable_put`, `mutable_get`, `immutable_put`,
   `immutable_get`). No protocol changes, no custom relay code, no peer
   cooperation required.
6. **Two chat modes** — chatroom (group) and direct message (DM).

---

## Threat Model

### Adversary Goals

These are the things an adversary wants to achieve. They don't change
regardless of who the adversary is or how they're positioned.

1. **Unmask identity** — Link a chat identity (`id_pubkey`) to a real-world
   person via their IP address.
2. **Read unauthorized content** — Decrypt messages on channels or DMs the
   adversary is not part of.
3. **Map relationships** — Determine who is talking to whom (DM partners,
   channel membership).
4. **Correlate across channels** — Link the same person's activity across
   different channels, building a behavioral profile.
5. **Disrupt communication** — Prevent messages from being delivered
   (censorship, denial of service).
6. **Impersonate** — Post messages that appear to come from another identity.
7. **Enumerate channels** — Discover what channels exist on the network
   without knowing their names.
8. **Recover history** — Obtain past messages or activity patterns after
   the fact.

### The Core Principle

**Security is a function of channel type + identity hygiene.**

Public channels are public. The channel name IS the key. Security scales
with the secrecy of the channel and the discipline of the user's identity
management. The protocol provides the tools — multiple profiles,
cryptographic unlinkability, encrypted transport — but cannot prevent a
user from burning their own identity through careless usage.

### Usage Profile: Casual (single identity, public channels)

A user who uses one profile everywhere, participates in public channels,
and doesn't think about identity separation. This is the "lazy and open"
baseline — what you get with zero effort.

| Adversary Goal | Protection | How |
|---|---|---|
| 1. Unmask identity | **Strong vs participants; Medium vs DHT nodes** | No direct connections between participants. DHT store-and-forward only. No IP in any stored record. However, DHT nodes serving feed `mutable_put`/`mutable_get` see source IP + plaintext feed record containing `id_pubkey`. Epoch rotation and feed rotation limit exposure duration. |
| 2. Read content | **None for public channels** | Anyone who knows the channel name can derive the key and read everything. |
| 3. Map relationships | **None for public channels** | Your id_pubkey is visible in your feed record. Anyone who knows the channel name can enumerate all active participants' identities. |
| 4. Cross-channel correlation | **None** | Same id_pubkey everywhere = trivially linkable by anyone on any shared channel. |
| 5. Disrupt | **Weak** | DHT nodes can refuse to store records. Announce slots can be exhausted by spam. No redundancy beyond standard Kademlia replication (K-closest nodes). |
| 6. Impersonate | **Strong** | All messages are Ed25519 signed. Ownership proofs bind feeds to identities. Forgery requires the private key. |
| 7. Enumerate channels | **Strong (name recovery); Weak (existence)** | Topics are opaque BLAKE2b hashes. No directory. Must know the name to find it. But common/guessable names can be brute-forced, and DHT nodes can observe active topic hashes without knowing what they represent. |
| 8. Recover history | **Medium (network-side)** | 20-min TTL. Messages expire from DHT if not refreshed. But any participant or DHT node that captured records earlier can keep them indefinitely via re-`immutable_put`. Local client caches also persist. |

**Summary**: Casual usage gives you **participant-to-participant IP anonymity**
(no other chatter learns your IP) and **impersonation resistance** (no one can
forge your messages). You do NOT get identity privacy — your participation in
public channels is visible to anyone who knows the channel names. DHT nodes
serving your feed can correlate your IP with your identity within their
observation window.

### Usage Profile: Careful (dedicated identities, private channels/DMs)

A user who creates a dedicated profile for sensitive communications, uses
only private channels or DMs, and never uses that profile on public channels.
This is what you CAN get with disciplined operational security.

| Adversary Goal | Protection | How |
|---|---|---|
| 1. Unmask identity | **Strong vs participants; Medium vs DHT nodes** | Same as casual — IP never exposed to other participants. DHT nodes serving feeds still see source IP + `id_pubkey` in plaintext feed record. Mitigated by: feed rotation limits observation window. If personal nexus is enabled, `hash(id_pubkey)` is a stable address — use `--no-nexus` for maximum privacy. |
| 2. Read content | **Strong (message bodies); Weak (feed metadata)** | Private channel requires name + salt (or keyfile). DMs require ECDH. Brute-force infeasible with high-entropy salt. But feed records are unencrypted — `id_pubkey` and message-hash structure visible to feed-serving nodes. |
| 3. Map relationships | **Strong vs outsiders; Medium vs DHT nodes** | Channel topic unguessable without the salt. Outsiders cannot discover the channel. Feed-serving DHT nodes see `id_pubkey` in plaintext feed records. Invite inbox reveals only opaque feed_pubkeys and encrypted payloads to serving nodes. |
| 4. Cross-channel correlation | **Strong (between profiles); Medium (within one profile)** | Different profiles = unique id_pubkey, cryptographically unlinkable. But one profile used across multiple private channels is linkable wherever those feed records are discovered (same `id_pubkey` appears in each). |
| 5. Disrupt | **Weak** | Same as casual — DHT-level censorship resistance is minimal. |
| 6. Impersonate | **Strong** | Same as casual — Ed25519 signatures. |
| 7. Enumerate channels | **Strong (name recovery); Weak (existence)** | Private channel topics require the salt to compute. Cannot be discovered by scanning. But DHT nodes can still observe opaque active topic hashes. |
| 8. Recover history | **Medium (network-side)** | Same network-side ephemerality. TTL protects against late observers who captured nothing earlier. Does not protect against participants or nodes that archived records during the active window. |

**Summary**: Careful usage gives you participant-to-participant IP anonymity +
identity privacy (from outsiders) + message-body confidentiality + relationship
hiding (from outsiders). DHT nodes serving your feeds still observe source IP
and plaintext feed metadata within their rotation window. The realistic attack
vectors are: key compromise (stolen seed file), traffic analysis by a global
passive observer, Sybil nodes near your feed/inbox targets, DHT-level
censorship, or low-entropy channel secrets enabling brute-force.

### Residual Risks (Both Profiles)

These apply regardless of usage discipline:

- **Personal nexus is an opt-in privacy trade-off.** When enabled (the
  default), `mutable_put(id_keypair, ...)` gives `id_pubkey` a stable DHT
  address (`hash(id_pubkey)`). The K-closest nodes serving that address can
  correlate IP↔identity for as long as the nexus is refreshed (~8 min
  intervals). Users requiring source-IP anonymity from DHT nodes should use
  `--no-nexus`. This does NOT affect participant-to-participant anonymity
  (no direct connections regardless).
- **DHT nodes see your IP** when you make requests (inherent to UDP). They
  can correlate "IP X operated on topic Y at time T" within a single epoch.
  Epoch rotation limits this to 1-minute windows for discovery, but feed
  polling persists for the feed's lifetime. **For best IP protection, run
  peeroxide-chat behind a VPN or self-hosted relay.** This is the single
  most effective mitigation against DHT-node-level traffic analysis and is
  strongly recommended for careful-profile users.
- **Feed-serving nodes see plaintext metadata.** Nodes handling `mutable_put`/
  `mutable_get` for your feed see `id_pubkey`, message hashes, and can
  correlate with source IP. Feed rotation limits the observation window.
- **Ownership proof is an offline verification oracle.** An adversary who
  obtains a feed record can test candidate channel keys against the ownership
  proof signature. Harmless for high-entropy keyfiles; a risk for guessable
  channel names/salts.
- **Traffic analysis by a global passive observer** can potentially correlate
  writers and readers through timing. This is a fundamental limit of
  store-and-forward without onion routing.
- **Sybil attacks** — an adversary running many DHT nodes increases their
  observation coverage across feeds, inboxes, and announce topics.
- **No forward secrecy** (v1) — DM keys are static ECDH. Key compromise
  allows decryption of past messages (including archived ciphertext).
- **No censorship resistance** — DHT nodes can refuse to store records.
  Announce slots can be exhausted by spam.
- **No deniability** — messages are signed. Signatures are proof of authorship.
  This is deliberate (verifiable authorship is a core requirement).
- **Local client caches** are in-memory for v1 (lost on exit). Future
  persistent caches would survive beyond DHT TTL — physical access to a
  device would then mean access to chat history.
- **Ephemerality is not enforceable** — any participant or DHT node that
  captured immutable records can re-`immutable_put` them to keep them alive
  indefinitely. TTL is "default retention on honest nodes," not guaranteed
  deletion.
- **Message ordering is approximate** — cross-feed ordering relies on
  untrusted timestamps. Different readers may render different orderings.
  Per-feed ordering is reliable (via `prev_msg_hash` chain).

---

## Architecture Overview

### Why Pure DHT (No Direct Connections)

Direct peer connections (`connect()`) expose the caller's IP to the remote
peer. To achieve source IP anonymity without a custom onion-routing layer
(which requires peer cooperation), we use the DHT itself as a
store-and-forward message bus:

- Sender writes messages to DHT -> DHT nodes see sender IP, but not content
- Readers poll DHT for messages -> DHT nodes see reader IP, but not who they're reading
- **Sender and reader IPs are never exposed to each other**

DHT nodes that handle the operations see the source IP of each request, and:
- Cannot read message content (encrypted with channel/DM key)
- Cannot link the announce topic hash to a channel name (requires the channel key)
- CAN see plaintext feed records (including `id_pubkey`) when serving
  `mutable_put`/`mutable_get` — this links source IP to identity for the
  K-closest nodes handling that feed. Epoch rotation and feed rotation
  limit the duration of this exposure but do not eliminate it.

### Per-Participant Feed Model

Each participant maintains **one mutable_put "feed"** per channel they're active
in. Messages themselves are stored via `immutable_put` (content-addressed,
immutable). The feed acts as a pointer to the participant's latest messages.

The system has two distinct layers:

**Discovery layer** (announce/lookup): Epoch-rotating announce topics signal
"I have new content." A participant announces their feed_pubkey on an
epoch+bucket topic when they post a message. Readers scan these topics to
discover active posters. Announce is NOT idle presence — you only announce
when you have something new to say.

**Content layer** (mutable_put/immutable_put): Feed records and messages.
Feed keypairs are random and rotated by the client to prevent long-term
traffic monitoring. Once a reader discovers a feed_pubkey through the
announce layer, they poll it directly via `mutable_get` until it goes stale.

```
EPOCH+BUCKET TOPICS (announce/lookup) -- rotate every minute
  |
  +-- epoch 1042, bucket 0:  lookup -> [alice_feed_pubkey]
  +-- epoch 1042, bucket 1:  lookup -> [bob_feed_pubkey]
  +-- epoch 1042, bucket 2:  lookup -> []
  +-- epoch 1042, bucket 3:  lookup -> [carol_feed_pubkey]

FEED ADDRESSES (mutable_put/get) -- random per session, client rotates
  |
  +-- Alice's feed @ hash(alice_feed_pubkey)  [this session]
  |     -> points to her recent message hashes
  +-- Bob's feed @ hash(bob_feed_pubkey)  [this session]
  |     -> points to his recent message hashes
  +-- Carol's feed @ hash(carol_feed_pubkey)  [this session]
        -> points to her recent message hashes

MESSAGES (immutable_put/get) -- content-addressed, immutable
  +-- hash(msg1) -> encrypted message content
  +-- hash(msg2) -> encrypted message content
  ...
```

**Why this model**:
- Announce topics rotate every epoch (1 minute) — different DHT nodes handle
  discovery in different time windows. No single set of nodes builds a
  persistent traffic profile for a channel.
- 4 buckets per epoch — 80 announce slots per epoch per node (4 × 20).
  Handles burst posting scenarios.
- Feed records are random per session, rotated by the client — once
  discovered, a feed_pubkey is polled directly until it goes stale.
  Rotation prevents long-term traffic monitoring of any single address.
- Messages are immutable_put — content-addressed, can't be altered, anyone
  can re-put to refresh TTL (Good Samaritan persistence).
- Feed record contains up to 26 recent message hashes — readers can fetch all
  new messages in parallel instead of sequential linked-list walking.

### Two-Layer Security Model

```
+----------------------------------------------------------------------+
|  CONTENT LAYER (what is said, and who said it)                       |
|                                                                      |
|  All messages encrypted (XSalsa20Poly1305, random 24-byte nonce)     |
|  Chatroom:  encrypt(signed_msg, KDF(channel_key))                   |
|  DM:        encrypt(signed_msg, ECDH(sender_sk, recipient_pk))      |
|                                                                      |
|  -> Only intended recipients can decrypt                             |
|  -> Signature proves authorship (verifiable identity)                |
|  -> Author identity hidden inside ciphertext (not in feed metadata) |
+----------------------------------------------------------------------+
|  TRANSPORT LAYER (where messages are stored and found)               |
|                                                                      |
|  Announce:     feed keypair on epoch+bucket topic (new content signal)|
|  Mutable put:  feed record (message pointers) at stable address     |
|  Immutable put: individual encrypted messages                       |
|                                                                      |
|  -> Feed keypair is random, unlinked to author identity             |
|  -> No IP address in announce records (confirmed in code)            |
|  -> Announce topics rotate every epoch -- no persistent DHT target  |
+----------------------------------------------------------------------+
```

---

## Track 1: Participant Identity

### Two-Keypair Model

Each user has two kinds of keypairs:

**Identity keypair** (`id_keypair`): Long-term, persistent across all channels
and devices. The public key IS the identity. Used ONLY to sign message
content (inside encryption), ownership proofs, and the personal nexus record.
Never used for DHT transport operations (announce, mutable_put, etc.)
**except** the personal nexus record (one mutable slot, opt-out via `--no-nexus`).

**Per-channel feed keypair** (`feed_keypair`): Random, generated per session
(or rotated by the client on a configurable schedule). Used for `announce`
and `mutable_put` on a specific channel. Not derived from the identity key —
feed rotation is a client-side privacy decision.

```
Identity keypair   -> signs message content (inside encryption)
                   -> signs personal nexus record (mutable_put under id_keypair — opt-out via --no-nexus)
                   -> signs ownership proofs (including invite feed proofs)
                   -> NEVER used for channel announce or channel feed mutable_put

Feed keypair       -> used for announce + mutable_put (per channel)
                   -> random per session, rotated by client
                   -> bound to identity via ownership proof in feed record
                   -> also used for temporary invite feeds (same machinery)
```

### Profiles (Multiple Identities)

Users can maintain multiple named profiles, each with its own identity keypair:

```
~/.config/peeroxide/chat/profiles/
  +-- default/
  |   +-- seed       # Ed25519 seed (32 bytes, plaintext for v1)
  |   +-- name       # Optional display name
  +-- work/
  |   +-- seed
  |   +-- name
  +-- throwaway/
      +-- seed
      +-- name
```

- Default profile used if `--profile` not specified
- Same profile across channels = same identity (provably, via signatures)
- Different profiles = cryptographically unlinkable
- Key storage: plaintext Ed25519 seed on disk for v1 (assumes full disk encryption)
- Key rotation: out of scope for v1

### Personal Nexus

Each identity has a "nexus" record — a profile page stored at
`mutable_put(id_keypair, ...)`. Contains screen name, bio, and is signed
by the identity key. This is the only use of the identity keypair's mutable
slot. Addressed by `hash(id_pubkey)` — anyone who knows a user's pubkey can
look up their profile.

---

## Track 2: Key Derivation

All derivations use **BLAKE2b-256** — both unkeyed (`hash()`, `hash_batch()`)
and keyed (`Blake2bMac`, same pattern as `discovery_key()`). No new
dependencies. No HKDF.

### Channel Key (root secret per channel)

`len4(x)` = `(x.len() as u32).to_le_bytes()` — a 4-byte little-endian
length prefix. Required because `hash_batch` is equivalent to hashing
the concatenation of all slices; without explicit lengths, different
splits of the same bytes would hash identically.

```rust
// Public channel
channel_key = hash_batch(&[b"peeroxide-chat:channel:v1:",
    len4(name), name.as_bytes()])

// Private channel (salt = group name or keyfile bytes)
channel_key = hash_batch(&[b"peeroxide-chat:channel:v1:",
    len4(name), name.as_bytes(),
    b":salt:", len4(salt), salt])

// DM (symmetric -- same for both parties)
channel_key = hash_batch(&[b"peeroxide-chat:dm:v1:",
    lex_min(id_a, id_b), lex_max(id_a, id_b)])
```

### Derived Values

```rust
// Announce topic -- epoch-rotating, 4 buckets per epoch
// epoch = unix_time_secs / 60 (1-minute epochs)
// bucket = 0..3 (poster picks randomly)
announce_topic = keyed_blake2b(key = channel_key,
    msg = b"peeroxide-chat:announce:v1:" || epoch_u64_le || bucket_u8)

// Message encryption key (channels only, NOT DMs)
msg_key = keyed_blake2b(key = channel_key, msg = b"peeroxide-chat:msgkey:v1")
```

### Feed Keypair (Client-Side Decision)

Feed keypair management is a **client implementation detail**, not a protocol
concern. The protocol only requires that a valid Ed25519 keypair is used for
`announce` and `mutable_put`, with a valid ownership proof in the feed record.

Reference implementation behavior:
- Generate a random feed keypair per session (`KeyPair::generate()`)
- User-configurable maximum keypair lifetime (e.g., `--feed-lifetime 60m`)
- Auto-rotate when lifetime exceeded, with ±50% random wobble to prevent
  predictable rotation timing
- On rotation: generate new keypair, set `next_feed_pubkey` in old feed record,
  then announce the new feed_pubkey on next post. Old feed is kept alive briefly
  (one extra refresh cycle) so readers can follow the handoff link.
- Old feed naturally expires from DHT (20-min TTL) after the overlap period

This means:
- No deterministic feed derivation (no KDF for feeds)
- Each device gets its own independent feed — no multi-device conflicts
- Readers unify messages by `id_pubkey` (inside encrypted payload), not by feed
- Feed rotation is invisible to the protocol — readers just discover whatever
  feed_pubkey you announce

### DM Encryption Key

DMs use X25519 ECDH instead of deriving from channel_key:

```rust
// Ed25519 -> X25519 conversion
// Public key: birational map (Edwards -> Montgomery), per RFC 7748 §4.1
//   Equivalent to crypto_sign_ed25519_pk_to_curve25519 in libsodium
//   In Rust: curve25519_dalek CompressedEdwardsY -> MontgomeryPoint
// Private key: SHA-512(ed25519_seed)[0..32], clamped per X25519 spec
//   Equivalent to crypto_sign_ed25519_sk_to_curve25519 in libsodium
//   In Rust: use ed25519_dalek ExpandedSecretKey, take scalar bytes

ecdh_secret = X25519(my_x25519_priv, their_x25519_pub)
dm_msg_key = keyed_blake2b(key = ecdh_secret,
    msg = b"peeroxide-chat:dm-msgkey:v1:" || channel_key)
```

Static ECDH — no forward secrecy for v1. Can add ephemeral key ratcheting later.

### Inbox Topic (Generalized Invite Inbox)

```rust
// Epoch-rotating, same scheme as channel announces
// Used for ALL channel invitations (DMs, private groups, etc.)
// epoch = unix_time_secs / 60, bucket = 0..3
inbox_topic = keyed_blake2b(key = hash(id_pubkey),
    msg = b"peeroxide-chat:inbox:v1:" || epoch_u64_le || bucket_u8)
```

### Security Gradient

| Mode | Find topic | Read messages | Security |
|------|-----------|---------------|----------|
| Public | Know channel name | Know channel name | Open (intentional) |
| Private (group name) | Know name + group | Know name + group | Social secret |
| Private (keyfile) | Have keyfile | Have keyfile | Cryptographic |
| DM | Know both pubkeys | Only the two parties (ECDH) | End-to-end encrypted |

---

## Track 3: Message Transport

### Posting a Message

1. Build message payload (author_pubkey, timestamp, content, prev_msg_hash
   [previous message from this feed], content_type, signature)
2. Encrypt with channel's msg_key (or dm_msg_key for DMs) using
   XSalsa20Poly1305 with a random 24-byte nonce
3. `immutable_put(encrypted_envelope)` -> returns `msg_hash`
4. Update feed record: `mutable_put(feed_keypair, updated_record, seq+1)`
   with new msg_hash added to the message hash list
5. Signal new content: `announce(announce_topic, feed_keypair, [])` on the
   current epoch, random bucket (0-3). This is the only time announce is
   used — it signals "I have something new."

### Reading Messages

Two-phase process: **discover** active feeds, then **poll** known feeds.

**Discovery** (scanning announce topics for new posters):
1. **On join/resume**: scan last 20 epochs (20 minutes of history) × 4 buckets
   = 80 lookups. One-time cost to catch up on recent activity.
2. **Steady-state**: scan current + previous epoch (2 epochs × 4 buckets = 8 lookups)
3. Each lookup returns feed_pubkeys of recent posters
4. Add any new feed_pubkeys to the local "known feeds" set for this channel

**Polling** (fetching content from known feeds):
1. For each known feed_pubkey: `mutable_get(feed_pubkey)` -> feed record
2. Compare seq number against cached version — skip if unchanged
3. Extract new message hashes (compare against locally cached set)
4. `immutable_get(hash)` for each new message (parallelizable)
5. Decrypt, verify signature, verify prev_msg_hash chain, display
6. If `next_feed_pubkey` is set: add new feed to known set (rotation handoff)
7. If `summary_hash` is set and not yet fetched: `immutable_get(summary_hash)`
   → verify signature, extract msg_hashes, fetch referenced messages.
   Follow `prev_summary_hash` chain for deeper history (cap at 100 blocks).
8. Never mark a hash "seen" until decrypt+verify succeeds; retry on next cycle

Once a feed_pubkey is discovered, it stays in the known set for the
channel session. Stale feeds (3+ consecutive polls with unchanged seq)
are deprioritized (reduced poll frequency). Feeds are removed from active
polling only after TTL expiry with no seq change (presumed dead). The reader
polls it directly via mutable_get without needing to re-discover it through
announce. Re-discovery through announce reactivates a deprioritized feed. This means the announce
layer handles discovery of new/active posters, while the content layer
delivers messages independently.

### Message Properties

- **Immutable**: stored via `immutable_put`, content-addressed by hash.
  Cannot be altered after posting.
- **Encrypted**: all messages encrypted, even on public channels. DHT nodes
  see only opaque ciphertext. Author identity is inside the encrypted payload.
- **Signed**: Ed25519 signature over plaintext fields (sign-then-encrypt).
  Readers verify after decryption.
- **Chained**: each message includes `prev_msg_hash` linking to the previous
   message posted from the same feed (not per-identity). This avoids forks
   when multiple devices post concurrently under the same identity. Readers
   can walk the chain per-feed; cross-feed ordering is approximate (by timestamp).
- **Refreshable**: anyone who has the immutable record can re-`immutable_put`
  it to refresh the TTL (Good Samaritan persistence).

### Feed Record

Each participant's feed (mutable_put) contains:
- `id_pubkey` — author's identity public key
- `ownership_proof` — cryptographic binding of feed_pubkey to id_pubkey
- `msg_hashes` — up to 26 recent message hashes (newest first)
- `summary_hash` — optional link to a summary block for older history
- `next_feed_pubkey` — optional; set before rotation to link old → new feed

Screen name is NOT in the feed record — it lives only inside encrypted
message payloads and the personal nexus record. This prevents DHT nodes
from building identity profiles from feed metadata.

The ownership proof is: `sign(id_secret, b"peeroxide-chat:ownership:v1:" ||
feed_pubkey || channel_key)`. This prevents feed spoofing — readers verify
the proof matches the id_pubkey before trusting the feed.

The feed record is **not encrypted** — readers need to parse it to
discover message hashes and verify ownership. The `id_pubkey` is visible
to anyone who can fetch the feed (requires knowing the feed_pubkey address).
Screen name is NOT included — it lives only inside encrypted messages.

Feed records must be refreshed every ~8 minutes via `mutable_put` with an
incremented seq (even if unchanged) to prevent TTL expiration.

### Summary Blocks

When a participant's feed reaches 20 message hashes (out of a maximum
26-hash capacity), older hashes are proactively evicted into **summary
blocks** stored via `immutable_put`. Each summary block contains up to 27 message hashes and a
`prev_summary` link to the next older block, forming a chain.

Summary blocks are signed by the identity key and linked from the feed
record's `summary_hash` field. This enables efficient history browsing:
fetch the summary chain, then parallel-fetch all referenced messages.

**Ordering requirement**: `immutable_put(summary_block)` must complete before
`mutable_put(feed_record)` that references it. This prevents readers from
encountering a feed that points to a not-yet-propagated summary.

### Size Budgets

| Record type | Max size | Overhead | Content budget |
|------------|---------|----------|---------------|
| Message (immutable_put) | 1000 bytes | 180 bytes (envelope + encryption + signature) | 820 bytes (screen_name + content) |
| Feed record (mutable_put) | 1000 bytes | 161 bytes (fixed fields) | 26 msg hashes |
| Invite feed record (mutable_put) | 1000 bytes | 171 bytes (encryption + fixed fields) | 829 bytes for invite payload |
| Summary block (immutable_put) | 1000 bytes | 129 bytes (header + signature) | 27 msg hashes |
| Personal nexus (mutable_put) | 1000 bytes | 3 bytes (fixed fields) | 997 bytes for name + bio |

### Encryption Details

- **AEAD**: XSalsa20Poly1305 (already used in `secure_payload.rs`)
- **Nonce**: random 24-byte (birthday-safe at 2^96; no nonce reuse risk)
- **Wire format**: `nonce(24) || tag(16) || ciphertext` (matches existing pattern)
- **Overhead**: 40 bytes per message (nonce + auth tag)
- **Public channels**: encrypted with key derivable from channel name.
  DHT nodes can't read without knowing the name. Anyone who knows the name
  can derive the key — same security as "public" with one layer of indirection.
- **Private channels**: encrypted with key derived from name + salt. Only
  people with both strings can decrypt.
- **DMs**: encrypted with ECDH-derived key. Only the two participants can decrypt.

---

## Track 4: Invite Inbox & Direct Messages

### DMs Are Private Channels

A DM between Alice and Bob is simply a **deterministic private 2-person
channel**. It works exactly like any other channel:
- Both generate their own `feed_keypair` for that channel (random per session)
- Both derive the same `channel_key` from their sorted pubkeys
- Both announce on the DM's epoch+bucket topics when they post (same
  rotation scheme as channels)
- Messages encrypted with ECDH-derived key (not channel_key)

The only difference from a group channel is the key derivation formula
and the encryption method (ECDH vs shared channel key).

### Invite Inbox (Generalized Channel Invitation)

The inbox is a **general-purpose invitation mechanism** for inviting any
user to any channel — DMs, private groups, or anything else. It uses the
exact same feed/announce/mutable_put machinery as the rest of the protocol.
There are no special cases.

**Inbox topic** (epoch-rotating, same scheme as channel announces):
```rust
inbox_topic = keyed_blake2b(
    key = hash(recipient_id_pubkey),
    msg = b"peeroxide-chat:inbox:v1:" || epoch_u64_le || bucket_u8
)
```

### Inbox Is Opt-In

Monitoring the invite inbox is **not required** for normal chat participation.
A user who never polls their inbox can still join channels they already know
the key for, participate in DMs coordinated out-of-band, and receive messages
on any channel they're already active in. The inbox only solves the cold-start
problem of "how does Bob learn Alice wants to talk to him?"

Clients may choose to not monitor the inbox at all, or to poll it infrequently,
for any reason — battery life, network traffic, user preference, etc. Mobile
clients in particular may disable inbox polling by default and only check on
user request.

### Invite Flow (Alice invites Bob to any channel)

1. **Alice computes the target `channel_key`** for the channel she's inviting
   Bob to:
   - DM: `hash_batch([b"peeroxide-chat:dm:v1:", lex_min(alice_id, bob_id), lex_max(alice_id, bob_id)])`
   - Private group: the normal channel key (name + salt/keyfile) she already
     knows as a member.

2. **Alice generates a temporary invite feed keypair** — random Ed25519,
   same as any other per-channel feed keypair. Short-lived.

3. **Alice builds an invite feed record** (see §7.5 for wire format):
   - `id_pubkey` — Alice's real identity public key
   - `ownership_proof` — `sign(alice_id_sk, b"peeroxide-chat:ownership:v1:" || invite_feed_pubkey || channel_key)`
   - `next_feed_pubkey` — Alice's real ongoing feed_pubkey for this channel
     (so Bob can immediately start polling the conversation)
   - `invite_type` — "dm" (0x01) or "private" (0x02)
   - `payload` — invite-type-specific: optional message for DMs,
     channel_name + salt + optional message for group invites

4. **Alice encrypts the invite payload** under Bob's X25519 public key
   (derived from Bob's Ed25519 id_pubkey, same conversion used for DM ECDH).
   The entire feed record value is encrypted — DHT nodes serving the invite
   feed see only opaque ciphertext. **This is mandatory, not optional.**

5. **Alice publishes**:
   ```
   mutable_put(invite_feed_keypair, encrypted_invite_record, seq=0)
   announce(bob_inbox_topic, invite_feed_keypair, [])
   ```
   She re-announces across subsequent epochs (client-side decision on
   duration, default: 20 minutes / one full TTL cycle) until she observes
   Bob's activity on the channel, or the duration expires.

### Bob's Side (Receiving Invites)

Bob polls his inbox topic periodically (same cadence as background channels):

1. Scan current + previous epoch × 4 buckets for his inbox topics
   (8 lookups per cycle, 15-30s interval)
2. For each new feed_pubkey discovered: `mutable_get(invite_feed_pubkey)`
3. **Decrypt** the feed record using his X25519 private key + the invite
   feed's X25519 public key (ECDH). If decryption fails → not for Bob
   (or spam); discard.
4. **Verify ownership proof** against the `id_pubkey` in the decrypted record.
5. **Discern channel type automatically**:
   - Compute the DM `channel_key` between Alice's `id_pubkey` and Bob's own.
   - Verify the ownership proof using that candidate key: if the signature
     over `(invite_feed_pubkey || candidate_channel_key)` verifies against
     Alice's `id_pubkey` → **DM invite**.
   - Otherwise → **group/private channel invite**. Use the provided
     `channel_name` + `salt` (from inside the encrypted payload) to derive
     the channel key, then verify the ownership proof against that key.
6. **Begin normal operation**: add Alice's real feed (via `next_feed_pubkey`)
   to the channel's known feeds and start polling.
7. Ignore invites for channels Bob is already participating in.

### Why This Design

- **Total uniformity**: every form of discovery (channels, DMs, group invites)
  uses identical feed/announce/mutable_put machinery. No special cases.
- **The long-term `id_keypair` is NEVER used for channel announce or channel
  feed mutable_put.** The only transport use is the personal nexus record
  (opt-out via `--no-nexus`). The last exception (old inbox announce) is
  eliminated.
- **Better metadata hygiene**: inbox DHT nodes see only opaque feed_pubkeys
  in announce records and encrypted blobs in feed records. They cannot
  determine the sender, the channel, or the invite contents. (Note: if
  Bob's `id_pubkey` is publicly known, his inbox topics are computable —
  an observer can infer that *someone* is inviting Bob, but not who or to what.)
- **Rich invites**: initial message, welcome text, channel name/salt all
  fit inside the encrypted feed record.
- **Group administration**: moderators can invite people to private channels
  without prior DM contact. (Note: v1 CLI only exposes DM invites as a
  sender-side command. Group/private channel invite sending is deferred to v2.
  The protocol and inbox receiver support group invites already.)
- **Forward compatible**: future extensions (read-only invites, multi-person
  invites, expiration times) fit naturally in the feed record.

### Abuse Resistance

- **Epoch rotation** spreads inbox announces across different DHT nodes
  (same benefit as channel announce rotation)
- **Client-side sender cap**: ignore inbox invites from more than N unknown
  identities per polling cycle (configurable, e.g., 10)
- **Client-side blocklist**: permanently ignore specific `id_pubkeys`
- **Decryption as filter**: invites that fail decryption are immediately
  discarded — spam that doesn't know Bob's pubkey can't even produce a
  valid encrypted payload
- **Invite feeds are cheap**: temporary, short-lived, expire via normal TTL

### DM Properties

- Messages are end-to-end encrypted (X25519 ECDH, static keys)
- No forward secrecy in v1 (can add ephemeral key ratcheting later)
- DM topic is derivable by anyone who knows both pubkeys — an observer can
  detect that Alice and Bob have a DM channel but cannot read the content
- Inbox invite reveals nothing to DHT nodes beyond "someone announced an
  opaque feed on Bob's inbox topic" — the invite payload is encrypted

---

## Track 5: Discovery & Announce Semantics

### Announce = "I Have New Content"

Announce is used strictly as a **new content signal**, not as idle presence.
A participant announces on the channel's epoch+bucket topic only when they
post a new message. Idle readers do not announce.

**Exception**: Invite inbox re-announces (§8.5 per-message nudges) are exempt
from the new-content-only rule — they signal availability to the recipient,
not new channel content. These use the inbox topic, not the channel topic.

This means:
- Announce slots are consumed only by active posters, not lurkers
- A participant who stops posting naturally disappears from announce results
  after their record expires (~20 min TTL)
- "Who's in this channel" is not directly answerable — only "who has posted
  recently" is visible through announce. Longer-term participant knowledge
  is accumulated locally as readers discover feeds over time.

### Epoch+Bucket Topic Rotation

Announce topics rotate every epoch (1 minute) with 4 buckets per epoch:

```rust
announce_topic = keyed_blake2b(key = channel_key,
    msg = b"peeroxide-chat:announce:v1:" || epoch_u64_le || bucket_u8)

// epoch = unix_time_secs / 60
// bucket = 0..3
```

**Why rotate**: A static topic means the same K-closest DHT nodes handle
all operations for a channel indefinitely. Those nodes accumulate a
persistent traffic analysis profile. With epoch rotation, different DHT
nodes handle discovery in different time windows — no single set of nodes
builds the complete picture.

**Why 4 buckets**: Each bucket supports 20 announce records per node.
4 buckets = 80 concurrent announces per epoch per node, handling burst
scenarios where many participants post within the same minute.

**Bucket selection (reference client)**: Each client generates a random
permutation of [0, 1, 2, 3] once per feed keypair (on channel join or feed
rotation) and cycles through it sequentially for successive announces. This
spreads a single client's traffic evenly across buckets without protocol-level
coordination. Randomly selecting a bucket for every announce is equally valid —
the permutation approach is a minor optimization, not a requirement. Malicious
clients may ignore this; the design remains robust (oldest-first eviction
handles hotspots naturally).

**Announce is a hint, not delivery.** Even if all buckets within an epoch
become full (due to abuse or a naturally busy channel), a reader who discovers
a feed_pubkey even once — from any epoch, any bucket — gains access to the
independent feed record and thus all messages in that feed. Previous or future
announces that get evicted before detection do not cause message loss; they
only delay discovery of new feeds.

### Capacity

- 4 buckets × 20 records per node per bucket = 80 announce slots per epoch
- Announce records expire after ~20 minutes (DHT TTL)
- Since announce is per-post (not idle presence), slots are consumed only
  by active posters — a channel with 100 readers but 5 active posters
  uses only 5 slots
- Eviction is oldest-first by `inserted_at`

### Discovery Flow

Readers scan epoch+bucket topics to discover new feed_pubkeys:

```
ON JOIN/RESUME (one-time catch-up):
  For epoch in [current, current-1, ..., current-19]:  // last 20 minutes
      For bucket in 0..4:
          lookup(announce_topic(channel_key, epoch, bucket))
          -> collect any new feed_pubkeys not in local known set

STEADY-STATE (periodic scan):
  For each of [current_epoch, previous_epoch]:
      For bucket in 0..4:
          lookup(announce_topic(channel_key, epoch, bucket))
          -> collect any new feed_pubkeys not in local known set
```

8 lookups per steady-state scan cycle (80 on initial join). Once a feed_pubkey
is discovered, it's added to the reader's local known set and polled directly
via `mutable_get` until it goes stale (feed record stops being refreshed).
After the feed expires or rotates, the user is re-discovered through announce
the next time they post, or via the `next_feed_pubkey` handoff link in the
old feed record.

### No IP Exposure

- `relay_addresses = []` always — no addresses in stored records
- DHT nodes handling announces see the source IP of the request, but:
  - Cannot link it to an identity (feed_pubkey is random, unlinked to id)
  - Cannot link it to a channel name (announce topic is an opaque hash)
  - The stored record contains only `{feed_pubkey, relay_addresses: []}`
- Epoch rotation means different DHT nodes handle the channel over time

---

## Track 6: Polling Strategy

### Intervals

| Context | Interval | Operations |
|---------|----------|------------|
| Focused channel (discovery) | 5-8s | Scan current + previous epoch (8 lookups) |
| Focused channel (feeds) | 5-8s | mutable_get per known feed + immutable_get for new msgs |
| Background channel (discovery) | 30-60s | Same 8 lookups, lower frequency |
| Background channel (feeds) | 30-60s | Same feed polling, lower frequency |
| Invite inbox | 15-30s | lookup(inbox_topic) for new invite feed_pubkeys |
| Re-mutable_put (feed refresh) | ~8 min | Refresh feed record TTL (even if unchanged) |

### Polling Flow (per channel)

```
DISCOVERY (scan for new posters):
  On join/resume:
      Scan last 20 epochs × 4 buckets (80 lookups, one-time)
  Steady-state:
      For epoch in [current, previous]:
          For bucket in 0..4:
              lookup(announce_topic(channel_key, epoch, bucket))
              -> add new feed_pubkeys to known set

CONTENT (poll known feeds):
  For each known feed_pubkey:
      mutable_get(feed_pubkey)    -> check seq for changes
      If changed:
          extract new msg_hashes
          For each new hash:
              immutable_get(hash) -> decrypt, verify, display
          If next_feed_pubkey is set:
              add new feed to known set, schedule old feed for expiry

ADAPTIVE BEHAVIOR:
  - Back off quiet feeds: if unchanged for 3+ cycles, reduce poll rate
  - Cap known-feed set: max ~100 active feeds per channel
  - Expire stale feeds: remove from active polling after 3 consecutive missed refreshes (seq unchanged + TTL likely expired); re-activate on re-discovery via announce
  - Never mark a msg_hash "seen" until immutable_get + decrypt + verify succeeds
  - Retry failed fetches on next poll cycle
  - Malformed records: silently discard (truncated, invalid lengths, bad UTF-8, failed signature). In verbose mode, log a warning with feed_pubkey and failure reason.
  - Cyclic summary chains: cap traversal depth (e.g., 100 blocks) to prevent infinite loops from malicious data
```

### Cost Estimates

Focused channel (10 known participants, 2 new messages per cycle):
- Discovery: 8 lookups (current + previous epoch × 4 buckets)
- Feeds: 10 mutable_gets
- Messages: 2 immutable_gets
- Total: ~20 DHT operations per 5-8 seconds
- Bandwidth: roughly 30-50 KB/min

Background channel (same scenario):
- Same operations, 30-60s interval
- Bandwidth: roughly 3-8 KB/min

---

## Track 7: Wire Formats

All multi-byte integers are **little-endian**. Hash references are
always 32 bytes (BLAKE2b-256). Public keys are always 32
bytes (Ed25519). Signatures are always 64 bytes (Ed25519 detached).

No version byte is needed in record payloads — the `:v1:` namespace
in all key derivation paths (announce topics, message keys, ownership
proofs, invite keys) means a v2 protocol would produce entirely
different DHT addresses. A v1 client will never encounter a v2 record.

### Size Budgets

| Storage method | Practical max value | Source |
|---|---|---|
| `mutable_put` value | 1000 bytes | UDP packet budget (established by deaddrop) |
| `immutable_put` value | 1000 bytes | Same UDP constraint |
| Encryption overhead | 40 bytes | 24 (nonce) + 16 (Poly1305 tag) |

### 7.1 Message Envelope (immutable_put, encrypted)

Stored via `immutable_put`. The value is the encrypted ciphertext.
`target = hash(ciphertext)` (content-addressed).

**On-wire (what DHT nodes store):**
```
nonce(24) || tag(16) || ciphertext(N)
```

**Plaintext (inside encryption):**
```
Offset  Size  Field
─────────────────────────────────────────────────
0       32    id_pubkey (author's identity public key)
32      32    prev_msg_hash (previous msg from this feed, or 32 zeros)
64      8     timestamp (unix_time_secs, u64 LE)
72      1     content_type (enum, see below)
73      1     screen_name_len (0-255)
74      N     screen_name (UTF-8, N = screen_name_len)
74+N    2     content_len (u16 LE)
76+N    M     content (UTF-8 text for type 0x01)
76+N+M  64    signature (Ed25519 detached)
```

**Signature covers** (sign-then-encrypt):
```
b"peeroxide-chat:msg:v1:" || prev_msg_hash(32) || timestamp(8) || content_type(1) || screen_name_len(1) || screen_name(N) || content(M)
```

**Content types:**
| Value | Meaning |
|-------|---------|
| 0x01 | UTF-8 text message |
| 0x02–0xFF | Reserved for future use |

**Size budget:**
- Fixed overhead (plaintext): 32 + 32 + 8 + 1 + 1 + 2 + 64 = 140 bytes
- Encryption overhead: 40 bytes
- Total overhead: 180 bytes
- **Max screen_name + content: 820 bytes** (1000 - 180)
- With a 32-byte screen name: max content = 788 bytes (~197 words)

### 7.2 Feed Record (mutable_put, plaintext)

Stored via `mutable_put(feed_keypair, value, seq)`. The DHT handles
signing and seq enforcement — no application-level signature needed in
the value. Addressed by `hash(feed_pubkey)`.

**On-wire (the mutable_put value):**
```
Offset  Size  Field
─────────────────────────────────────────────────
0       32    id_pubkey (author's identity public key)
32      64    ownership_proof (see below)
96      32    next_feed_pubkey (32 zeros if no rotation pending)
128     32    summary_hash (32 zeros if no summary blocks yet)
160     1     msg_count (number of message hashes, 0-26)
161     N×32  msg_hashes (newest first, N = msg_count)
```

**Ownership proof:**
```
sign(id_secret_key, b"peeroxide-chat:ownership:v1:" || feed_pubkey(32) || channel_key(32))
```

This binds the feed to both the identity AND the specific channel.
Readers verify by reconstructing the signable from the feed_pubkey
(known from the mutable_get address) and their own channel_key.

**Size budget:**
- Fixed overhead: 161 bytes
- Remaining: 839 bytes / 32 = **26 message hashes max**
- With 26 hashes: total = 161 + 832 = 993 bytes ✓

### 7.3 Summary Block (immutable_put, plaintext)

Batches older message hashes that no longer fit in the feed record.
Chained via `prev_summary_hash`. Signed by identity key for integrity.

**On-wire (the immutable_put value):**
```
Offset  Size  Field
─────────────────────────────────────────────────
0       32    id_pubkey (author's identity public key)
32      32    prev_summary_hash (32 zeros if this is the first summary)
64      1     msg_count (number of message hashes in this block)
65      N×32  msg_hashes (oldest first — chronological within block)
65+N×32 64    signature (Ed25519 detached)
```

**Signature covers:**
```
b"peeroxide-chat:summary:v1:" || prev_summary_hash(32) || msg_hashes(N×32)
```

**Size budget:**
- Fixed overhead: 129 bytes
- Remaining: 871 bytes / 32 = **27 message hashes per block**
- With 27 hashes: total = 129 + 864 = 993 bytes ✓

### 7.4 Personal Nexus (mutable_put, plaintext)

Stored via `mutable_put(id_keypair, value, seq)`. Addressed by
`hash(id_pubkey)`. The DHT's built-in signature verification ensures
only the identity holder can update it. Seq uses `unix_time_secs`.

**On-wire (the mutable_put value):**
```
Offset  Size  Field
─────────────────────────────────────────────────
0       1     name_len (0-255)
1       N     name (UTF-8 screen name, N = name_len)
1+N     2     bio_len (u16 LE, 0-65535)
3+N     M     bio (UTF-8 bio text, M = bio_len)
```

**Size budget:**
- Fixed overhead: 3 bytes
- **Max name + bio: 997 bytes**
- Practical: 32-byte name + 960-byte bio, or any split

No application-level signature needed — `mutable_put` is already
authenticated by the DHT layer (Ed25519 signature over the value
verified by storing nodes).

**Multi-device note**: If two devices update the nexus in the same second,
they produce the same seq. The DHT accepts whichever arrives first at each
node; the other is silently dropped (SEQ_REUSED). Clock skew between devices
may cause a lower-seq update to be rejected (SEQ_TOO_LOW). This is acceptable
for v1 — nexus is best-effort profile data, not critical state.

### 7.5 Invite Feed Record (mutable_put, encrypted)

Stored via `mutable_put(invite_feed_keypair, encrypted_value, seq=0)`.
The entire value is encrypted under the recipient's X25519 public key
(derived from their Ed25519 id_pubkey via birational map).

**On-wire (what DHT nodes store):**
```
nonce(24) || tag(16) || ciphertext(N)
```

**Encryption key derivation:**
```
invite_feed_x25519_pub = ed25519_to_x25519(invite_feed_pubkey)
invite_feed_x25519_priv = ed25519_to_x25519(invite_feed_secret_key)
recipient_x25519_pub = ed25519_to_x25519(recipient_id_pubkey)

// Alice (sender):
ecdh_secret = X25519(invite_feed_x25519_priv, recipient_x25519_pub)

// Bob (recipient) — knows invite_feed_pubkey from mutable_get address:
ecdh_secret = X25519(bob_x25519_priv, invite_feed_x25519_pub)

invite_key = keyed_blake2b(key = ecdh_secret,
    msg = b"peeroxide-chat:invite-key:v1:" || invite_feed_pubkey(32))
```

Using the invite_feed_keypair (not Alice's identity keypair) for ECDH
means Bob can decrypt without knowing who sent the invite. Alice's
identity is revealed only after successful decryption (inside the
plaintext). This also means each invite feed has a unique ECDH secret
even between the same sender/recipient pair.

**Plaintext (inside encryption):**
```
Offset  Size  Field
─────────────────────────────────────────────────
0       32    id_pubkey (sender's identity public key)
32      64    ownership_proof (same format as feed record)
96      32    next_feed_pubkey (sender's real feed for this channel)
128     1     invite_type (enum, see below)
129     2     payload_len (u16 LE)
131     N     payload (invite-type-specific content)
```

**Invite types:**
| Value | Meaning | Payload contents |
|-------|---------|-----------------|
| 0x01 | DM invite | optional message (UTF-8) |
| 0x02 | Private channel invite | name_len(1) + name + salt_len(2) + salt + optional message |

**Ownership proof for invites:**
```
sign(id_secret_key, b"peeroxide-chat:ownership:v1:" || invite_feed_pubkey(32) || channel_key(32))
```

Same formula as regular feed records. Bob verifies by computing the
candidate channel_key (DM: from sorted pubkeys; group: from name+salt
in the decrypted payload) and checking the proof.

**Size budget:**
- Fixed plaintext overhead: 131 bytes
- Encryption overhead: 40 bytes
- Total overhead: 171 bytes
- **Max invite payload: 829 bytes**
- For a DM invite with message: 829 bytes of lure text
- For a group invite: ~800 bytes after name+salt headers

### 7.6 Inbox Nudge (mutable_put, encrypted)

The per-message DM nudge (max once per epoch) uses the same invite feed
record format (§7.5). The sender maintains **one invite_feed_keypair per
DM session** for nudging — incrementing seq on each nudge rather than
generating a new keypair each time. The `next_feed_pubkey` points to the
sender's current DM feed, giving the recipient a direct path to the
conversation.

Bob's inbox client tracks seen invite_feed_pubkeys. A new feed_pubkey =
new notification to display. Same feed_pubkey with higher seq = refresh
(don't re-display, but update `next_feed_pubkey` if it changed due to
feed rotation).

No separate wire format needed — reuses §7.5 exactly.

### 7.7 Encryption Details

All encryption uses **XSalsa20Poly1305** with:
- 24-byte random nonce (birthday-safe at 2^96)
- 16-byte Poly1305 authentication tag
- Empty associated data (b"")
- Wire format: `nonce(24) || tag(16) || ciphertext`

**Key derivation per context:**

| Context | Encryption key |
|---------|---------------|
| Channel messages | `keyed_blake2b(key=channel_key, msg=b"peeroxide-chat:msgkey:v1")` |
| DM messages | `keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:dm-msgkey:v1:" \|\| channel_key)` |
| Invite records | `keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:invite-key:v1:" \|\| invite_feed_pubkey)` |

---

## Track 8: Operation Sequences

Step-by-step choreography for key operations. All DHT operations within
a sequence that have no data dependency on each other should be executed
concurrently (tokio tasks). Operations with ordering dependencies are
marked with **"← MUST complete before next step"**.

### 8.1 Joining a Channel (Cold Start)

What happens when a user runs `peeroxide chat join <channel>`.

```
SETUP:
  1. Load identity seed from profile → derive id_keypair
  2. channel_key = hash_batch([b"peeroxide-chat:channel:v1:", len4(name), name])
     (add salt for private channels)
  3. msg_key = keyed_blake2b(key=channel_key, msg=b"peeroxide-chat:msgkey:v1")
  4. Bootstrap DHT node (bind UDP, connect to bootstraps, warm routing table)
  5. feed_keypair = KeyPair::generate()
  6. ownership_proof = sign(id_sk, b"peeroxide-chat:ownership:v1:" || feed_pubkey || channel_key)
  7. Pick random bucket permutation [0,1,2,3] for this feed_keypair
  8. Initialize empty feed record (msg_count=0)

COLD-START SCAN (all parallel):
  9. current_epoch = unix_time_secs / 60
  10. Spawn 80 lookup tasks (20 epochs × 4 buckets) concurrently
      As each returns → collect new feed_pubkeys into known_feeds set
      As each new feed_pubkey discovered → immediately spawn mutable_get
      As each feed record returns:
        Verify ownership_proof against id_pubkey and channel_key
        If invalid → discard
        Extract msg_hashes → spawn immutable_get for each unknown hash
      As each message returns:
        Decrypt with msg_key, verify signature
        If valid → cache message, update known_users file
        If invalid → skip, do NOT mark hash as "seen"

DISPLAY:
  11. Sort all cached messages by timestamp
  12. Display chronologically
  13. Print "*** — live —" separator

MAIN LOOP (concurrent tasks):
  14a. Discovery: scan current + previous epoch (8 lookups, every 5-8s)
  14b. Feed polling: mutable_get per known feed (every 5-8s)
       → fetch new msg_hashes via immutable_get, decrypt, display
  14c. Stdin reader: read lines, post as messages (see §8.2)
  14d. Feed refresh: re-mutable_put own feed record (every ~8 min)
  14e. Nexus refresh: re-mutable_put own nexus (every ~8 min, unless --no-nexus)
  14f. Friend refresh: mutable_get one friend's nexus per poll cycle (unless --no-friends)
  14g. Feed rotation check: if feed_keypair age > lifetime → rotate (see §8.3)
```

### 8.2 Posting a Message

What happens when the user types a line and hits enter.

```
BUILD:
  1. prev_msg_hash = last msg_hash posted from THIS feed (or 32 zeros if first)
  2. timestamp = unix_time_secs as u64
  3. content_type = 0x01 (text)
  4. content = UTF-8 input bytes
  5. screen_name = user's configured display name (from profile)
  6. signable = b"peeroxide-chat:msg:v1:" || prev_msg_hash || timestamp || content_type || screen_name_len || screen_name || content
  7. signature = sign(id_sk, signable)
  8. Assemble plaintext per §7.1 layout (fields + signature appended)

ENCRYPT:
  9. nonce = random 24 bytes
  10. encrypted = nonce || tag || XSalsa20Poly1305::encrypt(msg_key, nonce, plaintext)

PUBLISH (ordered):
  11. immutable_put(encrypted) → msg_hash          ← MUST complete before step 12
  12. Prepend msg_hash to feed record's msg_hashes
      If msg_count reaches 20 → summary block first (see §8.4)
      Increment seq
  13. mutable_put(feed_keypair, updated_record, seq)  ← can parallel with step 14
  14. announce(current_epoch_topic, feed_keypair, [])

LOCAL:
  15. Display message immediately (no round-trip wait)
  16. Update prev_msg_hash = msg_hash
```

### 8.3 Feed Rotation

Triggered when feed_keypair age exceeds configured lifetime (default
60 min ± 50% random wobble).

```
PREPARE:
  1. new_feed_keypair = KeyPair::generate()
  2. new_ownership_proof = sign(id_sk, b"peeroxide-chat:ownership:v1:" || new_feed_pubkey || channel_key)

HANDOFF (ordered — publish target before pointer):
  3. Initialize new feed record (empty, msg_count=0, ownership_proof=new_ownership_proof)
  4. mutable_put(new_feed_keypair, new_feed_record, seq=0)  ← MUST complete before step 5
  5. Set next_feed_pubkey = new_feed_keypair.public_key in current feed record
  6. mutable_put(old_feed_keypair, updated_record, seq+1)
     Readers now see the handoff link.

SWITCH:
  7. Active feed = new_feed_keypair
  8. Reset: msg_hashes=[], msg_count=0, prev_msg_hash=zeros, seq=1 (already used 0)
  9. New random bucket permutation
  10. Record rotation timestamp (for next rotation check)

OVERLAP:
  11. Continue refreshing old feed record for ONE more cycle (~8 min)
      so readers have time to discover and follow next_feed_pubkey
  12. After that refresh, stop. Old feed expires via DHT TTL (~20 min).
```

### 8.4 Summary Block Publish

Triggered when msg_count reaches 20 (before prepending the new hash). Happens
inline during §8.2 step 12, before the feed record update. Eviction operates
on the existing 20 hashes; the new message hash is prepended afterward.

```
EVICT:
  1. Take the oldest 15 hashes from msg_hashes, leave newest 5
     Trigger threshold: 20/26. Headroom: 21 posts before next eviction.

BUILD:
  2. prev_summary_hash = current feed record's summary_hash (or 32 zeros)
  3. Assemble summary block per §7.3:
     - id_pubkey, prev_summary_hash, msg_count, msg_hashes (evicted, oldest first)
  4. signable = b"peeroxide-chat:summary:v1:" || prev_summary_hash || msg_hashes
  5. signature = sign(id_sk, signable)
  6. Append signature to summary block

PUBLISH (ordered):
  7. immutable_put(summary_block) → summary_hash   ← MUST complete before step 8
  8. Update feed record:
     - summary_hash = new summary_hash
     - msg_hashes = kept hashes only
     - msg_count = updated count
  9. Return to §8.2 step 12 (prepend new msg_hash, mutable_put)
```

### 8.5 Starting a DM

What happens when Alice runs `peeroxide chat dm <bob_pubkey>`.

```
SETUP:
  1. Load identity → derive id_keypair
  2. channel_key = hash_batch([b"peeroxide-chat:dm:v1:", lex_min(alice_id, bob_id), lex_max(alice_id, bob_id)])
  3. Derive dm_msg_key:
     - bob_x25519_pub = ed25519_to_x25519(bob_id_pubkey)
     - ecdh_secret = X25519(alice_x25519_priv, bob_x25519_pub)
     - dm_msg_key = keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:dm-msgkey:v1:" || channel_key)
  4. Bootstrap DHT, generate feed_keypair, compute ownership_proof
  5. Cold-start scan on DM topic (20 epochs × 4 buckets, same as §8.1)

STARTUP NUDGE (only if --message provided):
  6. invite_feed_keypair = KeyPair::generate()
  7. Build invite plaintext per §7.5:
     - id_pubkey = alice's
     - ownership_proof over invite_feed_pubkey + channel_key
     - next_feed_pubkey = alice's real feed_pubkey for this DM
     - invite_type = 0x01 (DM)
     - payload = --message text
  8. Encrypt invite:
     - ecdh_secret = X25519(invite_feed_x25519_priv, bob_x25519_pub)
     - invite_key = keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:invite-key:v1:" || invite_feed_pubkey)
     - encrypted = XSalsa20Poly1305::encrypt(invite_key, random_nonce, plaintext)
  9. Publish Alice's real DM feed record first:
     mutable_put(feed_keypair, initial_feed_record, seq=0)  ← MUST complete before step 10
  10. mutable_put(invite_feed_keypair, encrypted, seq=0)
  11. inbox_topic = keyed_blake2b(key=hash(bob_id_pubkey), msg=b"peeroxide-chat:inbox:v1:" || epoch || bucket)
  12. announce(inbox_topic, invite_feed_keypair, [])

  If --message is NOT provided:
  6. invite_feed_keypair = KeyPair::generate()
     (Created but not published yet — held for per-message nudges later)
  7. Publish Alice's real DM feed record:
     mutable_put(feed_keypair, initial_feed_record, seq=0)

MAIN LOOP:
  13. Same as §8.1 step 14, but using dm_msg_key for encryption
  14. Per-message inbox nudge: on each post, if current_epoch != last_nudge_epoch:
      - Build invite plaintext (same as startup nudge but payload = triggering message text, truncated to fit)
      - Update invite record's next_feed_pubkey if feed rotated
      - mutable_put(invite_feed_keypair, re-encrypted, seq+1)
      - announce(bob_inbox_topic_current_epoch, invite_feed_keypair, [])
      - last_nudge_epoch = current_epoch
```

### 8.6 Receiving an Invite

What happens in Bob's inbox client when a new invite_feed_pubkey is
discovered on his inbox topic.

```
FETCH:
  1. mutable_get(invite_feed_pubkey) → encrypted record

DECRYPT:
  2. invite_feed_x25519_pub = ed25519_to_x25519(invite_feed_pubkey)
  3. ecdh_secret = X25519(bob_x25519_priv, invite_feed_x25519_pub)
  4. invite_key = keyed_blake2b(key=ecdh_secret, msg=b"peeroxide-chat:invite-key:v1:" || invite_feed_pubkey)
  5. Decrypt. If fails → not for Bob (or spam), discard silently.

PARSE:
  6. Extract: id_pubkey, ownership_proof, next_feed_pubkey, invite_type, payload

VERIFY (determine channel type):
  7. Try DM:
     candidate_key = hash_batch([b"peeroxide-chat:dm:v1:", lex_min(sender, bob), lex_max(sender, bob)])
     Verify: verify(sender_id_pubkey, b"peeroxide-chat:ownership:v1:" || invite_feed_pubkey || candidate_key)
     If valid → DM invite confirmed.

  8. If DM failed, try group (invite_type must be 0x02):
     Extract name_len, name, salt_len, salt from payload
     candidate_key = hash_batch([b"peeroxide-chat:channel:v1:", len4(name), name, b":salt:", len4(salt), salt])
     Verify ownership_proof against candidate_key
     If valid → group invite confirmed.

  9. If neither verifies → discard.

DISPLAY:
  10. DM invite:
      [INVITE] DM from <sender_name@shortkey>
        → peeroxide chat dm <sender_full_pubkey> --profile <current>

  11. Group invite:
      [INVITE] Channel "name" from <sender_name@shortkey>
        → peeroxide chat join "name" --group "salt" --profile <current>

  12. Update known_users cache with sender's id_pubkey

DEDUP:
  13. Track seen invite_feed_pubkeys. Same pubkey with higher seq =
      refresh (update next_feed_pubkey, don't re-display).
```

---

## CLI Interface

See [`CHAT_CLI.md`](./CHAT_CLI.md) for the command-line interface design.
The protocol spec (this document) is implementation-agnostic.

---

## Key Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Transport | Pure DHT (no direct connections) | Only way to achieve source IP anonymity without custom relay |
| Architecture | Per-participant feed model | Each participant owns their feed; messages in immutable_put |
| Announce semantics | New content signal only (not idle presence) | Saves announce slots for active posters; lurkers don't consume capacity |
| Announce topics | Epoch-rotating with 4 buckets (1-min epochs) | Rotates DHT node exposure; 80 slots/epoch handles bursts |
| Messages | immutable_put (content-addressed) | Immutable, refreshable by anyone, no write conflicts |
| Feed records | mutable_put per feed keypair | One record per participant per channel; stable address across epochs; includes `next_feed_pubkey` for rotation handoff |
| Encryption | XSalsa20Poly1305, random 24-byte nonce | Already in codebase; birthday-safe nonce eliminates reuse risk |
| All messages encrypted | Yes, including public channels | Author identity and screen_name hidden inside ciphertext; not in feed metadata |
| KDF | Keyed BLAKE2b-256 (no HKDF) | Already used (`discovery_key` pattern); no new dependencies |
| DM encryption | X25519 ECDH (Ed25519 -> Curve25519) | `curve25519-dalek` already a dependency; static keys for v1 |
| Feed keypair | Random per session, client-rotated with ±50% wobble | No multi-device conflicts; rotation is client privacy decision, not protocol |
| Ownership proof | `sign(id_secret, "ownership" \|\| feed_pubkey \|\| channel_key)` | Binds feed to identity; prevents feed spoofing |
| Announce usage | `relay_addresses = []` always | No IP exposure; protocol-legal (confirmed Rust + JS implementations) |
| Key storage | Plaintext Ed25519 seed on disk (v1) | Simple; assumes full disk encryption |
| Personal nexus | mutable_put under id_keypair | Cross-channel profile; only use of identity keypair's mutable slot |
| DM discovery | Generalized Invite Inbox (feed-based, encrypted) | Uniform machinery for DMs + group invites; id_keypair never used for transport; encrypted payload hides invite metadata from DHT nodes |
| Polling | Focused 5-8s / Background 30-60s / Adaptive backoff | Balances latency vs DHT load; stale feeds expire from known set |
| Cold-start discovery | Scan 20 epochs on join (one-time) | Catches up on 20 min of history; prevents ghost-channel problem |
| Feed rotation handoff | `next_feed_pubkey` in old feed + brief overlap | Readers follow the link; prevents losing track of rotated feeds |
| Message chaining | `prev_msg_hash` scoped per-feed (not per-identity) | Avoids forks from multi-device concurrent posting |
| Screen name location | Inside encrypted message payloads (as a field) and personal nexus | Prevents DHT nodes from building identity profiles from feed metadata; recipients always have a display name per message |

---

## References (Code Locations)

| Component | File |
|-----------|------|
| BLAKE2b hash, keyed hash | `peeroxide-dht/src/crypto.rs` |
| XSalsa20Poly1305 encrypt/decrypt | `peeroxide-dht/src/secure_payload.rs` |
| Ed25519 sign/verify | `peeroxide-dht/src/crypto.rs` |
| KeyPair, from_seed | `peeroxide-dht/src/hyperdht.rs` |
| mutable_put/get API | `peeroxide-dht/src/hyperdht.rs` |
| immutable_put/get API | `peeroxide-dht/src/hyperdht.rs` |
| announce/lookup API | `peeroxide-dht/src/hyperdht.rs` |
| Record storage + TTL | `peeroxide-dht/src/persistent.rs` |
| Announce record fields (HyperPeer) | `peeroxide-dht/src/hyperdht_messages.rs` |
| Chunking pattern (dd) | `peeroxide-cli/src/cmd/deaddrop.rs` |
| X25519 (curve25519-dalek) | dependency of `peeroxide-dht` |

---

## Appendix A: DHT Operation Reference

Confirmed behaviour from code inspection of `peeroxide-dht`.

### A.1 -- immutable_put / immutable_get

Content-addressed storage. `target = hash(value)`. Anyone can re-put to
refresh TTL.

| Property | Detail |
|----------|--------|
| Max payload | 1000 bytes |
| Addressing | `hash(value)` -- immutable |
| Authentication | None (content-addressed) |
| Multi-writer | N/A (content-addressed, anyone can re-put) |

### A.2 -- mutable_put / mutable_get

Signed, updateable storage. `target = hash(public_key)`. Only key holder
can update.

| Property | Detail |
|----------|--------|
| Max payload (value) | 1000 bytes |
| Addressing | `hash(public_key)` -- one slot per keypair |
| Seq semantics | Strictly monotonic; higher wins |
| Authentication | Ed25519 signature verified by DHT nodes |

### A.3 -- announce / lookup

Multi-writer peer discovery. Multiple peers announce under one topic.

| Property | Detail |
|----------|--------|
| Data stored | `HyperPeer { public_key, relay_addresses }` |
| Multi-writer | Up to 20 per topic per node |
| IP in record | No -- source IP NOT stored |
| Empty relay_addresses | Valid (confirmed Rust + JS) |
| Eviction | Oldest `inserted_at` dropped first |

### A.4 -- TTL

All record types: 20-minute default TTL. Clients must re-announce /
re-put every ~8 minutes to keep data alive.
