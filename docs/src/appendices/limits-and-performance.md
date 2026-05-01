# Limits and Performance

This appendix documents hard limits, configurable bounds, and observed performance characteristics of the `peeroxide-cli` tools.

## Hard Limits

| Constant | Value | Context |
|---|---|---|
| `MAX_ECHO_SESSIONS` | 64 | Concurrent echo sessions per `announce` process |
| `HANDSHAKE_TIMEOUT` | 5 s | Echo protocol handshake timeout |
| `IDLE_TIMEOUT` | 30 s | Echo session idle timeout |
| `ECHO_MSG_LEN` | 16 bytes | Echo probe frame size (fixed) |
| `ECHO_TIMEOUT` (ping) | 5 s | Per-probe timeout in `ping --connect` mode |
| `MAX_CHUNKS` | 65 535 | Maximum chunks in a single `deaddrop` message |
| `MAX_PAYLOAD` | 1 000 bytes | Maximum payload per `deaddrop` chunk |
| `ROOT_HEADER_SIZE` | 39 bytes | `deaddrop` root chunk header size |
| `NON_ROOT_HEADER_SIZE` | 33 bytes | `deaddrop` non-root chunk header size |
| `CHUNK_SIZE` (cp) | 65 536 bytes | `cp` file chunk size |
| `--data` max (announce) | 1 000 bytes | Maximum `--data` payload for `announce` |
| lookup `--with-data` concurrency | 16 | `buffer_unordered(16)` for mutable DHT gets |
| ping topic mode peer cap | 20 | Maximum peers probed per topic lookup |

## Derived Limits

**Maximum `deaddrop` message size:**

```
MAX_CHUNKS × MAX_PAYLOAD = 65 535 × 1 000 = ~65.5 MB
```

Practical limit is lower due to DHT value size constraints and network latency.

**Maximum `cp` file size:**

Limited by available DHT storage and client memory. Each 65 536-byte chunk is stored as a separate immutable DHT value. There is no hard-coded upper bound in the CLI, but very large files will require many round-trips.

## Timing

| Parameter | Value | Notes |
|---|---|---|
| `announce` refresh interval | 600 s | Background mutable put to keep slot alive |
| `announce` seq | Unix epoch seconds | Two refreshes in the same second produce identical seq — see [ISSUES.md](https://github.com/Rightbracket/peeroxide/blob/main/ISSUES.md) |
| `ping --interval` default | 1.0 s | Configurable |
| `ping --count` default | 1 | 0 = infinite |

## Concurrency

- `lookup --with-data` fetches peer data in parallel with a concurrency window of 16 (`buffer_unordered(16)`).
- `announce` handles incoming connections concurrently; echo sessions are bounded by `MAX_ECHO_SESSIONS = 64`.
- `cp` uploads/downloads chunks sequentially per file (parallelism may be added in future releases).

## Exit Codes

| Code | Meaning | Tools |
|---|---|---|
| 0 | Success / clean shutdown | all tools |
| 1 | Fatal error | all tools |
| 130 | SIGINT received | `lookup`, `ping` |
| 0 | SIGINT/SIGTERM received | `announce` (intentional — clean shutdown is success) |

Note: `announce` returns 0 on SIGINT/SIGTERM because interactive shutdown is the normal workflow. `lookup` and `ping` return 130 to allow callers to distinguish interruption from success.
