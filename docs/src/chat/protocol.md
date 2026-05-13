# Operational Protocol

The Peeroxide chat protocol defines how peers discover each other, synchronize message feeds, and maintain a consistent conversation state without a central server.

## Feed Lifecycle

A "feed" is a sequence of messages published by a single identity under a temporary Ed25519 keypair.

### Rotation
To enhance privacy and limit the impact of key compromise, feed keypairs are rotated periodically.
1. At session start, a random feed keypair and a lifetime wobble (between 0.5x and 1.5x of `--feed-lifetime`) are chosen.
2. A rotation watcher checks the feed age every 30 seconds.
3. When the lifetime is reached, the publisher generates a new feed keypair.
4. The publisher first announces the new feed.
5. It then updates the old `FeedRecord` to include the `next_feed_pubkey` pointer.
6. The old feed remains active briefly to ensure peers follow the transition before it is abandoned.

## Message Publishing Pipeline

The publisher uses a bounded queue to batch and write messages to the DHT.

1. **Batching**: Messages are accumulated in a queue. A batch is processed when it reaches `--batch-size` or after `--batch-wait-ms`.
2. **Immutable Put**: Each message in the batch is stored as an immutable record on the DHT.
3. **Mutable Put**: The `FeedRecord` for the current feed is updated to include the hashes of the new messages. This operation is retried up to 3 times (at 200ms, 500ms, and 1000ms intervals) to handle DHT congestion.
4. **Announce**: The publisher announces the feed's availability on the channel's `announce_topic`.

## Reader Discovery Loop

The reader task starts with a one-shot cold-start scan, then settles into a steady-state discovery loop.

### Cold-Start Historical Scan
On startup, the reader fires concurrent lookups across the **last 20 epochs × 4 buckets = 80 discovery topics** (i.e. a 20-minute backwards window, since each epoch is 60 s). This surfaces feeds that announced before the session started so the client has visible history immediately, instead of waiting up to a full epoch rotation for the steady-state loop to reach them.

### Steady-State Loop
After the cold-start completes, the continuous loop runs:

1. **Discovery**: Every 8 seconds, the reader performs lookups on the 8 discovery topics (current and previous epoch across 4 buckets).
2. **Polling**: For every discovered peer, the reader fetches and decrypts their `FeedRecord`.
3. **Fetching**: New message hashes found in the `FeedRecord` are fetched as immutable records.
4. **Ordering**: Messages are passed to the `ChainGate` for causal ordering.

## Ordering and Deduplication

### DedupRing
The `DedupRing` is a FIFO cache with a capacity of 1000 hashes. It ensures that the client never processes or displays the same message twice, even if it is rediscovered through different feeds or topics.

### ChainGate
The `ChainGate` enforces strict ordering based on the `prev_msg_hash` field in each `MessageEnvelope`.
- If a message arrives and its `prev_msg_hash` matches the last seen message from that sender, it is released to the UI.
- If it doesn't match, it is buffered, and the reader triggers a refetch of the missing hash with an exponential backoff.
- If a gap remains for more than 5 seconds (`GAP_TIMEOUT`), the `ChainGate` force-releases the buffered messages, marking them as `late`.

## History and Eviction

The `FeedRecord` has a limited capacity (max 26 hashes). When the message count reaches `SUMMARY_EVICT_TRIGGER` (20), the publisher performs an eviction.

1. The 15 oldest messages (`SUMMARY_EVICT_COUNT`) are moved into a new `SummaryBlock`.
2. The `SummaryBlock` is stored as an immutable record.
3. The `FeedRecord` is updated to point to the new `SummaryBlock` hash and contains only the remaining 5 newest messages.
4. On a cold start, a reader can walk back through these `SummaryBlock` pointers up to a `MAX_SUMMARY_DEPTH` of 100 blocks.

## Inbox and Invites

The inbox monitor handles parallel scanning for new invites.

1. **Snapshot**: The monitor takes a snapshot of currently known feed sequences.
2. **Parallel Scan**: It fires 8 concurrent DHT lookups for the 8 inbox topics.
3. **Resolution**: Peer pubkeys found in the topics are fanned out into parallel `mutable_get` calls to retrieve `InviteRecord`s.
4. **Verification**: Invites are decrypted using the `invite_key` (derived via ECDH) and verified.
5. **Nudge**: In DM sessions, a "nudge" is sent at most once per epoch to signal the sender's presence. A nudge is an encrypted `InviteRecord` published via `mutable_put` on the sender's invite-feed keypair (with the lure payload truncated to 800 bytes), followed by an `announce` on the recipient's current inbox topic. This matches the regular inbox-invite write path.

## Graceful Shutdown

Upon exit, the client attempts a clean teardown:
1. **Publisher Drain**: It waits for the publish queue to empty.
2. **Invite Retraction**: For DM sessions, it attempts to retract the inbox invite by publishing an empty payload to the invite feed with a 1-second timeout.
3. **Terminal Reset**: The TUI is disabled and terminal settings are restored.
