# Roughup Development Roadmap

_Last updated: 2025-08-14_

## 0. Purpose and Scope

Roughup is a high-performance Rust CLI for **local-only**,
model-agnostic LLM workflows. It extracts precise code
context, validates human-readable edit specs, and applies
changes safely using a hybrid internal+Git architecture.

This roadmap is the single source of truth for near-term plans
(Phases 1–6), acceptance criteria, safety gates, and test
strategy. There are **no cloud/API integrations** in scope.

## 1. Vision

Deliver a **fast, privacy-preserving** CLI that shortens the
loop between “assemble context → review → apply edits”, while
keeping all processing on the user’s machine and maintaining
reproducibility, safety, and professional UX.

## 2. Non-Goals

- No network calls, provider SDKs, or remote inference.
- No browser extensions or hosted services.
- No telemetry leaving the user’s machine.

## 3. Design Principles

1. **Privacy first**: All operations are local; sending data
   elsewhere is out of scope.
2. **Determinism**: Identical inputs yield identical diffs,
   outputs, and exit codes.
3. **Safety over convenience**: Preview by default; explicit
   flags for writes; fail-closed on ambiguity.
4. **Performance**: Favor zero-copy reads, memory mapping, and
   parallel traversal; keep latencies sub-second for common
   tasks.
5. **Scriptable UX**: Human-friendly output by default;
   machine-readable modes available everywhere.

---

## 4. Current Status

- **Phase 1**: Production-ready edit system — complete.
- **Phase 2**: Git integration and advanced edit features —
  complete (with hardening).
- **Phases 3–6**: Planned/next; expanded below.

---

## 5. Roadmap at a Glance (Local-Only)

| Phase | Theme                            | Status   |
| ----: | -------------------------------- | -------- |
|     1 | Edit engine, EBNF, atomic writes | Complete |
|     2 | Git 3-way, exit codes, safety    | Complete |
|     3 | Smart context assembly           | Next     |
|     4 | Renderers & local discovery      | Next     |
|     5 | Analysis & dependency tools      | Next     |
|     6 | Session persistence (local)      | Next     |

---

## 6. Detailed Phases and Acceptance Criteria

### Phase 1 — Production-Ready Edit System (complete)

Deliverables (complete)

- EBNF parser (Replace/Insert/Delete) with strict validation.
- Deterministic GUARD-CID (xxh64), normalized comparisons.
- Overlap detection; stable operation ordering.
- Memory-mapped reads; cross-platform CRLF/LF preservation.
- Atomic writes with backup/rollback.

Enhancements (sustainment backlog, local only)

- [ ] JSONL machine-readable error stream (`--json` flag).
- [ ] Two-phase multi-file apply: all-or-nothing with automatic
      rollback on any per-file failure.
- [ ] Optional “strict mode”: warnings promoted to failures.
- [ ] Binary safety: detect and refuse edits to non-text files
      unless `--allow-binary` is provided.

Acceptance criteria

- Parser round-trips 10k-line files under 50 ms on laptop
  hardware; operations applied correctly across CRLF/LF.
- All writes are preceded by a preview unless `--apply` is set.
- On failure, original files are intact and backups present.

---

### Phase 2 — Git Integration & Advanced Edit Features (complete)

Deliverables (complete)

- `git apply --check` and `--3way` support; `--index` mode.
- Worktree detection; repository boundary enforcement; no path
  escape across repo root unless explicitly allowed.
- Typed error taxonomy with stable exit codes:
  `0=success, 2=conflicts, 3=invalid, 4=repo, 5=internal`.
- Hybrid engine: internal first; escalate to Git when needed.
- Windows/Unix parity for atomic writes and line endings.

Advanced edit operations (local backlog)

- [ ] **CREATE**: new file creation directive with unified-diff
      headers (`new file mode 100644`).
- [ ] **DELETE-FILE**: safe deletion via patch (`deleted file mode`).
- [ ] **RENAME-FILE**: rename/move with `rename from/to` headers.
- [ ] Multi-file apply transaction: stage all patches, verify,
      then commit atomically; rollback if any fails.
- [ ] Submodule safety: refuse edits inside submodules unless
      `--allow-submodules` is set.

