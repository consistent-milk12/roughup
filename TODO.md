# Roughup Development Roadmap

_Last updated: 2025-08-14_

## 0. Purpose and Scope

Roughup is a high-performance Rust CLI for **local-only**,
model-agnostic LLM workflows. It extracts precise code
context, validates human-readable edit specs, and applies
changes safely using a hybrid internal+Git architecture.

This roadmap is the single source of truth for near-term plans
(Phases 1–7), acceptance criteria, safety gates, and test
strategy. There are **no cloud/API integrations** in scope.

### Value Proposition

Roughup addresses the "last mile" problem in LLM-assisted development:
the friction between extracting relevant code context, reviewing AI-suggested
changes, and safely applying them. Unlike IDE-integrated tools (Cursor, Continue)
or interactive chat tools (aider), roughup provides:

- **Privacy-first**: All operations local, no cloud connectivity required
- **Deterministic & Scriptable**: Identical inputs yield identical outputs
- **Production-grade Safety**: Preview-first UX with atomic rollback
- **LLM-agnostic**: Works with any model/interface, not vendor-locked
- **Hybrid Architecture**: Fast internal engine with git fallback for robustness

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

| Phase | Theme                             | Status   | Priority |
| ----: | --------------------------------- | -------- | -------- |
|     1 | Edit engine, EBNF, atomic writes  | Complete | ✓        |
|     2 | Git 3-way, exit codes, safety     | Complete | ✓        |
|   2.5 | **Conflict Resolution Assistant** | Next     | Critical |
|     3 | **Smart context assembly**        | Next     | High     |
|   4.5 | **Feedback Loop & Learning**      | Next     | High     |
|     4 | Renderers & local discovery       | Next     | Medium   |
|     5 | Analysis & dependency tools       | Next     | Medium   |
|     6 | Session persistence (local)       | Next     | Low      |
|     7 | **Ecosystem Integration**         | Future   | Medium   |

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

### Phase 2.5 — Conflict Resolution Assistant (next - critical)

Goal
Dramatically reduce friction when conflicts occur (exit code 2) by providing
intelligent resolution strategies and guidance.

**Rationale**: Conflicts are the primary usability blocker. Current tooling
leaves users to manually resolve with limited guidance.

Deliverables

- `src/core/conflict_resolver.rs`:

  - [ ] Parse conflict markers and extract sections (ours/theirs/base)
  - [ ] Conflict categorization: imports, formatting, logic, overlapping edits
  - [ ] Automatic resolution for common patterns (import reordering, whitespace)
  - [ ] Confidence scoring for auto-resolution candidates

- Enhanced CLI conflict handling:

  - [ ] `rup resolve <file>`: Interactive conflict resolution mode
  - [ ] `--strategy [ours|theirs|manual|auto]`: Batch resolution strategies
  - [ ] Side-by-side conflict visualization in terminal with syntax highlighting
  - [ ] `--auto-resolve-safe`: Apply high-confidence resolutions only

- Integration improvements:

  - [ ] Better error messages with resolution suggestions
  - [ ] Conflict statistics and patterns tracking
  - [ ] Export conflict resolution rules for future automation

Performance targets

- Conflict detection and parsing under 100ms for typical files
- Interactive resolution mode responsive (<50ms per keystroke)
- Auto-resolution accuracy >95% for formatting/import conflicts

Acceptance criteria

- Auto-resolution reduces manual conflict rate by >60%
- Interactive mode allows navigation and resolution without external tools
- Resolution strategies are deterministic and auditable
- No false positive auto-resolutions (safety over convenience)

---

### Phase 3 — Smart Context Assembly (enhanced)

Goal
Assemble **minimal, high-signal** context packs for local
review or copy-paste into any chat, within a user budget.

Deliverables

- `src/core/symbol_index.rs`:

  - [ ] Load `symbols.jsonl` (Rust/Python) into an in-memory
        index; exact/fuzzy name lookup; file→symbols; span
        references.
  - [ ] **Semantic relevance scoring**: Lightweight local embeddings (ONNX)
        for better symbol relationship detection
  - [ ] **Change coupling analysis**: Files that historically change together
        (from git history when available)
  - [ ] Stable ranking: semantic similarity > scope > proximity > historical
        touches > lexical fallback; deterministic tie-breaks.

- `src/core/budgeter.rs`:

  - [ ] Token/char estimation with tiktoken-rs; xxh64 content
        IDs; deterministic selection and ordering.
  - [ ] **Test impact prediction**: Auto-include tests likely affected by changes
  - [ ] Budget overflow strategies: shrink by symbol boundary;
        then by docstring/comments; then by low-rank items.
  - [ ] **Context templates**: Predefined patterns for common tasks
        (refactoring, bug fix, feature add)

