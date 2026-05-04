# peeroxide-chat: Design Notes

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
   is acceptable. Local clients cache received messages for UX continuity.
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
| 1. Unmask identity | **Strong vs participants; Medium vs DHT nodes** | Same as casual — IP never exposed to other participants. DHT nodes serving feeds still see source IP + `id_pubkey` in plaintext feed record. Mitigated by: id_pubkey has no public footprint, feed rotation limits observation window. |
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

- **DHT nodes see your IP** when you make requests (inherent to UDP). They
  can correlate "IP X operated on topic Y at time T" within a single epoch.
  Epoch rotation limits this to 1-minute windows for discovery, but feed
  polling persists for the feed's lifetime.
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
- **Local client caches** persist beyond DHT TTL. Physical access to a
  device = access to chat history.
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
- Feed record contains ~26 recent message hashes — readers can fetch all
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
Never used for DHT transport operations (announce, mutable_put, etc.).

**Per-channel feed keypair** (`feed_keypair`): Random, generated per session
(or rotated by the client on a configurable schedule). Used for `announce`
and `mutable_put` on a specific channel. Not derived from the identity key —
feed rotation is a client-side privacy decision.

```
Identity keypair   -> signs message content (inside encryption)
                   -> signs personal nexus record
                   -> signs ownership proofs (including invite feed proofs)
                   -> NEVER used for announce or mutable_put

Feed keypair       -> used for announce + mutable_put (per channel)
                   -> random per session, rotated by client
                   -> bound to identity via ownership proof in feed record
                   -> also used for temporary invite feeds (same machinery)
```

### Profiles (Multiple Identities)

Users can maintain multiple named profiles, each with its own identity keypair:

```
~/.peeroxide/profiles/
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
// Ed25519 -> X25519 conversion (curve25519-dalek already a dep)
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
7. Never mark a hash "seen" until decrypt+verify succeeds; retry on next cycle

Once a feed_pubkey is discovered, it stays in the known set permanently
(for this channel session). The reader polls it directly via mutable_get
without needing to re-discover it through announce. This means the announce
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
- `msg_hashes` — up to ~26 recent message hashes (newest first)
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

When a participant's message count exceeds the ~26-hash capacity of the
feed record, older hashes are batched into **summary blocks** stored
via `immutable_put`. Each summary block contains ~30 message hashes and a
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
| Message (immutable_put) | ~1100 bytes | ~210 bytes (envelope + encryption + signature) | ~890 bytes |
| Feed record (mutable_put) | ~1002 bytes | ~100-132 bytes (fixed fields, no screen_name) | 27-28 msg hashes |
| Invite feed record (mutable_put) | ~1002 bytes | ~140 bytes (ECDH encryption overhead + fixed fields) | ~860 bytes for invite payload |
| Summary block (immutable_put) | ~1100 bytes | ~130 bytes (header + signature) | ~30 msg hashes |
| Personal nexus (mutable_put) | ~1002 bytes | ~132 bytes (fixed fields) | ~870 bytes for bio |

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
- Both derive the same `channel_key` from their sorted pubkeys
- Both derive their own `feed_keypair` for that channel
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

3. **Alice builds an invite feed record** (same structure as normal feed
   records, with optional extensions):
   - `id_pubkey` — Alice's real identity public key
   - `ownership_proof` — `sign(alice_id_sk, b"peeroxide-chat:ownership:v1:" || invite_feed_pubkey || channel_key)`
   - `msg_hashes` — optional initial encrypted message(s)
   - `next_feed_pubkey` — Alice's real ongoing feed_pubkey for this channel
     (so Bob can immediately start polling the conversation)
   - Optional: `channel_name`, `salt`/keyfile hint, `invite_type` ("dm" | "private"),
     `invite_message` (welcome text)

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
   duration) until she observes Bob's activity on the channel.

### Bob's Side (Receiving Invites)

Bob polls his inbox topic periodically (same cadence as background channels):

1. `lookup(inbox_topic)` → discovers temporary invite feed_pubkeys
2. For each new feed_pubkey: `mutable_get(invite_feed_pubkey)`
3. **Decrypt** the feed record using his X25519 private key + the invite
   feed's X25519 public key (ECDH). If decryption fails → not for Bob
   (or spam); discard.
4. **Verify ownership proof** against the `id_pubkey` in the decrypted record.
5. **Discern channel type automatically**:
   - Compute the DM `channel_key` between Alice's `id_pubkey` and Bob's own.
   - If it matches the `channel_key` from the ownership proof → **DM invite**.
   - Otherwise → **group/private channel invite**. Use the provided
     `channel_name` + `salt` (from inside the encrypted payload) to derive
     the channel key and join.
6. **Begin normal operation**: add Alice's real feed (via `next_feed_pubkey`)
   to the channel's known feeds and start polling.
7. Ignore invites for channels Bob is already participating in.

### Why This Design

- **Total uniformity**: every form of discovery (channels, DMs, group invites)
  uses identical feed/announce/mutable_put machinery. No special cases.
- **The long-term `id_keypair` is NEVER used for announce or any DHT transport
  operation.** The last exception (old inbox announce) is eliminated.
- **Better metadata hygiene**: inbox DHT nodes see only opaque feed_pubkeys
  in announce records and encrypted blobs in feed records. They cannot
  determine who is inviting whom or to what channel.
- **Rich invites**: initial message, welcome text, channel name/salt all
  fit inside the encrypted feed record.
- **Group administration**: moderators can invite people to private channels
  without prior DM contact.
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
  - Expire stale feeds: remove after 3 consecutive missed refreshes (seq unchanged + TTL likely expired)
  - Never mark a msg_hash "seen" until immutable_get + decrypt + verify succeeds
  - Retry failed fetches on next poll cycle
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

## Track 7: CLI Interface

### Command Shape (Sketch)

```bash
# Join a public channel
peeroxide chat join "general"

# Join a private channel (group name salt)
peeroxide chat join "general" --group "My Buddies"

# Join a private channel (keyfile salt)
peeroxide chat join "general" --keyfile ~/.config/peeroxide/mykey.bin

# Use a specific profile
peeroxide chat --profile work join "engineering"

# Send a direct message (sends invite via inbox, then joins DM channel)
peeroxide chat dm <pubkey-hex>

# Show your identity
peeroxide chat whoami

# List profiles
peeroxide chat profiles
```

### Open Questions

- [ ] TUI vs line-mode (TUI is better UX, more implementation work)
- [ ] Multiple rooms simultaneously (likely yes, separate polling tasks)
- [ ] Message history depth on join (configurable?)
- [ ] Notification mechanism for background channels

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
| All messages encrypted | Yes, including public channels | Author identity hidden inside ciphertext; screen_name only in encrypted messages |
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
| Screen name location | Inside encrypted messages only (not in feed record) | Prevents DHT nodes from building identity profiles from feed metadata |

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
| Max payload | ~1100 bytes |
| Addressing | `hash(value)` -- immutable |
| Authentication | None (content-addressed) |
| Multi-writer | N/A (content-addressed, anyone can re-put) |

### A.2 -- mutable_put / mutable_get

Signed, updateable storage. `target = hash(public_key)`. Only key holder
can update.

| Property | Detail |
|----------|--------|
| Max payload (value) | ~1002 bytes |
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
