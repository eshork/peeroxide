# Contributing to Peeroxide

Thank you for your interest in contributing to Peeroxide. This project is a Rust implementation of the Hyperswarm stack, focusing on wire compatibility with the Node.js reference network.

## Crate Stack

The workspace consists of three primary crates:
1. **libudx**: Reliable UDP transport with BBR congestion control.
2. **peeroxide-dht**: Kademlia DHT, Noise handshakes, hole-punching, and relaying.
3. **peeroxide**: High-level swarm and topic-based discovery.

## Development Requirements

- **Rust 2024 edition** (MSRV 1.85)
- **Node.js 18+** and **npm 9+** (required for interop tests)
- **Docker** (optional, for NAT simulation tests)

## Building

Build the entire workspace using cargo:

```bash
cargo build --workspace
```

## Testing

We maintain strict interoperability with Node.js. Tests include unit tests, golden fixtures, and live interop tests.

Before running interop tests, install the Node.js dependencies:
```bash
cd peeroxide/tests/node && npm install && cd -
```

Run the test suite:
```bash
# Run all workspace tests
cargo test --workspace

# Run unit tests only
cargo test --workspace --lib

# Run interop tests (requires Node.js)
cargo test --workspace --test '*interop*'
```

For more detailed testing information and specialized test suites, see [TESTING.md](./TESTING.md).

## Code Style and Quality

We enforce high standards for code quality and documentation:

- **Clippy**: Must pass without warnings.
  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  ```
- **Documentation**: Documentation builds must have zero warnings.
  ```bash
  cargo doc --workspace --no-deps --document-private-items
  ```
- **Formatting**: Follow standard Rust formatting (rustfmt).

## Pull Request Process

1. Ensure all tests pass, including interop tests if you modified protocol-sensitive code.
2. Run clippy and check that documentation builds without warnings.
3. Keep PRs focused on a single change or feature.
4. Update or add tests for any new functionality or bug fixes.
5. All commits must be clear and descriptive.
6. For API changes, run `cargo-semver-checks` to verify SemVer compliance:
   ```bash
   cargo install cargo-semver-checks
   cargo semver-checks check-release -p <crate>
   ```

## Licensing

Peeroxide is dual-licensed under the MIT license and the Apache License (Version 2.0). By contributing, you agree that your contributions will be licensed under these terms.