Acceptance criteria

- Conflicts consistently return exit code 2 with actionable,
  machine-parseable summaries.
- Repo boundary violations consistently return code 4.
- CREATE/DELETE/RENAME produce valid patches accepted by
  `git apply --check` and `--3way`.

---

### Phase 3 — Smart Context Assembly (next)

Goal
Assemble **minimal, high-signal** context packs for local
review or copy-paste into any chat, within a user budget.

Deliverables

- `src/core/symbol_index.rs`:

  - [ ] Load `symbols.jsonl` (Rust/Python) into an in-memory
        index; exact/fuzzy name lookup; file→symbols; span
        references.
  - [ ] Stable ranking: scope > proximity > historical touches
        (optional) > lexical fallback; deterministic tie-breaks.

- `src/core/budgeter.rs`:

  - [ ] Token/char estimation with tiktoken-rs; xxh64 content
        IDs; deterministic selection and ordering.
  - [ ] Budget overflow strategies: shrink by symbol boundary;
        then by docstring/comments; then by low-rank items.

- `rup context`:

  - [ ] Inputs: symbol names or free text; `--budget`,
        `--by-symbols`, `--include-tests`, `--include-deps`.
  - [ ] Output: single paste-ready block with per-file fences
        and CID headers; optional `--json` structured output.
  - [ ] Stability: identical inputs produce identical ordering.

Performance targets

- 1k-file repo: context build under 5 s; most requests < 2 s.
- Budgeter estimate within ±10% of actual tokenizer count.

Acceptance criteria

- For a targeted symbol, context includes its definition,
  immediate dependencies, and nearest tests when
  `--include-tests` is set, without exceeding budget.
- Outputs pass a reproducibility check (hash of rendered block).

---

### Phase 4 — Renderers & Local Discovery (next)

Goal
Provide chat-friendly and machine-friendly renderers plus local
discovery commands that help users triage and navigate the
codebase without leaving the terminal.

Deliverables

- `src/infra/renderer.rs`:

  - [ ] `markdown-chat` renderer (minimal headers, fenced blocks,
        CID lines; no line numbers).
  - [ ] `json-tool` renderer (stable schema for scripting).
  - [ ] `patch` renderer (unified diff view) with optional
        path roots and context width.

- `rup outline`:

  - [ ] Per-directory summaries with file counts, top symbols,
        and quick links (paths) suitable for copy-paste.

- Search helpers (local only):

  - [ ] `rup find <pattern>`: ranked text search across repo
        with context lines; ignore rules respected.
  - [ ] `rup find-function <name>`: AST-aware function lookup
        via symbol index.

Acceptance criteria

- Given the same pack, `markdown-chat` and `json-tool` contain
  the same semantics; only format differs.
- `outline` runs under 2 s on 1k files and is deterministic.

---

### Phase 5 — Analysis & Dependency Tools (next)

Goal
Offer fast, local program analysis primitives to reduce
guesswork prior to editing.

Deliverables

- Usage/callers:

  - [ ] `rup usage <symbol>`: list read/write sites and
        references with line spans.
  - [ ] `rup callers <function>`: call graph slice to depth N.

- Dependencies:

  - [ ] `rup deps <file|symbol>`: show incoming/outgoing
        dependencies (static; best-effort).

- Impact analysis:

  - [ ] `rup impact <patch|spec>`: estimate blast radius using
        symbol references and file touch frequency (local Git
        history optional).

Performance targets

- All queries return under 1 s for common cases; under 4 s for
  1k-file repos.

Acceptance criteria

- Results are stable and reproducible for a fixed repo state.
- Commands return non-zero exit codes when the query cannot be
  satisfied (e.g., symbol not found).

---

### Phase 6 — Session Management & Persistence (next)

Goal
Allow users to **save, reload, and reproduce** contexts and
apply sessions entirely offline.

Deliverables

- `src/infra/manifest.rs`:

  - [ ] Manifest schema (JSON) with: repo root, commit (if any),
        renderer preset, query terms, budget, pack hashes, and
        policy decisions.
  - [ ] Versioning and migrations; stores under
        `.roughup/contexts/`.

