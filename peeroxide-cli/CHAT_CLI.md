# peeroxide-chat CLI Design

> Command-line interface for peeroxide-chat. Each command is a long-running
> process managing its own DHT connection, feed, and polling loop. Users run
> multiple instances for multiple conversations.

See [`CHAT.md`](./CHAT.md) for the protocol specification.

---

## Architecture

### Process Model

Each `peeroxide chat` subcommand is an **independent long-running process**:
- Own UDP socket and DHT node (separate port per process)
- Own feed keypair (random per session)
- Own polling loop (discovery + content)
- No IPC, no daemon, no shared mutable state

Users manage multiple conversations via multiple terminals (or tmux panes,
background jobs, etc.). This matches the existing `peeroxide` CLI style
where `cp` and `dd` are also long-running.

### Shared State (Read-Only)

All processes share the **identity profile** on disk (read-only):
- `~/.config/peeroxide/chat/profiles/<name>/seed` — Ed25519 seed (32 bytes)
- `~/.config/peeroxide/chat/profiles/<name>/name` — optional display name
- `~/.config/peeroxide/chat/profiles/<name>/bio` — optional bio text
- `~/.config/peeroxide/chat/profiles/<name>/friends` — friends list (pubkeys + aliases + cached nexus)
- `~/.config/peeroxide/chat/profiles/<name>/known_users` — seen users cache (pubkey → last screen name)

No file locking needed — profile files are read-only during chat sessions
(only `chat profiles`, `chat nexus --set-*`, and `chat friends` modify them).
Runtime state (known feeds, cached messages, seq numbers) lives entirely
in memory and is discarded on exit.

**Known users cache**: Every chat process appends to `known_users` when it
encounters a new `id_pubkey` (from a decrypted message or feed record).
Stores the full pubkey and last-seen screen name. This allows users to
look up full pubkeys later for friending, even for resolved users whose
full key was never displayed. The file is append-friendly (no coordination
needed between processes; duplicates are harmless and deduped on read).

### Nexus Publishing

Every active chat process (`join`, `dm`, `inbox`) automatically refreshes
the user's personal nexus record every ~8 minutes (same cadence as feed
refresh). This ensures the user's screen name and bio are discoverable
by other participants without running a dedicated process.

- Nexus content is read from the profile directory on each refresh
- Seq number uses `unix_time_secs` — no coordination between processes;
  last writer wins, content is always the latest on-disk version
- Multiple processes pushing the same content is harmless (idempotent)
- If the user edits their profile mid-session (via `chat nexus --set-*`),
  the next refresh cycle picks up the change automatically
- `--no-nexus` disables nexus publishing only. User can still post.
- `--read-only` disables all write operations (no posting, no feed creation,
  no announce). Pure listener mode — the ultimate lurker. User can read any
  channel, DM, or inbox they have keys for, but produces zero DHT writes.
- `--stealth` combines `--no-nexus` + `--read-only` + `--no-friends`.
  Zero `put` or `announce` operations, and no friend nexus lookups that could
  reveal interest patterns to DHT nodes. The user is invisible to the network
  beyond the minimum read-only DHT queries needed to receive messages. May
  gain additional behavior in the future.

### Trade-offs Accepted

- Duplicate bootstrap/routing table warmup per process (~2-3s each)
- No cross-session notifications or unified unread state
- No persistent message history (ephemeral by design; local caching is
  a future enhancement, not v1)

If these become painful, a shared background DHT node (via Unix socket)
can be added later without changing the command surface.

---

## Commands

### `peeroxide chat join <channel>`

Join a channel and participate interactively.

```
peeroxide chat join <channel> [OPTIONS]

Arguments:
  <channel>    Channel name (used to derive channel_key)

Options:
  --group <salt>       Private channel with group name as salt
  --keyfile <path>     Private channel with keyfile as salt
  --profile <name>     Identity profile to use (default: "default")
  --no-nexus           Do not publish personal nexus
  --read-only          Listen only; no posting, no feed, no announce
  --stealth            Equivalent to --no-nexus --read-only (zero DHT writes)
  --feed-lifetime <duration>  Max feed keypair lifetime before rotation
                              (default: 60m, with ±50% wobble)
```

**Behavior:**
1. Load identity from profile
2. Bootstrap DHT node
3. Derive channel_key from channel name (+ salt if private)
4. Generate random feed keypair
5. Perform join scan (20 epochs × 4 buckets = 80 lookups)
6. Enter main loop:
   - Discovery: scan current + previous epoch (8 lookups, every 5-8s)
   - Content: poll known feeds, fetch new messages, display
   - Input: read lines from stdin, post as messages
7. On feed rotation: generate new keypair, set next_feed_pubkey, overlap

**Output format (stdout):**
```
[2026-05-04 14:23:01] [(alice)]: hello everyone
[2026-05-04 14:23:05] [<bob@7e4a1b2c>]: hey alice!
```

**Input (stdin):**
Lines typed are posted as messages. Empty lines are ignored.

**Exit:** Ctrl-C or EOF on stdin. Feed expires naturally via TTL.

