# AGENTS.md — docs/

This directory contains the peeroxide-cli mdBook documentation source.

## Structure

```
docs/
├── book.toml          — mdBook configuration (includes mdbook-mermaid preprocessor)
└── src/
    ├── SUMMARY.md     — Chapter outline and navigation tree
    ├── introduction.md
    ├── concepts/      — Shared conceptual background
    ├── lookup/        — lookup command documentation
    ├── announce/      — announce command documentation (echo protocol defined here)
    ├── ping/          — ping command documentation (cross-refs echo protocol)
    ├── cp/            — cp command documentation
    ├── dd/            — dd (Dead Drop) command documentation
    └── appendices/    — Security model, limits & performance
```

## Building

```bash
# Install prerequisites (once)
cargo install mdbook mdbook-mermaid

# Build
mdbook build docs/

# Serve locally with live reload
mdbook serve docs/
```

Output goes to `docs/book/` (gitignored).

## Conventions

- All Mermaid diagrams use ` ```mermaid ``` ` fences — rendered by `mdbook-mermaid`.
- Cross-references use relative `[text](../path/to/file.md)` links (mdBook requirement).
- Human output examples go on **stderr**; structured JSON output goes on **stdout**.
- The Echo Protocol is defined exactly once in `src/announce/echo-protocol.md`. All other chapters that reference it must link there rather than re-documenting it.
- `dd/future-direction.md` describes v2 (not yet implemented) — keep clearly labeled.

## Deployment

Docs are deployed to GitHub Pages automatically on push to `main` via `.github/workflows/docs-site.yml`.