- `rup context` (enhanced):

  - [ ] Inputs: symbol names or free text; `--budget`,
        `--by-symbols`, `--include-tests`, `--include-deps`.
  - [ ] **Template-based context**: `--template [refactor|bugfix|feature]`
  - [ ] **Semantic search**: `--semantic "error handling patterns"`
  - [ ] Output: single paste-ready block with per-file fences
        and CID headers; optional `--json` structured output.
  - [ ] **LLM prompt templates**: Export with optimized prompts for different models
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

### Phase 4.5 — Feedback Loop & Learning (new - high priority)

Goal
Learn from successful/failed edit applications to improve future operations
and provide confidence indicators for new edits.

**Rationale**: Without learning from outcomes, the tool cannot improve over time
or provide users with confidence indicators for risky operations.

Deliverables

- `src/core/feedback_tracker.rs`:

  - [ ] Local SQLite database for edit success/failure tracking
  - [ ] Pattern extraction from successful edit sequences
  - [ ] Failure mode categorization and common error patterns
  - [ ] Privacy-preserving analytics (no code content stored)

- Enhanced apply operations:

  - [ ] Confidence scoring for new edits based on historical patterns
  - [ ] Pre-application risk assessment with warnings
  - [ ] Success probability estimation for edit types
  - [ ] Automatic backup recommendations for high-risk edits

- Analytics and insights:

  - [ ] `rup stats`: Success rates, common failure modes, performance metrics
  - [ ] `rup insights`: Suggestions for improving edit success rates
  - [ ] Export learnings as validation rules for EBNF generation
  - [ ] Pattern-based suggestions for context assembly

Performance targets

- Feedback tracking adds <10ms overhead per operation
- Pattern analysis completes in <2s for 1000 historical operations
- Confidence scoring available in <100ms for new edits

Acceptance criteria

- Confidence scores correlate with actual success rates (>80% accuracy)
- Pattern recognition identifies common failure modes automatically
- Analytics provide actionable insights for workflow improvement
- No sensitive code content persisted in feedback database

---

### Phase 4 — Renderers & Local Discovery

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

### Phase 6 — Session Management & Persistence

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

### Phase 7 — Ecosystem Integration (new)

Goal
Seamless integration with existing developer workflows and popular
tools while maintaining local-only operations.

**Rationale**: Adoption requires minimal friction integration with
existing toolchains and workflows.

Deliverables

- CI/CD Integration:

  - [ ] GitHub Actions workflow templates for automated edit application
  - [ ] Pre-commit hooks for edit validation and safety checks
  - [ ] Docker container for consistent cross-platform environments
  - [ ] Exit code standardization for pipeline integration

- IDE Integration (remote-friendly):

  - [ ] VS Code extension for remote SSH development scenarios
  - [ ] Language server protocol support for symbol navigation
  - [ ] Editor-agnostic configuration via LSP

- Tool Interoperability:

  - [ ] Export formats compatible with aider, continue, cursor
  - [ ] Import/export for popular LLM chat tools
  - [ ] Standard schema for edit specifications across tools
  - [ ] Plugin architecture for custom renderers/processors

- Developer Experience:

  - [ ] Shell integration (bash/zsh/fish completions with context)
  - [ ] Man page generation with examples
  - [ ] Interactive tutorial mode (`rup tutorial`)
  - [ ] Configuration migration tools for major version upgrades

Performance targets

- GitHub Actions integration adds <30s to typical CI runs
- VS Code extension provides symbol navigation in <500ms
- Shell completions load in <100ms

Acceptance criteria

- Zero-configuration CI/CD integration for common scenarios
- VS Code extension works seamlessly over SSH connections
- Tool interoperability maintains edit fidelity across formats
- Shell integration feels native and responsive

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

## 7. Success Factors & Strategic Considerations

### Critical Success Factors

1. **Conflict Resolution Quality**: Smooth conflict resolution is where users
   spend most frustration. Auto-resolution accuracy and interactive UX quality
   directly impact adoption.

2. **Context Intelligence**: Context quality directly impacts LLM output quality.
   Semantic understanding and relevance scoring are crucial differentiators.

3. **Integration Friction**: The tool must fit naturally into existing workflows.
   High setup costs or workflow changes reduce adoption.

4. **Safety & Trust**: Production usage requires absolute confidence in rollback
   mechanisms and preview accuracy.

### Competitive Positioning

**Unique Differentiators to Maintain:**

- Local-only operations (privacy advantage)
- Deterministic, reproducible outputs (CI/CD advantage)
- EBNF format (more LLM-friendly than unified diff)
- Hybrid engine architecture (best of both worlds)
- Token-aware context optimization

**Areas to Watch:**

- IDE-integrated tools improving CLI/scripting support
- LLM providers building native edit application features
- Git evolving better merge conflict resolution

