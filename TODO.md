Here’s a clean, professional rewrite of `TODO.md` reflecting work completed through the callgraph test diff.

---

# Roughup — Local LLM CLI Roadmap

## Core mission

Privacy-first Rust CLI for LLM workflows: extract minimal code context, validate EBNF edits, and apply safely with atomic backups.

## Architecture invariants

- Local-only execution; deterministic outputs; preview-first UX
- Safety: repository boundary enforcement, atomic writes, sessionized backups
- Performance SLOs: context assembly < 2s; rollback < 300ms; backup listing < 150ms

## Production status

Phases 1–3.5 are complete with hardened systems and conflict resolution:

- Edit engine: EBNF parser, robust INSERT/REPLACE handling, atomic writes
- Hybrid apply: internal engine with git fallback; typed exit codes
- Backups: centralized `.rup/backups/`, BLAKE3 checksums, CLI lifecycle management
- Smart context: enhanced priority model with SymbolRanker, token budgeting, deterministic ordering
- Conflict resolution: byte-level Git marker parsing, deterministic SmartMerge, EOL preservation
- Test coverage in place; production-grade conflict detection (< 100ms parsing, ≥ 95% auto-resolution)

---

## Implementation priority queue

### Phase 3 — Smart context assembly (complete, enhanced)

- Priority model: level/relevance/proximity with NaN-safe math
- SymbolRanker: semantic matching; anchor-aware scoring
- Budgeting: corrected shrink logic, two-stage hard-item expansion, deterministic `total_cmp`
- EBNF parser: fenced/unfenced content, optional blank lines, improved INSERT handling
- CLI: `rup context --budget --template [refactor|bugfix|feature] --semantic`
- Performance: < 2s typical; deterministic across runs

### Phase 3.5 — Conflict resolution (complete; production-ready with apply integration)

- Byte-accurate Git marker parsing (2-way/3-way), non-UTF-8 safe
- Deterministic confidence scoring (whitespace/addition/superset/disjoint)
- SmartMerge ordering with EOL preservation
- CLI: `rup resolve --strategy …` with JSON and human output
- BackupManager integration; stable JSON schema
- Performance: parsing < 2ms observed on typical fixtures; > 95% auto-accuracy

### Phase 4 — Precision context (current program; 8-week track)

**Target metrics**

- Context Efficiency Factor (CEF) ≥ 6.0 across varied repositories
- Turns to Valid Edit (TVE) median ≤ 1.5
- First-try pass uplift: +25–30% vs baseline
- Duplicate Collapse Rate (DCR) ≥ 0.70 on large repositories
- Probe-First Ratio (PFR) ≥ 0.90 sessions starting with ranges-only
- Determinism: byte-identical JSON across OS/arch CI matrix

#### Week 1 — Foundation and probe-first defaults (planning artifacts retained)

- Dedupe v1 design (Jaccard 4-gram + rolling hash), shrink recipe, determinism harness
- Probe-first manifest and onboarding defaults
- Determinism policies (global stable sort; canonical float/paths/EOL)

#### Week 2 — Selection intelligence (complete)

- Dedupe v2: AST-aware shingles with SimHash fallback; interface spans are non-dedupe unless exact match
- N-gram mode selection (word vs char) with tolerance to prevent whitespace-induced flips
- Token-accurate budgeting (`take_prefix` guarantees no overflow)
- Buckets with hard caps: `--buckets code=…,interfaces=…,tests=…` with refusal logs
- Novelty floor: `--novelty-min` via TF-IDF rarity; robust tokenization and template file support
- Results: DCR ≥ 0.70 validated; CEF uplift without TVE regression

#### Week 3 — Relevance via fail-signal seeding (complete)

- Log parsing framework; rustc/cargo parser in production; proximity-weighted boosting
- CLI: `--fail-signal <PATH>` with auto-detection and graceful degradation
- Results: failing line appears in top-3 spans ≥ 90% on fixtures; TVE improvement (−0.2)

#### Week 4 — Type and callgraph narrowing (in progress; initial implementation landed)

- CLI:

  - `--trait-resolve Type::method` (parses `Type::method`, derives queries for `trait Type`, `impl Type for …`, and `Type::method`)
  - `--callgraph "anchor=path:line depth=N"` (lightweight, static edges; depth clamped to 1–3)

- Core:

  - Deterministic BFS over same-file neighborhoods; lexical frontier; bounded window scans
  - Helpers promoted to module scope for testing (`parse_trait_resolve`, `extract_function_name_at`, `collect_callgraph_names`)
  - Query augmentation integrated prior to lookup; deduplication preserves order

- Tests (passing):

  - `tests/context_callgraph.rs::trait_resolve_finds_impl_block`
  - `tests/context_callgraph.rs::callgraph_finds_callers_at_depth_2` (dynamic anchor line discovery)