---

### `peeroxide chat dm <pubkey>`

Start or resume a DM conversation.

The DM channel is **deterministic** — both parties can independently derive
the channel_key from their sorted pubkeys. No invite is required for access.
The invite inbox is purely a notification mechanism ("hey, check our DM").

```
peeroxide chat dm <pubkey-hex> [OPTIONS]

Arguments:
  <pubkey-hex>   Recipient's identity public key (64-char hex)

Options:
  --profile <name>     Identity profile to use (default: "default")
  --no-nexus           Do not publish personal nexus
  --read-only          Listen only; no posting, no feed, no announce
  --stealth            Equivalent to --no-nexus --read-only (zero DHT writes)
  --message <text>     Message to include in the startup inbox nudge
  --feed-lifetime <duration>  Max feed keypair lifetime (default: 60m)
```

**Behavior:**
1. Load identity from profile
2. Bootstrap DHT node
3. Derive DM channel_key from sorted pubkeys
4. Generate random feed keypair for DM channel
5. Perform join scan on DM topic (20 epochs × 4 buckets)
6. **Startup inbox nudge (only if `--message` provided):** Poke recipient's
   inbox once — announce a temporary invite feed containing Alice's identity
   + the lure text. This says "hey, come talk to me" and gives Bob a
   reason to open the DM. No `--message` = no startup nudge.
7. Enter main loop (same as `chat join` but on DM topic)
8. **Per-message inbox nudge (v1 policy):** When posting a message, poke
   the recipient's inbox — but at most once per epoch (~1 min). The nudge
   reuses the same invite_feed_keypair (incrementing seq) so Bob's client
   can recognize it as a re-ping for an existing DM, not a new invitation.
   The nudge payload contains the message text that triggered it (truncated
   to fit the invite payload budget). Bob's inbox client may truncate
   further for display.

**Output/Input:** Same format as `chat join`.

---

### `peeroxide chat inbox`

Monitor the invite inbox and display incoming invitations.

```
peeroxide chat inbox [OPTIONS]

Options:
  --profile <name>     Identity profile to use (default: "default")
  --poll-interval <secs>  Inbox polling interval (default: 15s)
```

**Behavior:**
1. Load identity from profile
2. Bootstrap DHT node
3. Enter polling loop:
   - Scan inbox topics (current + previous epoch × 4 buckets, every 15-30s)
   - For each new invite feed: fetch, decrypt, verify
   - Display invite details with copy-paste command

**Output format:**
```
[INVITE #1] DM from alice (a3f2b...c891)
  → peeroxide chat dm a3f2b...c891 --profile default

[INVITE #2] Channel "engineering" from bob (7e4a1...d023)
  → peeroxide chat join "engineering" --group "TeamX" --profile default
```

Invites that fail decryption or verification are silently discarded.
Invites for channels already joined (if detectable) are noted but not
re-displayed.

---

### `peeroxide chat whoami`

Display the current profile's identity.

```
peeroxide chat whoami [OPTIONS]

Options:
  --profile <name>     Profile to display (default: "default")
```

**Output:**
```
Profile: default
Public key: a3f2b4c5...(64 hex chars)
Screen name: alice
Nexus topic: 7f8e9a...  (for others to look up your profile)
```

---

### `peeroxide chat profiles`

List available profiles.

```
peeroxide chat profiles [SUBCOMMAND]

Subcommands:
  list       List all profiles (default)
  create <name> [--screen-name <name>]   Create a new profile
  delete <name>                          Delete a profile
```

**Output (list):**
```
  default    a3f2b4c5...  (alice)
  work       7e4a1b2c...  (bob-work)
  throwaway  9c8d7e6f...  (no screen name)
```

---

### `peeroxide chat nexus`

Manage the personal nexus (public profile record). When run standalone,
also acts as a friend refresh loop — continuously updating cached nexus
data for all friends in the background.

```
peeroxide chat nexus [OPTIONS]

Options:
  --profile <name>       Profile to manage (default: "default")
  --set-name <name>      Update screen name
  --set-bio <text>       Update bio
  --publish              Publish/refresh nexus to DHT (one-shot, then exit)
  --lookup <pubkey-hex>  Look up another user's nexus (one-shot, then exit)
  --daemon               Run continuously: publish own nexus + refresh friends
```

When run with `--daemon` (or no one-shot flags), enters a long-running loop:
- Publishes own nexus every ~8 minutes
- Cycles through friends list, refreshing one friend's nexus per ~30s
- Updates cached screen names/bios in the friends file
- Useful as a background process for keeping friend data fresh

---

### `peeroxide chat friends`

Manage the local friends list (known pubkeys + cached metadata).

```
peeroxide chat friends [SUBCOMMAND]

Subcommands:
  list                                Show all friends with cached info
  add <key> [--alias <name>]          Add a friend. <key> can be:
                                        - full 64-char hex pubkey
                                        - shortkey (8 hex chars, e.g., "a3f2b4c5")
                                        - name@shortkey (e.g., "alice@a3f2b4c5")
                                      Resolved from known_users cache. Errors if
                                      shortkey not found in cache.
  remove <key>                        Remove a friend (same resolution as add)
  refresh                             One-shot: fetch nexus for all friends now
```