### Risk Mitigation Strategies

1. **LLM Format Evolution**: Design for format versioning/migration as LLMs
   improve and generate different edit formats.

2. **Scale Validation**: Explicit testing with monorepos (10k+ files) to ensure
   performance targets hold under real-world conditions.

3. **Error Recovery Enhancement**: Expand beyond rollback to include partial
   application recovery and checkpoint/resume for large operations.

4. **Backward Compatibility**: Establish clear API contracts and deprecation
   policies for breaking changes.

## 8. CLI Surface (local-only, scriptable)

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

## 9. Safety, Privacy, and Policy (local)

- Path discipline: refuse `..` traversal and symlinks that
  escape the repo unless `--allow-outside-repo` is provided.
- Submodules: read-only by default; edits require explicit
  `--allow-submodules`.
- Binary files: skipped unless `--allow-binary` is set.
- Dry-run everywhere; preview is the default for edits.
- Machine output mode (`--json`) hides color/decoration and
  emits single-line JSON records for easy parsing.

---

## 10. Performance Targets

- Parse + preview a 1k-line spec in under 100 ms.
- Build a 1k-file outline in under 2 s on commodity laptops.
- Assemble a context pack for a symbol under 2 s; heavy packs
  under 5 s.
- Apply multi-file patches with two-phase commit; rollback in
  under 300 ms on error.

---

## 11. Testing Strategy

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

## 12. Release Checklist (per tag)

- All unit/integration/determinism tests pass on Linux/macOS/Windows.
- Exit codes verified for the six canonical scenarios.
- `core.txt` regenerated and committed with provenance header
  (UTC time, commit).
- README updated with current CLI synopsis and examples.

---

## 13. Now / Next (Current Implementation Status)

**COMPLETED: Phase 1 Centralized Backup System** ✅

- [x] Centralized `.rup/backups/` directory with session-scoped organization
- [x] Mirrored directory structure (no filename encoding collisions)
- [x] Atomic session finalization with DONE markers for crash safety
- [x] JSON manifests with Git state and Blake3 checksums
- [x] Append-only index.jsonl for fast session listing
- [x] Proper symlink handling and cross-platform fsync ordering
- [x] Lock file cleanup with guards to prevent deadlocks
- [x] All critical mentor review fixes implemented

**CURRENT: Backup System Integration (Sequential Implementation)**

**Phase B1: Replace Legacy Backup System** (Complete - Following Mentor Guidance)

✅ **Mentor Consultation Complete**: Received comprehensive integration plan with:

- Single session per `rup apply` with contextual API design
- ApplyContext pattern for stateless engines
- Backward-compatible ApplyReport evolution
- Fail-fast backup error handling with CI override option

**Step 1**: Implement Contextual API (Complete)

- [x] **Step 1a**: Add ApplyContext struct and contextual ApplyEngine trait
  - `ApplyContext<'a>` with repo_root, backup: Option<&mut BackupManager>, engine knobs
  - New `apply_with_ctx()` method with backward-compatible `apply()` default
- [x] **Step 1b**: Extend ApplyReport with backward-compatible fields
  - Keep `backup_paths: Vec<PathBuf>` but populate with session directory
  - Add `backup_session_id: Option<String>`, `backup_manifest_path`, `backup_file_count`
- [x] **Step 1c**: Add repo-relative path utility
  - `make_relative_to_repo()` function with boundary enforcement

**Step 2**: Update Engine Implementations

- [x] **Step 2a**: Update InternalEngine with BackupManager integration
  - Use contextual API with `Option<&mut BackupManager>`
  - Back up files before modification, finalize session with success status
- [x] **Step 2b**: Update GitEngine backup handling
  - Back up files that unified diff intends to modify before `git apply`
  - Coordinate with shared session for Auto engine fallback

**Step 3**: Integration Testing and CLI Updates

- [x] **Step 3a**: Add integration tests for new backup flow
  - Single file backup, Auto engine fallback, CI override mode
  - Boundary enforcement, session directory in ApplyReport.backup_paths
- [x] **Step 3b**: Update CLI integration points
  - Modify apply command reporting to show session info
  - Update JSON output with new fields while maintaining compatibility

**Phase B2: Add Backup Management CLI Commands** (Step 4 Complete)

- [✅] **Step 4**: Add backup subcommands to CLI - Read-only operations

  - [x] CLI argument structures with comprehensive help text (BackupSubcommand, BackupListArgs, BackupShowArgs)
  - [x] Core operations module with mentor review fixes applied (backup_ops.rs)
  - [x] CLI command handlers wired in `core/edit.rs` and `main.rs` dispatch
  - [x] JSON output formatting for list/show commands
  - [x] Integration tests for read-only operations (engine filter case-insensitive, alias completion preference, payload size excludes metadata)

