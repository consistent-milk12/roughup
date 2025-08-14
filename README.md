Below is an ultra-compact, token-efficient rewrite of `TODO.md` that preserves full project context, moves Phase 3 to the front, and defers former Phase 2.5 to Phase 3.5.

---

# Roughup — Roadmap (Local-Only, Phases 1–7)

## 0) Purpose

Local-only, model-agnostic Rust CLI for LLM workflows: extract code context, validate EBNF edits, apply safely with deterministic backups/rollback.

## 1) Invariants

- No network; deterministic outputs; preview-first; `--dry-run` ubiquitous.
- Strict repo-relative paths; refuse escapes/submodules unless flagged.
- CRLF/LF preserved; atomic writes; sessionized backups with DONE.

## 2) Status Snapshot

- Phase 1: Edit engine + EBNF + atomic writes — complete.
- Phase 2: Hybrid apply (internal+Git), typed exit codes, safety — complete.
- Backups: Centralized system + CLI (list/show/restore/cleanup) — complete.
- Foundation hardened: all 10 critical fixes applied (cross-platform fsync, ID gen, finalize timing, stale locks, atomic manifest, path validation, cleanup dedup, symlink UX, non-regular file guard, binary-diff fallback).
- Tests: all unit + integration passing; clean build; no regressions.

## 3) Prioritized Roadmap (Next)

3. **Phase 3 — Smart Context Assembly** \[Top Priority]
   Goal: minimal, high-signal, budgeted context packs.
   Deliverables:

   - `symbol_index`: load `symbols.jsonl` (Rust/Py), exact/fuzzy lookup, spans.
   - Relevance ranking: semantic (local embeddings, ONNX) → scope → proximity → history → lexical.
   - `budgeter`: token/char estimate (tiktoken-rs), deterministic ordering, overflow strategies; optional test-impact heuristic.
   - CLI `rup context`: `--budget`, `--include-tests|--include-deps`, `--template [refactor|bugfix|feature]`, `--semantic`, paste-ready output and `--json`.
     Targets/AC:
   - 1k-file repo: <2 s typical, <5 s heavy; estimate ±10%.
   - Deterministic ordering; includes defs+deps and nearest tests (when requested) within budget.

3.5. **Phase 3.5 — Conflict Resolution Assistant** \[After Phase 3]
Goal: reduce/manual conflicts (exit code 2).
Deliverables:

- Parser for conflict blocks (ours/theirs/base); categorizations; safe auto-fixes (imports/whitespace); confidence scoring.
- `rup resolve <file>`; `--strategy [ours|theirs|manual|auto]`; TUI diff; `--auto-resolve-safe`.
  Targets/AC: <100 ms parse; >95% accuracy on formatting/imports; deterministic; no unsafe auto-resolves.

4. **Phase 4 — Renderers & Local Discovery**

   - Renderers: `markdown-chat`, `json-tool`, `patch` (configurable context).
   - `rup outline`, `rup find`, `rup find-function`.
     AC: deterministic output; 1k files outline <2 s.

4.5. **Phase 4.5 — Feedback Loop & Learning**

- Local SQLite: outcomes, patterns; confidence scores, risk hints; `rup stats|insights`.
  AC: <10 ms overhead; actionable insights; no code content stored.

5. **Phase 5 — Analysis & Dependencies**

   - `usage`, `callers`, `deps`, `impact`.
     AC: <1 s common; <4 s 1k-file; stable results.

6. **Phase 6 — Session/Context Persistence**

   - Save/load reproducible contexts; manifests with policy/budget; optional local encryption.
     AC: byte-identical reload for unchanged repo; precise delta report otherwise.

7. **Phase 7 — Ecosystem Integration**

   - CI templates; pre-commit; container; editor/LSP; export/import with other tools.
     AC: zero-config defaults; stable exit codes; fast startup.

## 4) CLI Surface (current+planned)

`apply`, `preview`, `check-syntax`, `backup {list|show|restore|cleanup}`, `extract`, `symbols`, `chunk`, `tree`, `context`, `outline`, `find`, `find-function`, `usage`, `callers`, `deps`, `impact`.
Global: `--no-color`, `--quiet`, `--dry-run`, `--json`, `--context-lines=N`.
Exit codes: `0` ok, `2` conflicts, `3` invalid, `4` repo, `5` internal.

## 5) Performance Targets

- Context: <2 s typical; <5 s heavy.
- Outline: <2 s (1k files).
- Apply rollback: <300 ms.
- Listing 1k+ backup sessions: <150 ms.

## 6) Testing (must/should)

- Must: determinism (all OSes), EBNF fuzz/property, repo-boundary & submodule refusal, backup life-cycle, conflict exit-code semantics.
- Should: tokenizer/budget microbench, context macrobench, large-repo smoke, stale-lock + crash-recovery paths.

## 7) Next Actions (Do Now)

1. Implement Phase 3 core: `symbol_index`, `budgeter`, deterministic ranking, `rup context` UX (`--budget`, templates, `--json`).
2. Add Phase 3 tests/benches (estimate accuracy ±10%, determinism, perf caps).
3. Plan Phase 3.5 API/TUI surfaces; define safe auto-rules and confidence thresholds.
4. Update README/`--help` for `context`; add minimal examples.

## 8) Appendix: Completed Hardening (reference)

- Cross-platform `sync_dir` (Windows no-op); atomic manifest writes; finalize flag timing; stale-lock GC (>60 s).
- Path validation (reject `Prefix`/`RootDir`); cleanup dedup via `HashSet`; improved symlink/broken-link UX; non-regular file guard; binary-diff fallback.
- Backups: centralized layout, DONE markers, BLAKE3, index.jsonl, list/show/restore/cleanup with JSON output.

---