**Storage:** `~/.config/peeroxide/chat/profiles/<name>/friends`
(simple format: one pubkey per line, with optional alias and cached nexus data)

**Opportunistic refresh:** All active chat processes (`join`, `dm`, `inbox`,
`nexus --daemon`) automatically cycle through the friends list in the
background, refreshing one friend's nexus per poll cycle (round-robin).
With 20 friends at 5-8s intervals, the full list refreshes in ~2-3 minutes.
This is negligible overhead on top of existing feed polling.

- `--no-friends` flag on any command disables this behavior
- `--stealth` also disables friend refresh (mutable_get for known pubkeys
  reveals interest patterns to DHT nodes serving those nexus addresses)

---

## Message Format (Display)

All commands that display messages use the same format:

```
[TIMESTAMP] [DISPLAY_NAME]: MESSAGE_CONTENT
```

- Timestamp: local time, `YYYY-MM-DD HH:MM:SS`
- Display name wrapped in `[]` with `:` separator to clearly delimit identity from message text
- Messages from self are displayed immediately (no round-trip wait)

### Display Name Rules

The delimiter itself signals trust level:
- **`()` = friend** (trusted, locally controlled)
- **`<>` = not friend** (untrusted, user-controlled content)

| Situation | Format | Example |
|-----------|--------|---------|
| Friend, alias matches screen name | `[(alias)]` | `[(alice)]` |
| Friend, alias differs from screen name | `[(alias) <screen_name>]` | `[(bob) <alice>]` |
| Non-friend, has screen name | `[<screen_name@shortkey>]` | `[<alice@7e4a1b2c>]` |
| Non-friend, no screen name | `[<@shortkey>]` | `[<@c9d8e7f6>]` |
| Non-friend, name recently changed | `[<!screen_name@shortkey>]` | `[<!alice@7e4a1b2c>]` |

- **`()`** is your local alias — impossible to spoof
- **`<>`** contains untrusted content (screen name, shortkey)
- **Shortkey** = first 8 hex chars of pubkey (4 bytes, ~4 billion values)
- **`!` prefix** inside `<>` = name-change warning; active for a cooldown
  period (default ~10 min) after a screen name change is detected.
  Resets if they change again.
- When both are shown (`(bob) <alice>`), the friend changed their screen
  name to something different from your alias — your alias remains stable

### Name Change Handling

When a screen name change is detected (compared against `known_users` cache):

```
*** alice@7e4a1b2c changed screen name: "charlie" → "alice"
[14:23:15] [<!alice@7e4a1b2c>]: haha I'm alice now
```

- **Non-friends**: `!` warning prefix active until cooldown expires
- **Friends with alias**: system message shown for awareness, but display
  is unaffected (alias is your local truth)
- **Friends without alias**: `!` warning applies (their name is self-chosen)
- Cooldown resets on each subsequent name change

### Identity System Messages

Full 64-char pubkey printed for non-friend users, triggered by receiving
a message from them when the last identity line for that user was >10
minutes ago (or never shown). This means:

- First message from them → identity line shown
- Rapid messages → no repeat (already shown recently)
- Gap of >10 min then another message → identity line shown again
- Friends never get identity lines (alias is your identifier)

```
*** @a3f2b4c5 is a3f2b4c5d6e7f80910111213141516171819202122232425262728293031
[14:23:01] [<@a3f2b4c5>]: hey man, what's up?
```

The identity line always appears immediately before the message that
triggered it, so the full pubkey is visually adjacent and easy to copy.

### System Events

```
*** alice joined (new feed discovered)
*** feed rotated: alice (new feed key)
*** connection established with DHT (X peers in routing table)
*** alice@7e4a1b2c changed screen name: "bob" → "alice"
*** — live —
```

The `— live —` separator is printed once after the cold-start scan
completes and all backlog messages have been displayed. Everything above
it is history; everything below is real-time. This helps users distinguish
"who just said something" from backlog they're catching up on.

---

## Signals and Lifecycle

- **SIGINT / Ctrl-C**: graceful shutdown. Stop polling, let feed expire
  naturally (no explicit "leave" announcement needed).
- **SIGTERM**: same as SIGINT.
- **stdin EOF**: stop accepting input but continue displaying messages
  (read-only mode). Second EOF or SIGINT exits.
- **DHT bootstrap failure**: retry with backoff, print warning. Exit after
  N failures (configurable).

---

## Future Enhancements (Not v1)

- `--json` output mode for machine consumption / piping to other tools
- Local message cache (SQLite) for history persistence across sessions
- Optional shared DHT node (background daemon) to reduce bootstrap cost
- TUI mode (`--tui`) with ncurses/ratatui for multi-pane single-process UX
- File/image attachments via chunked immutable_put (like `peeroxide dd`)
- Read receipts (optional, opt-in)
- Group administration commands (kick, invite-only enforcement)
