# Roughup Deep-Review Context (Starter Header)

## One-liner

Roughup is a high-performance Rust CLI for LLM workflows. It parses a human-readable EBNF edit spec and applies changes via a dual engine system: fast internal engine and Git 3-way merge, with an "auto" fallback mode.

## Current focus

Phase 2: Git integration and CLI hardening. Preview-first UX, explicit --apply for writes, standardized exit codes, repo detection, and auto fallback.

## Primary interfaces to review

- src/core/apply_engine.rs (trait, factory, auto fallback logic)
- src/core/git.rs (git apply, 3-way, whitespace handling, conflict surfacing)
- src/core/patch.rs (EBNF → unified diff, context lines)
- src/core/edit.rs (parser, GUARD-CID, atomic writes, CRLF/LF handling)
- src/cli.rs (flags: --apply, --engine, --git-mode, --context-lines, --whitespace, global --quiet|--no-color|--dry-run)

## Exit codes

0 success; 2 conflicts; 3 invalid spec; 4 repository issues; 5 internal error.

## Review objectives

- Validate preview-first flow and explicit --apply requirement.
- Confirm EngineChoice factory wiring and graceful auto behavior when no repo is present.
- Verify repo-root detection and boundary safety.
- Ensure context_lines and whitespace policy propagate to both diff generation and git apply.
- Evaluate conflict reporting quality and consistency across engines.
- Check atomic write semantics and cross-platform correctness.

## Constraints

Backward compatible CLI, fast internal path, data-loss prevention, Windows/Unix support, user-friendly errors.

---

## Paste Artifacts Below (exact order and tags)

=== tree.txt (project overview) ===
[34m.[39m/
├─ .gitignore:5
├─ [37mCLAUDE.md[39m:135
├─ [94mCargo.toml[39m:79
├─ LICENSE:21
├─ [37mREADME.md[39m:364
├─ [37mTODO.md[39m:476
├─ context
│  └─ [37mtree.txt[39m:0
├─ [94mroughup.toml[39m:29
├─ script.sh:44
├─ src
│  ├─ [33mcli.rs[39m:315
│  ├─ [33mcompletion.rs[39m:39
│  ├─ core
│  │  ├─ [33mapply_engine.rs[39m:333
│  │  ├─ [33mchunk.rs[39m:465
│  │  ├─ [33medit.rs[39m:1392
│  │  ├─ extract
│  │  ├─ [33mgit.rs[39m:434
│  │  ├─ [33mpatch.rs[39m:511
│  │  ├─ [33msymbols.rs[39m:582
│  │  └─ [33mtree.rs[39m:297
│  ├─ infra
│  │  ├─ [33mconfig.rs[39m:145
│  │  ├─ [33mio.rs[39m:154
│  │  ├─ [33mline_index.rs[39m:132
│  │  ├─ [33mutils.rs[39m:579
│  │  └─ [33mwalk.rs[39m:270
│  ├─ [33mlib.rs[39m:95
│  ├─ [33mmain.rs[39m:29
│  └─ parsers
│     ├─ [33mpython_parser.rs[39m:500
│     └─ [33mrust_parser.rs[39m:451
└─ tests
   └─ [33mline_index_tests.rs[39m:89
=== /tree.txt ===

=== symbols.jsonl (symbol inventory) ===
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"AppContext","qualified_name":"AppContext","byte_start":140,"byte_end":296,"start_line":6,"end_line":10,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"Cli","qualified_name":"Cli","byte_start":501,"byte_end":877,"start_line":18,"end_line":33,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"enum","name":"Commands","qualified_name":"Commands","byte_start":901,"byte_end":1662,"start_line":36,"end_line":66,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"ExtractArgs","qualified_name":"ExtractArgs","byte_start":1682,"byte_end":2174,"start_line":69,"end_line":88,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"TreeArgs","qualified_name":"TreeArgs","byte_start":2194,"byte_end":2489,"start_line":91,"end_line":103,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"SymbolsArgs","qualified_name":"SymbolsArgs","byte_start":2516,"byte_end":2951,"start_line":106,"end_line":122,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"ChunkArgs","qualified_name":"ChunkArgs","byte_start":2971,"byte_end":3685,"start_line":125,"end_line":149,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"ApplyArgs","qualified_name":"ApplyArgs","byte_start":3705,"byte_end":5048,"start_line":152,"end_line":199,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"enum","name":"ApplyEngine","qualified_name":"ApplyEngine","byte_start":5085,"byte_end":5309,"start_line":202,"end_line":209,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"enum","name":"GitMode","qualified_name":"GitMode","byte_start":5346,"byte_end":5578,"start_line":212,"end_line":220,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"enum","name":"WhitespaceMode","qualified_name":"WhitespaceMode","byte_start":5615,"byte_end":5787,"start_line":223,"end_line":230,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"PreviewArgs","qualified_name":"PreviewArgs","byte_start":5807,"byte_end":6738,"start_line":233,"end_line":264,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"CheckSyntaxArgs","qualified_name":"CheckSyntaxArgs","byte_start":6758,"byte_end":6860,"start_line":267,"end_line":270,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"BackupArgs","qualified_name":"BackupArgs","byte_start":6880,"byte_end":7077,"start_line":273,"end_line":280,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"InitArgs","qualified_name":"InitArgs","byte_start":7097,"byte_end":7295,"start_line":283,"end_line":291,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"enum","name":"Shell","qualified_name":"Shell","byte_start":7332,"byte_end":7407,"start_line":294,"end_line":300,"visibility":"public","doc":null}
{"file":"src/cli.rs","lang":"rust","kind":"struct","name":"CompletionsArgs","qualified_name":"CompletionsArgs","byte_start":7427,"byte_end":7747,"start_line":303,"end_line":315,"visibility":"public","doc":null}
=== /symbols.jsonl ===

=== core.txt (annotated sources: cli, edit, patch, git, apply_engine) ===
[Note: Core source files containing the complete annotated source code for all reviewed modules - see context/core.txt for full content]
=== /core.txt ===

=== chunks/ (LLM-sized slices; paste in filename order) ===
--- chunks/index (list each chunk filename you are pasting) ---
chunk_001.txt
--- /chunks/index ---
--- chunk: chunk_001.txt ---
[Note: Single chunk containing all core source content - see context/chunks/chunk_001.txt for full content]
--- /chunk: chunk_001.txt ---
=== /chunks/ ===

=== preview.diff (optional, from `cargo run -- preview ...`) ===
No sample edit files found, skipping preview.diff generation
=== /preview.diff ===

---

## Reviewer Contract (what to return)

Please provide:

1. **A line-anchored findings list keyed to core.txt line numbers.**

2. **A short risk matrix covering:** preview/apply flow, auto fallback, repo detection, context/whitespace propagation, conflict UX, atomic write path.

3. **Concrete patchlets or function-local diffs to address issues found.**

4. **A minimal e2e test plan exercising exit codes 0/2/3/4/5 and repo/no-repo scenarios.**

---

## Regeneration (for reference only; do not run here)

Tree: `cargo run -- tree --depth 3 --ignore target --ignore .git > context/tree.txt`

Symbols: `cargo run -- symbols . --output context/symbols.jsonl`

Core bundle: for each core file f, run
`cargo run -- extract "$f:1-$(wc -l < $f)" --annotate --fence --output context/core.txt`

Chunks:
`cargo run -- chunk context/core.txt --model o200k_base --max-tokens 5000 --overlap 200 --by-symbols true --output-dir context/chunks`

Preview (optional):
`cargo run -- preview edits/sample.rup --engine auto --git-mode 3way --whitespace nowarn --repo-root . > context/preview.diff`