# Project Issues and Inconsistencies

## Bugs

### announce.rs: seq reuse when two refreshes occur within the same second
The `announce` command uses Unix epoch seconds for the `seq` field in metadata puts. If two refreshes (or the initial put and a refresh) occur within the same second, they will produce identical `seq` values. The DHT may reject a `put` with a `seq` that is less than or equal to the existing sequence number.
**File**: `peeroxide-cli/src/cmd/announce.rs` around the refresh loop.

## Inconsistencies

### announce/unannounce output inconsistency
The `ANNOUNCE` output always displays the hashed topic (e.g., `ANNOUNCE blake2b("topic")`), even if the user provided a raw 64-character hex key. However, the `UNANNOUNCE` output correctly displays the raw hex if it was provided as input (e.g., `UNANNOUNCE <hex>`). This inconsistency makes it harder to correlate start and end logs when using raw keys.
**File**: `peeroxide-cli/src/cmd/announce.rs`