- [ ] **Step 5**: Implement restoration logic

  - [ ] `rup backup restore <session> [--path] [--force]` - Restore backups
  - [ ] Basic file restoration with conflict detection
  - [ ] Preview mode for restoration (`--dry-run`, `--show-diff`)
  - [ ] Safe restoration with current file backup (create new session for overwritten files)

- [ ] **Step 6**: Add cleanup policies
  - [ ] `rup backup cleanup [--older-than] [--keep-latest]` - Cleanup old sessions
  - [ ] Age-based cleanup (--older-than=7d)
  - [ ] Count-based cleanup (--keep-latest=20)
  - [ ] Interactive confirmation with --yes override for CI

**Phase B3: Enhanced Backup Features** (Future)

- [ ] **Step 7**: Advanced restoration features

  - Interactive restoration mode
  - Partial session restoration
  - Restoration conflict resolution

- [ ] **Step 8**: Performance optimizations
  - Content deduplication across sessions
  - Compression for large backup sets
  - Streaming checksums for large files

**NEXT: Phase 2 Advanced Edit Features** (After Backup Integration)

- Finish Phase 2 backlog: CREATE/DELETE/RENAME operations
- Two-phase multi-file apply with atomic rollback
- Submodule safety switch and JSON error stream

**DEFERRED: Major Feature Phases**

**Phase 2.5**: Conflict resolution assistant
**Phase 3**: Smart context assembly with semantic intelligence  
**Phase 4**: Renderers and local discovery tools
**Phase 5**: Analysis and dependency tools
**Phase 6**: Session persistence and contexts
**Phase 7**: Ecosystem integration

---

### Implementation Notes

**Current Session Status**: Phase B1 complete + Phase B2 core operations complete, mentor review fixes applied.

**Key Architectural Decisions**:

**Phase B1** (Complete):

1. **Single session per `rup apply`** with shared BackupManager across engine fallbacks
2. **ApplyContext pattern** keeps engines stateless while providing backup session access
3. **Backward compatibility** via extended ApplyReport fields (session dir in backup_paths)
4. **Fail-fast default** for backup errors with `--no-backup-on-error` CI override
5. **Boundary enforcement** via `make_relative_to_repo()` utility function

**Phase B2** (In Progress - Step 4): 6. **Performance-first listing** with filter→sort→limit→manifest-read to keep operations <150ms 7. **Completion policy** for aliases - prefer sessions with DONE markers 8. **Case-insensitive engine filtering** for better UX (--engine=Auto matches auto/AUTO) 9. **Strict relative time parsing** with negative duration rejection 10. **Payload size calculation** excludes manifest.json and DONE metadata files

**CURRENT: Phase B2 - Backup Management CLI Commands** (Next: Steps 5–6)

**Next Session Priorities**

- Implement Step 5 (Restore): safe restoration, preview/diff, checksum verify, and current-file backup
- Implement Step 6 (Cleanup): age/count policies with confirmation and CI-friendly `--yes`

**Architecture Progress**:
✅ **Core Operations Module Complete** (`src/core/backup_ops.rs`):

- Applied all mentor review fixes for correctness and performance
- Session ID resolution with completion preference and robust sorting
- List filtering with minimized manifest IO (filter→sort→limit→read manifests)
- Case-insensitive engine matching, negative duration rejection
- Metadata-excluded size calculation, proper alias handling

**Success Criteria**:

- `rup backup list/show` commands work with all filters and JSON output (validated by tests)
- Performance targets met: list 1k+ sessions <150ms, manifest reads only for top-N results
- Aliases (latest, last-successful) prefer completed sessions

---

## 14. Appendices

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

**End of local-only roadmap (Phases 1–7).**

---

## 15. Implementation Notes

### Phase Interdependencies

- **Phase 2.5 blocks Phase 3**: Conflict resolution must be solid before
  context assembly, as poor conflicts will undermine context quality feedback
- **Phase 4.5 enhances all others**: Feedback loop provides data to improve
  every other component
- **Phase 7 requires stable core**: Ecosystem integration should only begin
  once core functionality (Phases 1-3) is production-proven

### Architecture Considerations

- **Conflict Resolution**: Consider separate conflict detection engine that can
  be used by both internal and git apply modes
- **Semantic Features**: Evaluate lightweight local embedding models (sentence-transformers
  via ONNX) vs. simpler heuristic approaches for initial implementation
- **Feedback Storage**: SQLite provides good local persistence without external
  dependencies, aligns with privacy-first approach

### Success Metrics by Phase

- **Phase 2.5**: >60% reduction in manual conflict resolution time
- **Phase 3**: >40% improvement in context relevance (measured via user surveys)
- **Phase 4.5**: >20% improvement in edit success rate over 3-month period
- **Phase 7**: <2 hour setup time for new CI/CD integrations
