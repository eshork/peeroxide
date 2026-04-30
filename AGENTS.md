# AGENTS.md — peeroxide workspace root

This is the root of the peeroxide workspace — a Rust implementation of the Hyperswarm P2P networking stack, wire-compatible with the Node.js reference implementation.

## Workspace Crates

| Crate | Description |
|---|---|
| `peeroxide` | High-level swarm management and topic-based peer discovery |
| `peeroxide-dht` | HyperDHT: Kademlia routing, Noise handshakes, hole-punching, relay |
| `libudx` | UDX reliable UDP transport with BBR congestion control |
| `peeroxide-cli` | CLI toolkit: lookup, announce, ping, cp, deaddrop |

## Key Files

- `DOCS_PLAN.md` — Documentation build progress tracker (checkbox-based)
- `ISSUES.md` — Source-level issues discovered during documentation and review
- `PR-TODOS.md` — Outstanding pre-merge checklist
- `RALPH_PROMPT.md` — Ralph Loop spec for automated documentation build
- `docs/` — mdBook documentation source (build: `mdbook build docs/`)
- `.github/workflows/` — CI (`ci.yml`), release (`release.yml`), docs (`docs-site.yml`)

## Build

```bash
# Build all crates
cargo build --workspace

# Run tests
cargo test --workspace

# Build documentation site
mdbook build docs/
```

## Architecture

```
peeroxide          — topic-based peer discovery + connection management
└── peeroxide-dht  — Kademlia DHT, Noise handshakes, hole-punching, relay
    └── libudx     — reliable UDP transport with BBR congestion control
```

## Agent Notes

- Do not modify `peeroxide-cli/DEADDROP_V2.md` — reference only.
- Do not push to remote — commits are local until explicitly requested.
- All documentation lives under `docs/` — use `mdbook build docs/` to verify.
- Echo protocol is documented exactly once in `docs/src/announce/echo-protocol.md`.
- MSRV: Rust 1.85 (2024 edition).
