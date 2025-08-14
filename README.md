# roughup

A CLI tool optimized for LLM workflows with bidirectional code editing, smart extraction, and safe change management.

**Key Features:**

- **Hybrid Edit System** - Human-readable EBNF input with Git's robust 3-way merge application
- **High-Performance Extraction** - Memory-mapped file I/O, parallel processing, deterministic output
- **LLM-Optimized Output** - Token-aware chunking, symbol extraction, structured formats
- **Cross-Platform Excellence** - Windows/Unix parity, CRLF/LF preservation, tested compatibility
- **Production Hardening** - Zero-warning builds, typed error system, comprehensive validation

---

## Installation

### Quick Install

```bash
cargo install --path .
```

### Build from Source

```bash
# Release build (optimized binary named 'rup')
cargo build --release

# Binary location: target/release/rup
```

> **Requirements:** Rust 2024 edition (MSRV 1.85+)

---

## Core Commands

### Project Exploration

```bash
# Project tree with line counts (gitignore-aware)
rup tree --depth 2

# Extract specific line ranges with annotations
rup extract src/lib.rs:1-75 src/cli.rs:1-171 --annotate --fence -o extracted.txt

# Symbol extraction (Rust + Python via Tree-sitter)
rup symbols --languages rust,python --output symbols.jsonl
```

### LLM-Assisted Editing (Production-Ready)

#### Apply Edit Specifications

```bash
# Preview changes (safe default - no writes)
rup apply edit_spec.txt

# Preview with custom context lines
rup preview edit_spec.txt --context-lines 5

# Apply changes with backup
rup apply edit_spec.txt --apply --backup

# Use different engines for robustness
rup apply edit_spec.txt --apply --engine git --git-mode 3way
rup apply edit_spec.txt --apply --engine auto  # Smart fallback (recommended)
```

#### EBNF Edit Format

````
FILE: src/main.rs
GUARD-CID: a1b2c3d4e5f6g7h8
REPLACE lines 10-15:
OLD:
```rust
fn old_function() {
    println!("old implementation");
}
````

NEW:

```rust
fn new_function() {
    println!("improved implementation");
    // Added error handling
}
```

INSERT at 20:
NEW:

```rust
// Additional helper function
fn helper() -> Result<()> {
    Ok(())
}
```

````

### Content Processing
```bash
# Token-aware chunking for GPT models
rup chunk large_file.rs --model gpt-4o --max-tokens 4000 --overlap 128

# Chunk with symbol boundary preferences
rup chunk codebase.txt --by-symbols --output-dir chunks/
````

### Validation & Safety

```bash
# Validate edit syntax
rup check-syntax edit_spec.txt

# Preview changes with unified diff and custom context
rup preview edit_spec.txt --show-diff --context-lines 5

# Create manual backups
rup backup src/important.rs src/critical.rs
```

---

## Engine Architecture

### Hybrid EBNF→Git System

**Internal Engine (Fast)**

- Lightning-fast direct file manipulation
- Clear, structured error messages
- Perfect for simple, clean edits
- Comprehensive validation and conflict detection

**Git Engine (Robust)**

- Leverages `git apply` with 3-way merge capability
- Handles code drift and context relocation
- Professional patch generation with standard headers
- Comprehensive error parsing and user guidance

**Auto Engine (Intelligent - Recommended)**

- Tries internal first for maximum speed
- Automatically falls back to git on conflicts
- Best of both worlds with smart decision making
- Production-tested fallback logic

```bash
# Engine selection
rup apply --engine internal  # Fast path (direct file manipulation)
rup apply --engine git       # Robust path (3-way merge)
rup apply --engine auto      # Smart fallback (recommended default)

# Git engine modes
rup apply --engine git --git-mode 3way    # Resilient (may leave conflict markers)
rup apply --engine git --git-mode index   # Clean tree required
# rup apply --engine git --git-mode worktree # (Not yet implemented)
```

### Safety Features

**Preview-First UX**

```bash
rup apply edit.txt           # Shows preview only (safe default)
rup apply edit.txt --apply   # Actually writes changes
rup preview edit.txt         # Dedicated preview command with more options
```

**Advanced Conflict Management**

- GUARD-CID system with deterministic content hashing (xxh64)
- Comprehensive validation with typed error taxonomy
- Clear conflict reporting with actionable suggestions
- Machine-readable conflict output for scripts

**Robust Backup System**

```bash
# Automatic timestamped backups with extension preservation
main.rs → main.rup.bak.1703123456.rs
config.toml → config.rup.bak.1703123456.toml
```

**Cross-Platform Atomic Operations**

- Windows-safe file replacement with proper permission handling
- Unix/Linux compatibility with fsync guarantees
- Cross-filesystem fallback for atomic writes
- Repository boundary validation and symlink protection

---

## Exit Codes (CI/Automation Ready)

Standardized exit codes for reliable automation and CI/CD integration:

- `0` - Success (no conflicts, changes applied successfully)
- `2` - Conflicts detected (use --force to override, or resolve manually)
- `3` - Invalid input/syntax (fix edit specification format)
- `4` - Repository issues (boundary violations, git repo problems)
- `5` - Internal errors (file I/O, system issues)

---

## Configuration

Create `roughup.toml` for project defaults:

