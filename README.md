# Roughup (`rup`)

**Privacy-first CLI for smart code extraction and safe, LLM-assisted editing — entirely local.**

Roughup helps you talk to an LLM about your codebase without sending source anywhere. It builds a _paste-ready_ context from your repo (deterministically), lets you preview suggested edits, and applies them safely with backups.

---

## Why Roughup?

- **Local by design** – No network calls. Your code never leaves your machine.
- **Deterministic** – Same inputs → same outputs. Great for tests, scripts, and CI.
- **Fast** – Memory-mapped reads, parallel lookups, and careful token budgeting.
- **Safe application** – Atomic writes with checkpointed backups and optional git 3-way merge.
- **Model-agnostic** – Use Claude, GPT, local models, or your own tooling.

---

## Quick Start

### Install

```bash
# From source
git https://github.com/consistent-milk12/roughup.git
cd roughup
cargo install --path .
```

Requires recent stable/nightly Rust.

### 5-minute workflow

```bash
# 1) Index your repo (symbols.jsonl)
rup symbols

# 2) Build LLM context (copies to clipboard for easy paste)
rup context --clipboard "authentication" "login"

# 3) Paste into your LLM, get an edit proposal back (EBNF format)

# 4) Preview the proposal
rup preview --clipboard

# 5) Apply safely (backup + atomic write)
rup apply --clipboard
```

---

## Core Concepts

### Smart Context Assembly

Roughup turns queries into a ranked, token-budgeted bundle of code slices:

- **Lookup**: exact/substring/semantic hits from an on-disk symbol index.
- **Overlap-merge**: coalesce adjacent slices per file (stable order).
- **Anchor-aware ranking**: anchor file first → same directory → others (lexicographic).
- **Boosts**:

  - **Fail-signal** boost: lines near errors/warnings are prioritized.
  - **Call-distance** boost: functions near the anchor in a cheap call graph get a bounded lift.
  - **History-aware** downrank: deprioritize repeats from previous runs.

- **Fitting**: token budget enforced deterministically; optional buckets & dedupe.
- **Output**: paste-ready blocks with optional code fences, or structured JSON.

Examples:

```bash
# Basic context
rup context "MyClass" "handle_request"

# Semantic mode + budget control
rup context --semantic --budget 8000 "error handling" "validation"

# Task presets
rup context --template bugfix "authentication" "security"

# Anchor-aware proximity
rup context --anchor src/auth.rs --anchor-line 45 "login" "session"

# Tier presets (A/B/C) tune budget & intake caps
rup context --tier B "router" "middleware"

# Fail-signal boost from a compiler log
rup context --fail-signal target/rustc.log "borrow checker" "lifetime"

# Lightweight callgraph-driven expansion
rup context --callgraph 'anchor=src/main.rs:120 depth=2 files_per_hop=20 edges=300' "init"
```

Useful flags (selection):

- `--semantic` toggle semantic search in lookups
- `--budget <tokens>`
- `--tier <A|B|C>` (sets budget, limit, and per-query caps)
- `--limit <n>` and `--top-per-query <n>`
- `--anchor <path>` and `--anchor-line <1-based>`
- `--fail-signal <path>` (rustc-style logs are auto-parsed)
- `--callgraph '<k=v ...>'` (see example above)
- `--buckets '<Tag:cap,...>'` and `--novelty-min <0..1>`
- `--dedupe-threshold <0..1>`
- `--fence` (wrap snippets in language fences)
- `--json` (machine-readable output)
- `--clipboard` (copy output text)

### Edit Application (EBNF format)

Roughup applies edits described in a simple, human-readable format. You can paste the LLM’s proposal directly:

````ebnf
FILE: src/auth.rs
REPLACE lines 10-15:
OLD:
```rust
fn login(user: &str) -> bool {
    // old implementation
}
````

NEW:

```rust
fn login(user: &str, password: &str) -> Result<Session, AuthError> {
    // improved implementation
}
```

````

Commands:

```bash
# Show what would change (diff-like view)
rup preview --clipboard

# Apply changes atomically with backup
rup apply --clipboard

# Use git 3-way merge engine (more robust conflict handling)
rup apply --engine git --clipboard
````

---

## Commands Overview

