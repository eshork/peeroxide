# Interactive TUI

Peeroxide chat features a terminal-based interactive interface (TUI) designed for real-time communication.

## Mode Selection

The TUI is automatically enabled if:
1. `stdout` is a TTY.
2. `stdin` is a TTY.
3. The `--line-mode` flag is not set.
4. The `PEEROXIDE_LINE_MODE` environment variable is unset, empty, or `"0"` (any other non-empty value forces line mode).

If any of these conditions are not met, the client falls back to line mode. If TUI initialization fails on a TTY, a warning is printed and the client reverts to line mode.

## Status Bar Layout

The status bar sits at the bottom of the terminal and provides real-time feedback on network activity and session state.

```text
●  Sending 3  Receiving 12              inbox              Feeds 2  DHT 32  general
```

### Components

- **Activity Indicator (●)**: Lights up when any DHT operation (put, get, announce, lookup) is in flight.
- **Left Segments**:
    - `Sending N`: Number of messages currently in the publish batching pipeline.
    - `Receiving N`: Number of messages currently being fetched or ordered.
    - `Ready`: Indicates the publisher queue is empty and the client is idle.
    - *Note*: These slots use "sticky width"—once they grow to accommodate a larger number, they remain that size until the terminal is resized.
- **Center Segment**: 
    - Shows `inbox` (or `i`) when there are no unread invites.
    - Shows `INBOX` (or `I`) in yellow-on-black when new invites have arrived.
    - The segment is centered. It automatically shrinks or disappears if the terminal width is too narrow to avoid overlapping left or right segments.
- **Right Segments**:
    - `Feeds N`: Total number of active feeds being tracked in the session.
    - `DHT N`: Current number of connected peers in the DHT routing table.
    - `<channel>`: The name of the current channel or the recipient's name.

## Keyboard Controls

| Key | Behavior |
|---|---|
| `Enter` | Send the current input buffer. |
| `Ctrl-C` | If buffer is non-empty: Clear the buffer. If buffer is empty: Arms a 2-second force-quit window. |
| `Ctrl-D` | If buffer is empty: Initiate graceful exit. If non-empty: Forward-delete character. |
| `Ctrl-L` | Full screen repaint and history replay. |
| `Up/Down` | Move the cursor up or down within the multi-line input area. |

### Ctrl-C Force Quit

When the buffer is empty, pressing `Ctrl-C` once will display a yellow-on-black notice:
`*** press Ctrl-C again within 2 seconds to force quit — press Ctrl-D for graceful exit`

Pressing `Ctrl-C` a second time within the 2-second window will terminate the process immediately. This remains responsive even if the network publisher is blocked. Any other key disarms the window.

## Slash Commands

Commands can be entered directly into the input buffer starting with a `/`.

| Command | Action |
|---|---|
| `/help`, `/?` | Display available commands. |
| `/quit`, `/exit` | Initiate a graceful shutdown. |
| `/ignore [name]` | List ignored users, or add a user to the ignore list. |
| `/unignore <name>`| Remove a user from the ignore list. |
| `/friend [name]` | List friends, or add a user to your friends list. |
| `/unfriend <name>`| Remove a user from your friends list. |
| `/inbox` | Display and drain the list of accumulated invites. Resets the INBOX status segment. |

## Input Features

### Multi-line Input
The input area above the status bar supports multi-line text. Use `Alt-Enter` (or your terminal's equivalent) to insert a newline without sending.

### Bracketed Paste
The TUI supports bracketed paste mode. When you paste large blocks of text, the client treats it as a single input operation, preventing the terminal from interpreting pasted newlines as "Send" commands.

### History Replay
The client maintains a bounded in-memory scrollback buffer of the last 500 messages (`HISTORY_CAP`). When the terminal is resized or repainted (`Ctrl-L`), the client replays the last `min(history_len, terminal_height)` entries to restore context.

## Terminal Lifecycle

The `TerminalGuard` ensures the terminal state is correctly managed:
- **Enter**: Scrolls existing terminal content into the scrollback, enables raw mode, enables bracketed paste, hides the cursor, and installs a panic hook.
- **Exit/Panic**: Resets the scroll region, restores the cursor, disables bracketed paste, restores original colors, and disables raw mode.

## EOF and Shutdown

When `stdin` reaches EOF (e.g., via `Ctrl-D` or piped input completion):
- **Default**: The client begins a graceful drain. It displays `*** flushing publish queue (Ctrl-C to abort)…` and waits for all pending messages to be published to the DHT. There is no fixed timeout, though `Ctrl-C` can be used to skip the wait.
- **--stay-after-eof**: Instead of exiting, the client enters read-only listener mode, allowing you to continue seeing incoming messages without being able to reply.
