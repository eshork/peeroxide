# Wire Format

Peeroxide chat uses a structured wire format for all data exchanged over the DHT. All records are encrypted within a common frame.

## Encryption Frame

Every record (Message, Feed, Summary, Nexus, Invite) is encapsulated in an XSalsa20-Poly1305 encryption frame.

```text
[nonce: 24 bytes] [tag: 16 bytes] [ciphertext: variable]
```

- **Cipher**: XSalsa20-Poly1305.
- **Nonce**: 24-byte random nonce generated per message.
- **Tag**: 16-byte authentication tag.
- **No AAD**: No additional authenticated data is used in the frame.

## Record Types

### MessageEnvelope

The `MessageEnvelope` represents a single chat message.

| Field | Size | Description |
|---|---|---|
| `id_pubkey` | 32 | Ed25519 public key of the author. |
| `prev_msg_hash` | 32 | Blake2b hash of the previous message in this feed's chain. |
| `timestamp` | 8 | Unix timestamp in seconds (u64 Little Endian). |
| `content_type` | 1 | `0x01` for text. |
| `screen_name_len`| 1 | Length of the screen name string. |
| `screen_name` | var | UTF-8 encoded screen name. |
| `content_len` | 2 | Length of the content (u16 Little Endian). |
| `content` | var | UTF-8 encoded message content. |
| `signature` | 64 | Ed25519 signature over the message body. |

**Signature Scheme**:
The signature covers the following bytes:
`b"peeroxide-chat:msg:v1:" || prev_msg_hash || timestamp || content_type || screen_name_len || screen_name || content`

### FeedRecord

The `FeedRecord` is a mutable record stored at a feed's public key. It acts as an index of recent messages.

| Field | Size | Description |
|---|---|---|
| `id_pubkey` | 32 | Author's permanent public key. |
| `ownership_proof`| 64 | Proof that `id_pubkey` owns this feed. |
| `next_feed_pubkey`| 32 | Pointer to the next feed after rotation (all zeros if none). |
| `summary_hash` | 32 | Hash of the latest `SummaryBlock` for this feed. |
| `msg_count` | 1 | Number of message hashes in this record (max 26). |
| `msg_hashes` | 32 * N | Array of message hashes, newest-first. |

**Ownership Proof**:
An Ed25519 signature by the `id_pubkey` over:
`b"peeroxide-chat:ownership:v1:" || feed_pubkey || channel_key`

### SummaryBlock

The `SummaryBlock` is an immutable record used to store history that has been evicted from the `FeedRecord`.

| Field | Size | Description |
|---|---|---|
| `id_pubkey` | 32 | Author's public key. |
| `prev_summary_hash`| 32 | Hash of the previous `SummaryBlock` (all zeros if none). |
| `msg_count` | 1 | Number of hashes in this block. |
| `msg_hashes` | 32 * N | Array of message hashes, oldest-first. |
| `signature` | 64 | Ed25519 signature. |

**Signature Scheme**:
Covers: `b"peeroxide-chat:summary:v1:" || prev_summary_hash || msg_hashes...`

### NexusRecord

The `NexusRecord` contains profile information published to the author's personal topic.

| Field | Size | Description |
|---|---|---|
| `name_len` | 1 | Length of the screen name. |
| `name` | var | UTF-8 encoded screen name. |
| `bio_len` | 2 | Length of the biography (u16 Little Endian). |
| `bio` | var | UTF-8 encoded biography. |

### InviteRecord

Used for DMs and private channel invites in the Inbox.

| Field | Size | Description |
|---|---|---|
| `id_pubkey` | 32 | Author's public key. |
| `ownership_proof`| 64 | Ownership proof (same as FeedRecord). |
| `next_feed_pubkey`| 32 | Next feed pointer. |
| `invite_type` | 1 | `0x01` = DM, `0x02` = Private Channel. |
| `payload_len` | 2 | Length of the payload (u16 Little Endian). |
| `payload` | var | Encrypted payload (see below). |

**DM Payload**: Opaque lure text.
**Private Invite Payload**: `[name_len: u8][name][salt_len: u16 LE][salt]`.

## Key Derivation

All derivation functions use keyed BLAKE2b-256.

| Key | Derivation Formula |
|---|---|
| `channel_key` (Public) | `hash([b"peeroxide-chat:channel:v1:", len4(name), name])` |
| `channel_key` (Private)| `hash([b"peeroxide-chat:channel:v1:", len4(name), name, b":salt:", len4(salt), salt])` |
| `dm_channel_key` | `hash([b"peeroxide-chat:dm:v1:", min(pk_a, pk_b), max(pk_a, pk_b)])` |
| `msg_key` | `keyed_blake2b(channel_key, b"peeroxide-chat:msgkey:v1")` |
| `dm_msg_key` | `keyed_blake2b(ecdh_secret, b"peeroxide-chat:dm-msgkey:v1:" || channel_key)` |
| `invite_key` | `keyed_blake2b(ecdh_secret, b"peeroxide-chat:invite-key:v1:" || invite_feed_pk)` |
| `announce_topic` | `keyed_blake2b(channel_key, b"peeroxide-chat:announce:v1:" || epoch_le || bucket)` |
| `inbox_topic` | `keyed_blake2b(hash(pk), b"peeroxide-chat:inbox:v1:" || epoch_le || bucket)` |

### DM ECDH
For direct messages, Ed25519 keys are converted to X25519:
- Public Key: Edwards-to-Montgomery conversion.
- Secret Key: `SHA-512(seed)[0..32]` with standard clamping.
- Shared Secret: standard `x25519` scalar multiplication.

## Epoch and Bucket Math

- **Epoch**: `unix_time_secs / 60` (60-second intervals).
- **Buckets**: 4 buckets per epoch (0, 1, 2, 3).
- **Discovery**: A client scans `(current_epoch, previous_epoch) × 4 buckets`, resulting in 8 lookups per cycle.
- **Randomization**: Each session uses a random permutation of the 4 buckets to distribute load.
