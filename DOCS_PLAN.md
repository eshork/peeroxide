# DOCS_PLAN.md — peeroxide-cli Technical Documentation

Authoritative progress tracker for the Ralph Loop documentation build.

## Checkbox Convention

```
[ ]  not started
[/]  drafted / ready to review
[X]  written, reviewed against source, and complete
```

---

## Phase 0 — DOCS_PLAN.md

- [X] Create DOCS_PLAN.md at workspace root

---

## Phase 1 — mdBook Scaffold

- [X] `docs/book.toml`
- [X] `docs/src/SUMMARY.md`
- [X] `docs/src/introduction.md` (stub)
- [X] `docs/src/concepts/dht-and-routing.md` (stub)
- [X] `docs/src/concepts/keys-and-identity.md` (stub)
- [X] `docs/src/concepts/topics-and-discovery.md` (stub)
- [X] `docs/src/lookup/overview.md` (stub)
- [X] `docs/src/lookup/output-formats.md` (stub)
- [X] `docs/src/announce/overview.md` (stub)
- [X] `docs/src/announce/architecture.md` (stub)
- [X] `docs/src/announce/echo-protocol.md` (stub)
- [X] `docs/src/ping/overview.md` (stub)
- [X] `docs/src/ping/architecture.md` (stub)
- [X] `docs/src/ping/output-formats.md` (stub)
- [X] `docs/src/cp/overview.md` (stub)
- [X] `docs/src/cp/protocol.md` (stub)
- [X] `docs/src/cp/reliability.md` (stub)
- [X] `docs/src/deaddrop/overview.md` (stub)
- [X] `docs/src/deaddrop/architecture.md` (stub)
- [X] `docs/src/deaddrop/format.md` (stub)
- [X] `docs/src/deaddrop/operations.md` (stub)
- [X] `docs/src/deaddrop/future-direction.md` (stub)
- [X] `docs/src/appendices/security-model.md` (stub)
- [X] `docs/src/appendices/limits-and-performance.md` (stub)
- [X] `mdbook build docs/` exits 0

---

## Phase 2 — GitHub Actions Workflow

- [X] `.github/workflows/docs-site.yml`

---

## Phase 3 — Shared Concepts

- [X] `docs/src/concepts/dht-and-routing.md` (complete)
- [X] `docs/src/concepts/keys-and-identity.md` (complete)
- [X] `docs/src/concepts/topics-and-discovery.md` (complete)
- [X] Cross-chapter consistency review

---

## Phase 4 — lookup Chapter

- [X] `docs/src/lookup/overview.md` (complete)
- [X] `docs/src/lookup/output-formats.md` (complete)
- [X] Verified against `peeroxide-cli/src/cmd/lookup.rs`

---

## Phase 5 — announce Chapter

- [X] `docs/src/announce/overview.md` (complete)
- [X] `docs/src/announce/architecture.md` (complete — Mermaid diagram included)
- [X] `docs/src/announce/echo-protocol.md` (complete — canonical echo protocol definition)
- [X] Verified against `peeroxide-cli/src/cmd/announce.rs`

---

## Phase 6 — ping Chapter

- [X] `docs/src/ping/overview.md` (complete)
- [X] `docs/src/ping/architecture.md` (complete — Mermaid diagram included, cross-refs echo-protocol)
- [X] `docs/src/ping/output-formats.md` (complete)
- [X] Verified against `peeroxide-cli/src/cmd/ping.rs`

---

## Phase 7 — cp Chapter

- [X] `docs/src/cp/overview.md` (complete)
- [X] `docs/src/cp/protocol.md` (complete — Mermaid diagram included)
- [X] `docs/src/cp/reliability.md` (complete)
- [X] Verified against `peeroxide-cli/src/cmd/cp.rs`

---

## Phase 8 — deaddrop Chapter

- [X] `docs/src/deaddrop/overview.md` (complete)
- [X] `docs/src/deaddrop/architecture.md` (complete — Mermaid diagram included)
- [X] `docs/src/deaddrop/format.md` (complete — binary layout tables)
- [X] `docs/src/deaddrop/operations.md` (complete)
- [X] `docs/src/deaddrop/future-direction.md` (complete)
- [X] Verified against `peeroxide-cli/src/cmd/deaddrop.rs`

---

## Phase 9 — Appendices

- [X] `docs/src/appendices/security-model.md` (complete)
- [X] `docs/src/appendices/limits-and-performance.md` (complete)

---

## Phase 10 — Polish & Cross-Cutting

- [X] All mandatory Mermaid diagrams present and render correctly
- [X] Cross-references using mdBook `[text](../concepts/chapter.md)` links
- [X] `docs/src/introduction.md` updated with tool overview
- [X] `mdbook build docs/` exits 0 with no warnings or broken links

---

## Phase 11 — AGENTS.md Files

- [X] `docs/AGENTS.md` created
- [X] `peeroxide-cli/AGENTS.md` created
- [X] Root `AGENTS.md` created or updated

---

## Completion Criteria

- [X] `DOCS_PLAN.md` exists and every checkbox is `[X]`
- [X] `mdbook build docs/` exits 0 with no warnings or broken links
- [X] Every chapter has been verified against its source files
- [X] All Mermaid sequence diagrams render correctly in the built HTML
- [X] Echo protocol is defined exactly once (`announce/echo-protocol.md`); ping chapter cross-references it
- [X] `AGENTS.md` files exist at `docs/`, `peeroxide-cli/`, and workspace root
- [X] `ISSUES.md` exists and has been reviewed
- [X] All commits are clean — one per completed work item, no WIP commits
- [X] Nothing has been pushed
