# AGENTS.md — peeroxide workspace root

This is the root of the peeroxide workspace — a Rust implementation of the Hyperswarm P2P networking stack, wire-compatible with the Node.js reference implementation.

## Workspace Crates

| Crate | Description | Published |
|---|---|---|
| `peeroxide` | High-level swarm management and topic-based peer discovery | crates.io |
| `peeroxide-dht` | HyperDHT: Kademlia routing, Noise handshakes, hole-punching, relay | crates.io |
| `libudx` | UDX reliable UDP transport with BBR congestion control | crates.io |
| `peeroxide-cli` | CLI toolkit: lookup, announce, ping, cp, dd | binary only |

The three library crates are published to crates.io and have external users. `peeroxide-cli` is a consumer of those libraries, not a library itself.

## Key Files

- `docs/` — mdBook documentation source (build: `mdbook build docs/`)
- `.github/workflows/` — CI (`ci.yml`), release (`release.yml`), docs (`docs-site.yml`)

## Build

```bash
# Build all crates
cargo build --workspace

# Run all tests (unit + integration + Node.js interop)
cargo test --workspace

# Run live network tests (requires internet — public HyperDHT bootstrap nodes)
cargo test -p peeroxide-cli --test live_commands -- --ignored

# Build documentation site
mdbook build docs/
```

## Architecture

```
peeroxide          — topic-based peer discovery + connection management
└── peeroxide-dht  — Kademlia DHT, Noise handshakes, hole-punching, relay
    └── libudx     — reliable UDP transport with BBR congestion control
```

## API Breaking Change Policy — HARD STOP

**Any change that modifies or removes an existing public API in `libudx`, `peeroxide-dht`, or `peeroxide` is a breaking change. You must stop and get explicit human approval before making one. No exceptions.**

### What counts as breaking

- Changing a function/method signature (parameter types, return type, receiver type)
- Removing a public function, method, field, or type
- Changing a public field's type
- Adding a required parameter to an existing function

### What does NOT count as breaking

- Adding new public functions, methods, or types
- Adding optional parameters via a new companion function
- Internal implementation changes with no public surface effect

### The boundary that matters

`peeroxide-cli` is a binary consumer. Constraints it places on how it uses library internals (e.g. needing to share a socket across tasks) must be solved **inside `peeroxide-cli` or by adding new non-breaking API**, never by altering existing library signatures.

If you find yourself needing to change a library signature to satisfy a CLI feature, stop. Propose an additive solution (new method, wrapper type, trait impl) and wait for approval.

### What to do when you hit this wall

1. Stop. Do not implement the breaking change.
2. Document the constraint you encountered and why it creates pressure toward a break.
3. List at least two non-breaking alternatives (e.g. new method, wrapper type, `Arc` inside the type).
4. Ask the human which approach to take.

## Test Completeness

"All tests pass" means all three suites:

1. `cargo test --workspace` — unit tests, integration tests, and the Node.js local interop test (`hyperswarm_cross_language_connect`)
2. `cargo test -p peeroxide-cli --test live_commands -- --ignored` — live public HyperDHT network tests (lookup, announce, cp, dd)

Do not mark work complete until both suites are green.

## Task Artifacts

Task-specific files (planning docs, progress trackers, spec drafts, Ralph Loop prompts, PR checklists, etc.) must **not** be committed to git unless explicitly directed by the human.

Before opening or merging a PR, scan the branch's added files (`git diff --name-only --diff-filter=A main`) and flag any task-artifact files to the human. Examples of files that should not be in git:

- `*_PLAN.md`, `*_PROMPT.md`, `*_TODOS.md`
- `DOCS_PLAN.md`, `RALPH_PROMPT.md`, `PR-TODOS.md`
- Any per-task progress trackers or scratch notes

If such a file appears, verify with the human that its inclusion is intentional before committing or pushing.

## Agent Notes

- Do not push to remote — commits are local until explicitly requested.
- MSRV: Rust 1.85 (2024 edition).

## PR Checklist

The full checklist is in [CONTRIBUTING.md](./CONTRIBUTING.md). **Check it before opening a PR or merging.** Key gates:

- Both test suites green (see Test Completeness above)
- Clippy clean
- No API breaking changes without explicit human approval
- No task-artifact files in the branch (see Task Artifacts above)
- CI green before merge