- Session commands:

  - [ ] `rup save-context [name]`: persist last context.
  - [ ] `rup load-context <name>`: rebuild context deterministically.
  - [ ] `rup recent-files`: list recently modified paths with
        timestamps; integrates with `apply` history.

- Optional local encryption:

  - [ ] `--encrypt` writes manifests using OS keyring for the
        key; restore with `--decrypt`. No network involved.

Acceptance criteria

- Reloading a saved context reproduces byte-identical output for
  the same repo state.
- Loading a context against a changed repo emits a precise diff
  of deltas and proposes regeneration.

---

## 7. CLI Surface (local-only, scriptable)

Primary commands (current and planned)

- `apply`, `preview`, `check-syntax`, `backup`
- `extract`, `symbols`, `chunk`, `tree`
- `context`, `outline`, `find`, `find-function`
- `usage`, `callers`, `deps`, `impact`
- Global flags: `--no-color`, `--quiet`, `--dry-run`,
  `--json`, `--context-lines=N`

Exit codes (stable)

- `0` success; `2` conflicts; `3` invalid spec; `4` repo issues;
  `5` internal error.

---

## 8. Safety, Privacy, and Policy (local)

- Path discipline: refuse `..` traversal and symlinks that
  escape the repo unless `--allow-outside-repo` is provided.
- Submodules: read-only by default; edits require explicit
  `--allow-submodules`.
- Binary files: skipped unless `--allow-binary` is set.
- Dry-run everywhere; preview is the default for edits.
- Machine output mode (`--json`) hides color/decoration and
  emits single-line JSON records for easy parsing.

---

## 9. Performance Targets

- Parse + preview a 1k-line spec in under 100 ms.
- Build a 1k-file outline in under 2 s on commodity laptops.
- Assemble a context pack for a symbol under 2 s; heavy packs
  under 5 s.
- Apply multi-file patches with two-phase commit; rollback in
  under 300 ms on error.

---

## 10. Testing Strategy

Unit tests (must)

- Parser fuzzing (cargo-fuzz) for EBNF directives and fences.
- Property-based tests (proptest) for overlap and ordering.
- CID stability tests across line ending variations.

Integration tests (must)

- EBNF → patch golden tests (snapshot).
- Git `--check`/`--3way` paths with induced conflicts.
- Boundary enforcement, submodule refusal, symlink traversal.

Performance tests (should)

- Microbenchmarks for tokenizer/xxh64; macro for `context`,
  `outline`, and multi-file `apply`.

Determinism tests (must)

- Same inputs produce identical outputs across runs and OSes.
- JSON modes validated against schemas.

---

## 11. Release Checklist (per tag)

- All unit/integration/determinism tests pass on Linux/macOS/Windows.
- Exit codes verified for the six canonical scenarios.
- `core.txt` regenerated and committed with provenance header
  (UTC time, commit).
- README updated with current CLI synopsis and examples.

---

## 12. Now / Next

Now

- Harden Phase 2 backlog: CREATE/DELETE/RENAME; two-phase
  multi-file apply; submodule safety switch; JSON error stream.

Next

- Phase 3 minimal slice: `SymbolIndex`, `Budgeter`, and `rup context`
  with `markdown-chat` rendering path.
- Phase 4 minimal slice: `renderer` module and `rup outline`.
- Phase 6: manifest save/load for reproducible contexts.

---

## 13. Appendices

A. JSON record sketch (machine mode)

```json
{
  "ts": "2025-08-14T00:00:00Z",
  "level": "error",
  "code": "CONFLICT",
  "file": "src/lib.rs",
  "line": 120,
  "detail": "guard_cid_mismatch"
}
```

B. CID header format (in fenced blocks)

```
# RUP-CID: xxh64=af4e…  file=src/lib.rs  lines=120-168  generated=UTC
```

C. Manifest fields (subset)

```json
{
  "repo_root": "/home/user/proj",
  "commit": "abc123",
  "renderer": "markdown-chat",
  "query": "fn parse_config",
  "budget": 8000,
  "pack": [{ "file": "src/lib.rs", "cid": "af4e…", "span": [120, 168] }],
  "policy": { "allow_binary": false, "allow_outside_repo": false }
}
```

---

**End of local-only roadmap (Phases 1–6).**