```toml
[walk]
ignore = ["target/**", "node_modules/**", ".git/**"]
depth = 3

[extract]
annotate = true
fence = true

[symbols]
languages = ["rust", "python"]
include_private = false

[chunk]
model = "gpt-4o"
max_tokens = 4000
overlap = 128
by_symbols = true

[apply]
engine = "auto"        # Recommended: smart fallback
git_mode = "3way"      # Resilient 3-way merge
backup = true          # Always create backups
context_lines = 3      # Standard patch context
force = false          # Require explicit --force for conflicts
whitespace = "nowarn"  # LLM-friendly (ignore whitespace issues)
```

Initialize with:

```bash
rup init
```

---

## Advanced Usage

### Multi-Engine Workflow

```bash
# Complex edit with fallback strategy
rup apply complex_refactor.txt \
  --engine auto \
  --backup \
  --context-lines 5 \
  --verbose

# Git-specific options with whitespace handling
rup apply patch.txt \
  --engine git \
  --git-mode 3way \
  --whitespace fix

# Preview with same context as apply
rup preview patch.txt --context-lines 5
rup apply patch.txt --apply --context-lines 5
```

### Repository Integration

```bash
# Auto-detect git repository
rup apply --repo-root .

# Explicit repository root
rup apply --repo-root /path/to/repo edit.txt
```

### Shell Completions

```bash
# Generate completions
rup completions bash --out-dir ~/.local/share/bash-completion/completions
rup completions zsh --out-dir ~/.oh-my-zsh/completions
rup completions fish --out-dir ~/.config/fish/completions
```

---

## Performance & Architecture

### High-Performance Design

- **Memory Mapping**: Files >1MB use mmap for speed
- **Parallel Processing**: Multi-threaded file walking and symbol extraction
- **AST Caching**: Moka cache for repeated Tree-sitter operations
- **Deterministic Output**: Stable, reproducible results

### Cross-Platform Robustness

- **Windows Support**: Drive letter parsing (`C:\path\file.rs:10-20`)
- **Line Ending Handling**: Automatic CRLF/LF detection and preservation
- **Atomic Operations**: Safe file replacement with permission preservation
- **Path Safety**: Repository boundary validation and symlink protection
- **Permission Handling**: Unix/Windows compatible with proper fallbacks
- **Tempfile Strategy**: Cross-filesystem atomic writes with robust error handling

### LLM Workflow Optimization

- **Token-Aware Processing**: tiktoken-rs integration for accurate token counting
- **Symbol Boundaries**: Intelligent chunking that preserves code structure
- **Structured Output**: JSONL, fenced blocks, and metadata preservation
- **Context Assembly**: Smart relevance ranking and budget management

---

## Development

### Build & Test

```bash
# Development workflow
cargo fmt && cargo clippy --all-targets  # Format and lint
cargo test                               # Full test suite (46+ tests)
cargo build --release                    # Optimized build

# Install locally as 'rup'
cargo install --path .

# Verify installation
rup --version
rup apply --help
```

### Architecture Overview

```
src/
├── core/                   # High-performance processing pipeline
│   ├── edit.rs            # EBNF parsing & application engine
│   ├── patch.rs           # EBNF→unified diff converter
│   ├── git.rs             # Git apply integration
│   ├── apply_engine.rs    # Unified engine trait architecture
│   ├── symbols.rs         # Tree-sitter symbol extraction
│   └── extract/           # Line-range extraction with mmap
├── infra/                 # Infrastructure & utilities
│   ├── io.rs              # Smart file I/O with performance thresholds
│   ├── line_index.rs      # Cross-platform line indexing
│   └── walk.rs            # Parallel directory traversal
├── parsers/               # Language-specific extractors
│   ├── rust_parser.rs     # Rust symbols with qualified names
│   └── python_parser.rs   # Python classes, functions, methods
└── cli.rs                 # Command-line interface
```

### Testing Strategy

- **Unit Tests**: Embedded in modules with `#[cfg(test)]` - 46+ test cases
- **Integration Tests**: Cross-platform compatibility focused
- **Production Hardening**: Max-depth code review with critical fixes applied
- **Performance Tests**: Memory usage and speed validation
- **Real-World Validation**: Actual git repository testing with complex edits
- **Error Path Coverage**: Comprehensive error handling and edge case testing

---

## Roadmap

### Phase 1 & 2: Production-Ready Edit System (Complete)

- ** Hybrid EBNF→Git architecture** with auto-fallback intelligence
- ** Safe preview-first UX** with atomic writes and comprehensive validation
- ** Cross-platform excellence** with Windows/Unix parity and robust error handling
- ** Professional CLI** with standardized exit codes and typed error taxonomy
- ** Production hardening** with max-depth code review fixes applied
- ** Zero-warning builds** with comprehensive type safety and memory management
- ** Enterprise robustness** with repository boundary validation and atomic operations

### Phase 3: Smart Context Assembly (In Progress)

- Queryable symbol index with dependency tracking
- Budget-aware context selection for token limits
- Automated test and helper inclusion logic
- Chat-optimized output with CID headers and relevance ranking

### Phase 4+: Advanced Features (Future)

- Enhanced output formats (clean, annotated, simple)
- Advanced dependency analysis and impact assessment
- Session management and context persistence
- IDE integrations and ecosystem tools

---

## License

MIT OR Apache-2.0

---

**Note**: Language support is intentionally scoped to Rust + Python through Phase 3 to maintain focus and quality. The architecture is designed for easy extension to additional languages in future phases.

**Status**: Production-ready with comprehensive hardening. Phase 2 complete with all critical fixes applied. Ready for Phase 3 development.
