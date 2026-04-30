# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0](https://github.com/Rightbracket/peeroxide/compare/libudx-v1.1.0...libudx-v1.2.0) - 2026-04-30

### Changed

- `UdxSocket` instances held internally by the I/O layer are now wrapped in `Arc`, allowing them to be shared with the DHT layer for UDX stream multiplexing over the same bound socket. No change to the public API.

## [1.1.0](https://github.com/Rightbracket/peeroxide/compare/libudx-v1.0.1...libudx-v1.1.0) - 2026-04-28

### Other

- Add #[non_exhaustive] to public structs and enums ([#10](https://github.com/Rightbracket/peeroxide/pull/10))
