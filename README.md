# roughup

A production-ready CLI tool optimized for LLM workflows with bidirectional code editing, smart extraction, and safe change management.

**Key Features:**

- **Hybrid Edit System** - Human-readable EBNF input with Git's robust 3-way merge application
- **⚡ High-Performance Extraction** - Memory-mapped file I/O, parallel processing, deterministic output
- **LLM-Optimized Output** - Token-aware chunking, symbol extraction, structured formats
- **Enterprise Safety** - Preview-first UX, atomic writes, backup management, exit codes
- **🌐 Cross-Platform** - Windows drive letters, CRLF/LF preservation, tested compatibility

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

### LLM-Assisted Editing

#### Apply Edit Specifications

```bash
# Preview changes (safe default)
rup apply edit_spec.txt

# Apply changes with backup
rup apply edit_spec.txt --apply --backup

# Use different engines for robustness
rup apply edit_spec.txt --apply --engine git --mode 3way
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

# Preview changes with unified diff
rup preview edit_spec.txt --show-diff

# Create manual backups
rup backup src/important.rs src/critical.rs
```

---

## Engine Architecture

### Hybrid EBNF→Git System

**Internal Engine (Default)**

- Fast, clear error messages
- Direct file manipulation
- Perfect for simple edits

**Git Engine (Robust)**

- Leverages `git apply` with 3-way merge
- Handles code drift and conflicts
- Context-aware relocation

**Auto Engine (Intelligent)**

- Tries internal first for speed
- Falls back to git on conflicts
- Best of both worlds

```bash
# Engine selection
rup apply --engine internal  # Fast path
rup apply --engine git       # Robust path
rup apply --engine auto      # Smart fallback (default)
```

### Safety Features

**Preview-First UX**

```bash
rup apply edit.txt           # Shows preview only
rup apply edit.txt --apply   # Actually writes changes
```

**Conflict Management**

- GUARD-CID system for change detection
- Comprehensive validation before writing
- Clear conflict reporting with suggestions

**Backup System**

```bash
# Automatic timestamped backups
main.rs → main.rup.bak.1703123456.rs
```

---

## Exit Codes (CI/Automation Ready)

- `0` - Success
- `2` - Conflicts detected
- `3` - Invalid input/syntax
- `4` - Repository issues
- `5` - Internal errors

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
engine = "auto"
backup = true
context_lines = 3
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

# Git-specific options
rup apply patch.txt \
  --engine git \
  --mode 3way \
  --whitespace fix
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
cargo fmt && cargo clippy  # Format and lint
cargo test                 # Full test suite (46+ tests)
cargo build --release      # Optimized build

# Install locally as 'rup'
cargo install --path .
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

- **Unit Tests**: Embedded in modules with `#[cfg(test)]`
- **Integration Tests**: Cross-platform compatibility focused
- **Performance Tests**: Memory usage and speed validation
- **Production Tests**: Real-world edit application scenarios

---

## Roadmap

### Phase 1 & 2: Production-Ready Edit System (Complete)

- Hybrid EBNF→Git architecture with auto-fallback
- Safe preview-first UX with atomic writes
- Cross-platform compatibility and enterprise robustness
- Professional CLI with standardized exit codes

### Phase 3: Smart Context Assembly (Planned)

- Queryable symbol index with dependency tracking
- Budget-aware context selection for token limits
- Automated test and helper inclusion
- Chat-optimized output with CID headers

### Phase 4+: Advanced Features (Future)

- Enhanced output formats (clean, annotated, simple)
- Advanced dependency analysis and impact assessment
- Session management and context persistence
- IDE integrations and ecosystem tools

---

## License

MIT OR Apache-2.0

---

**Note**: Language support is intentionally locked to Rust + Python through Phase 3 to maintain focus and quality.