| Command   | Purpose                                   | Example                                  |
| --------- | ----------------------------------------- | ---------------------------------------- |
| `symbols` | Build/update symbol index                 | `rup symbols --include-private`          |
| `tree`    | Show project structure & line counts      | `rup tree --depth 3`                     |
| `context` | Build ranked, budgeted paste-ready slices | `rup context --semantic "auth"`          |
| `extract` | Extract files/ranges                      | `rup extract src/lib.rs:1-100`           |
| `chunk`   | Token-aware chunking of large files       | `rup chunk src/huge.rs`                  |
| `preview` | Preview EBNF edits                        | `rup preview --clipboard`                |
| `apply`   | Apply edits with backups/engines          | `rup apply --engine git --clipboard`     |
| `backup`  | Manage backups                            | `rup backup list` / `rup backup restore` |

---

## Configuration (`roughup.toml`)

Create at repo root:

```toml
[symbols]
output_file = "symbols.jsonl"
include_private = false
languages = ["rust","python"]

[chunk]
model = "gpt-4o"
max_tokens = 4000

[context]
default_budget = 6000
default_template = "freeform"
fence = true

[apply]
engine = "internal" # or "git"
backup = true
```

Environment:

- `ROUGHUP_NO_AUTO_INDEX=1` — disable automatic symbol indexing/regeneration.

---

## Deeper Details

### Determinism & Ordering

- Ranking happens _after_ overlap-merge.
- Paths compare repo-relative; anchor equality is robust to abs/rel/symlinks.
- Stable tiebreaks: path asc, then start line.

### Buckets & Dedupe

- Bucket fitting lets you enforce token caps per tag (e.g., `Interface`, `Code`).
- Optional Jaccard dedupe reduces near-duplicate spans before fitting.

### Fail-Signals

- Provide a compiler/test log via `--fail-signal`. Roughup boosts slices near reported lines, weighted by severity (Info/Warn/Error).

### Call-Distance Boost

- With an anchor file/line, Roughup estimates a tiny callgraph around it and applies a bounded priority boost to nearby functions (kept conservative to preserve determinism).

### Backups & Safety

- Every `apply` creates a sessioned backup you can list, inspect, and restore.
- Writes are atomic; failures roll back cleanly.

---

## Examples

### Refactor a subsystem

```bash
rup symbols
rup context --template refactor --tier C "DatabaseConnection" "ConnectionPool" --clipboard
# paste to your LLM → copy EBNF back
rup preview --clipboard
rup apply --clipboard
```

### Investigate a bug with proximity and logs

```bash
rup context \
  --template bugfix \
  --anchor src/error.rs --anchor-line 200 \
  --fail-signal target/test.log \
  "handle_error" "logging"
```

### Work with a local model

```bash
rup context --json "optimize hot path" > ctx.json
# ... send ctx.json to your local model and get edits.ebnf ...
rup apply edits.ebnf
```

---

## Performance Tips

- Run `rup symbols` once per change burst; auto-refresh is enabled unless you set `ROUGHUP_NO_AUTO_INDEX`.
- Prefer **SVG fenced output** (`--fence`) for clearer pasting into LLM UIs.
- Use `--tier` to scale intake quickly (A=small, B=medium, C=large).
- Add an `--anchor` when you know the touchpoint; ranking gets much sharper.

---

## Troubleshooting

- **“Symbols file not found”**
  Run `rup symbols` (or unset `ROUGHUP_NO_AUTO_INDEX`).

- **Context is too big**
  Lower `--budget`, choose a smaller `--tier`, or add `--limit / --top-per-query`.

- **Clipboard errors (headless/WSL)**
  Omit `--clipboard` and redirect to a file: `rup context > ctx.md`.

- **No matches**
  Try relaxed queries, `--semantic`, or provide an anchor.

- **Index never refreshes**
  Delete the stale `symbols.jsonl` and re-run `rup symbols` (or ensure auto-indexing is enabled).

---

## Contributing

PRs and issues welcome! Good first areas:

- New language symbolizers
- Additional fail-signal parsers
- Better bucket strategies & tagging heuristics
- Editor integrations (Vim/VSCode/Helix)

Please run the test suite and keep changes deterministic.

---

## Security & Privacy

- No telemetry, no network calls.
- All processing is local; outputs are files you control.
- Backups live in your repo’s workspace and are easy to prune.

---

## License

MIT. See `LICENSE`.

---

**Built with:** `clap`, `rayon`, `tree-sitter` (where applicable), and a lot of careful I/O.
