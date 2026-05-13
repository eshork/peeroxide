# Chat `--debug` Flag — Working Note

> **Status**: working / historical design note. The `--debug` flag is implemented; this file is proposed for removal — see the PR description's Working Files table. For the current `--debug` behavior, see [`docs/src/chat/user-guide.md`](../docs/src/chat/user-guide.md) and [`docs/src/chat/reference.md`](../docs/src/chat/reference.md).

We need to add a `--debug` flag to the chat commands that enables logging of specific high value events for debugging purposes.
This would include high level network events with correlation IDs for tracing, such as:
- Nexus record updates (with pubkey and changed field)
- New invites received (with invite ID and sender pubkey)
- New messages received (with sender pubkey and message ID)
The aim is to keep these messages concise and focused on key events that are useful for understanding the system's behavior and diagnosing issues, without overwhelming users with too much information, so avoid logging full message contents or large data dumps and instead focus on metadata and correlation IDs that can be used to trace related events across the system.
This is expected to be helpful for development and troubleshooting without overwhelming users with large log dumps.

An example of the expected log output when `--debug` is enabled might look like:
```
[2024-07-01 12:00:00] [DEBUG] Update Nexus record: [mutable_put] id_keypair=abc123...def456, seq=2, name_len=8, bio_len=30
[2024-07-01 12:00:05] [DEBUG] Message received: [immutable_put] msg_hash=fedcba...123abc, author=abc123...def456, prev_hash=cafe00...00cafe, ts=1719829205, content_type=0x01
[2024-07-01 12:00:10] [DEBUG] Feed record discovered: [mutable_put] feed_pubkey=feed00...00feed, id_pubkey=abc123...def456, msg_count=5, next_feed=0x00...00
[2024-07-01 12:00:15] [DEBUG] Summary block: [immutable_put] summary_hash=feed00...00feed, id_pubkey=abc123...def456, msg_count=26, prev_summary=cafe00...00cafe
[2024-07-01 12:00:20] [DEBUG] Invite received: [mutable_put] invite_id=inv000...00inv, sender=abc123...def456, invite_type=0x01, payload_len=256
[2024-07-01 12:00:25] [DEBUG] Inbox nudge: [mutable_put] feed_pubkey=feed00...00feed, sender=abc123...def456, next_feed=feed11...11feed
[2024-07-01 12:00:30] [DEBUG] Inbox check: [lookup] topic=inbox:epoch=1719829200:bucket=0, results=1
[2024-07-01 12:00:30] [DEBUG] Inbox check: [lookup] topic=inbox:epoch=1719829200:bucket=1, results=0
[2024-07-01 12:00:30] [DEBUG] Inbox check: [lookup] topic=inbox:epoch=1719829200:bucket=2, results=2
[2024-07-01 12:00:30] [DEBUG] Inbox check: [lookup] topic=inbox:epoch=1719829200:bucket=3, results=0
```
These are loose examples of the types of events and metadata that could be logged, and you are expected to include additional relevant events and metadata as you see fit during implementation. The key is to focus on high-level events that provide insight into the system's behavior and can be correlated for tracing, without overwhelming users with too much detail.