- Next for Week 4:

  - Extend callgraph beyond same-file when cheap indices exist (guarded by SLA)
  - Score-level integration tuning (bounded weight for call distance)
  - Fixture-level measurement: precision\@k ≥ +20% and CEF +0.5 without TVE regression

#### Week 5 — Explainability and header improvements (planned)

- `--explain-scores` with stable schema (proximity, symbol_match, call_distance, dedupe penalties, final_score)
- `--guard-hash` (file+range+EOL) with selective refresh prompts
- Template header guidance informed by fail-signal data; measurable first-try uplift
- Determinism gates across OS/arch

#### Week 6 — Hardening and performance (planned)

- Shrink tuning, edge-case dedupe, rolling-hash prefilter; bounded rayon
- Feature flags for expensive passes; serialization < 10ms

#### Week 7 — Large-repository validation (planned)

- Sweeps on large codebases, crash-recovery drills, documentation finalization
- Developer ergonomics: `--dry-run --explain-scores`, `--why <file:line>`
- Scoreboard medians at A+ targets

#### Week 8 — Release and CI integration (planned)

- v4.3 release; scoreboard integrated into CI
- CI failure on > 5% regression in any primary metric
- Final gate: CEF ≥ 6.0; DCR ≥ 0.70; PFR ≥ 0.90; TVE ≤ 1.5

Deferred until Phase 4 completion: TUI for conflict resolution; Phase 5 analysis tools (usage/callers/deps/impact); Phases 6–7 persistence and integrations.

---

## CLI interface

Commands: `apply`, `preview`, `check-syntax`, `backup {list|show|restore|cleanup}`, `extract`, `symbols`, `chunk`, `tree`, `context`, `resolve`

Phase 4 feature flags and options:

- `context --tier A|B|C` (tier presets: A≈1200, B≈3000, C≈6000)
- `context --probe` (ranges-only manifests)
- `context --manifest-out|in <path>`; `context --replay <manifest>`
- `context --buckets code=N,interfaces=N,tests=N`
- `context --dedupe jaccard>=N`
- `context --novelty-min N`
- `context --fail-signal <path>`
- `context --trait-resolve Type::method`
- `context --callgraph "anchor=path:line depth=N"`
- Planned: `context --explain-scores`, `context --guard-hash`, `context --scoreboard <fixture_plan.json>`, `context --why <file:line>`

Global flags: `--no-color`, `--quiet`, `--dry-run`, `--json`, `--context-lines=N`, `--ranges-only`

Exit codes: `0=success`, `2=conflicts`, `3=invalid`, `4=repo`, `5=internal`

---

## Quality gates

Performance SLOs

- Context assembly < 2s typical, < 5s heavy
- Backup listing < 150ms; rollback < 300ms
- Conflict parsing < 100ms (100KB), with < 2ms observed on fixtures

Phase 4 precision targets

- Tier A ≤ 1200 tokens; Tier B ≤ 3000; Tier C ≤ 6000
- CEF ≥ 6.0; TVE ≤ 1.5; first-try uplift +25–30%
- DCR ≥ 0.70; PFR ≥ 0.90
- Manifest serialization < 10ms; byte-identical JSON across OS/arch

---

## Testing strategy

Foundational

- Determinism across OSes; EBNF fuzzing; repository boundary enforcement; backup lifecycle

Conflict resolution

- Git marker edge cases; CRLF preservation; confidence scoring accuracy

Phase 4 precision suite

- `tests/context_dedupe.rs`: DCR on boilerplate; interface span preservation; rationale logs
- `tests/context_buckets.rs`: cap enforcement and shrink ordering; budget compliance within ±5%
- `tests/context_novelty.rs`: TF-IDF novelty filtering and thresholds
- `tests/context_fail_signal.rs`: failing-line inclusion ≥ 90%; anchor-distance effect
- `tests/context_callgraph.rs`: trait/impl inclusion and caller/callee discovery at bounded depth (now passing)
- Planned: `tests/context_explain.rs` (schema/value determinism), `tests/context_guardhash.rs`, `tests/context_probe.rs`, `tests/context_matrix.rs`, `tests/context_tier.rs`, `tests/context_scoreboard.rs`

Performance gates

- Context < 2s typical, < 5s heavy; serialization < 10ms

Fixtures

- Small Rust library; medium Rust+Python monorepo; large boilerplate repository; failing-tests repository; trait-heavy repository

CI gates

- CEF ≥ 6.0; DCR ≥ 0.70; PFR ≥ 0.90; TVE ≤ 1.5; CI fails on > 5% regression

---

## Immediate actions (reflecting current state)

- Finalize Week 4 scoring integration for `--callgraph` (bounded weight; no SLA regressions)
- Add cross-file callgraph edges when a cheap index is available; keep depth and cost caps
- Introduce scoreboard harness for precision\@k and CEF deltas on typed fixtures
- Expand tests for determinism around augmented queries and BFS frontier ordering
- Prepare documentation for new CLI flags (`--trait-resolve`, `--callgraph`) with examples and guidance

---
