# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Renamed `deaddrop` command to `dd` (short for "Dead Drop")
- Renamed `deaddrop leave` subcommand to `dd put`
- Renamed `deaddrop pickup` subcommand to `dd get`

## [0.1.0] - 2026-04-29

### Added

- `peeroxide node` — run a long-lived DHT bootstrap node with configurable port, host, stats interval, and record limits
- `peeroxide lookup` — query the DHT for peers on a topic with `--json` NDJSON output and `--with-data` support
- `peeroxide announce` — announce presence on a topic with `--ping` echo responder, `--data` attachment, `--duration` limit, and `--seed` keypair
- `peeroxide ping` — diagnose reachability by address, public key, or topic with `--connect` full handshake, `--count`/`--interval` probes, and `--json` output
- `peeroxide cp send` / `peeroxide cp recv` — streaming file transfer over encrypted P2P connections with atomic writes, progress reporting, `--keep-alive`, `--force`, and stdin/stdout support
- `peeroxide deaddrop leave` / `peeroxide deaddrop pickup` — anonymous store-and-forward messaging via DHT mutable records with passphrase encryption, CRC32c integrity, and chunked payloads
- `peeroxide config init` — generate a default TOML configuration file
- TOML configuration system with `~/.config/peeroxide/config.toml`, `$PEEROXIDE_CONFIG` env var, and CLI flag overrides
- `--generate-man <DIR>` flag to produce roff man pages with rich descriptions, examples, exit status, and cross-references
- AIMD adaptive concurrency for deaddrop publish (responds to commit timeouts)
- SIGINT/SIGTERM graceful shutdown across all long-running commands
- Comprehensive test suite: 24 unit tests, 9 local integration tests, 4 live network tests

### Known Limitations

- Performance has not been optimized yet, particularly deaddrop throughput. This is an initial release to get something tangible out for people to play with — expect significant speed improvements in future versions.
