# Roughup — Local LLM CLI Roadmap

## Core Mission

Privacy-first Rust CLI for LLM workflows: extract minimal code context, validate EBNF edits, apply safely with atomic backups.

## Architecture Invariants

- Local-only: no network, deterministic outputs, preview-first
- Safety: repo boundaries enforced, atomic writes, sessionized backups
- Performance: <2s context assembly, <300ms rollback, <150ms backup listing

## Production Status

**Phases 1-3 Complete + Hardened Systems + Enhanced Priority System**

- Edit engine: EBNF parser, robust INSERT/REPLACE parsing, atomic writes
- Hybrid apply: internal engine + git fallback, typed exit codes
- Backup system: centralized `.rup/backups/`, BLAKE3 checksums, CLI management
- Smart context: enhanced Priority system with SymbolRanker, token budgeting, NaN-safe deterministic ordering
- Foundation: mentor's critical budget fixes applied, comprehensive test coverage
- Latest: parse_content_block robustness for fenced/unfenced content, optional blank lines, CRLF normalization

## Implementation Priority Queue

**Phase 3: Smart Context Assembly** [COMPLETED + ENHANCED]

- Enhanced Priority system: level/relevance/proximity fields with NaN safety
- SymbolRanker: semantic matching, anchor-aware scoring, development phase awareness
- Budget system: fixed shrink logic, 2-stage hard item expansion, deterministic total_cmp ordering
- Robust EBNF parser: fenced/unfenced content, optional blank lines, improved INSERT handling
- CLI: `rup context --budget --template [refactor|bugfix|feature] --semantic`
- Performance: <2s typical, deterministic across runs, production-ready

**Phase 3.5: Conflict Resolution** [NEXT - Critical Priority]

- Parse conflict markers (ours/theirs/base), categorize, confidence scoring
- `rup resolve --strategy --auto-resolve-safe` with TUI diff
- Target: <100ms parse, >95% auto-accuracy, no false positives

**Phase 4: Discovery & Rendering** [Medium Priority]

- Renderers: markdown-chat, json-tool, patch
- Commands: outline, find, find-function
- Target: deterministic, <2s outline for 1k files

**Phase 4.5: Feedback Loop** [Value Multiplier]

- Local SQLite: pattern tracking, confidence scores, insights
- Commands: stats, insights with <10ms overhead
- Privacy: no code content stored

**Phase 5: Analysis Tools**

- Commands: usage, callers, deps, impact
- Target: <1s common, <4s large repos

**Phase 6-7: Persistence & Integration** [Future]

- Session save/load, CI templates, editor integration

## CLI Interface

**Commands**: apply, preview, check-syntax, backup {list|show|restore|cleanup}, extract, symbols, chunk, tree, context, outline, find, find-function, usage, callers, deps, impact

**Global Flags**: --no-color, --quiet, --dry-run, --json, --context-lines=N

**Exit Codes**: 0=success, 2=conflicts, 3=invalid, 4=repo, 5=internal

## Quality Gates

**Performance SLA**

- Context assembly: <2s typical, <5s heavy
- Backup operations: <150ms list, <300ms rollback
- Discovery: <2s outline (1k files), <1s analysis queries

**Testing Strategy**

- Must: determinism across OSes, EBNF fuzzing, boundary enforcement, backup lifecycle
- Should: performance benchmarks, large-repo validation, crash recovery

## Immediate Actions

1. **Start Phase 3.5**: Implement conflict resolution engine and TUI
2. **Conflict CLI**: Add `rup resolve` with strategy selection and auto-resolve
3. **Validation**: Auto-resolution accuracy tests, determinism validation
4. **Integration**: Wire conflict detection with existing apply operations

## Architecture Reference

**Completed Hardening (10 fixes)**

- Cross-platform sync, atomic manifests, stale-lock cleanup
- Path validation, symlink handling, binary-diff fallback
- Centralized backup layout with DONE markers and BLAKE3 checksums
