# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

**roughup** is a high-performance CLI tool optimized for LLM workflows. It extracts, processes, and packages source code for AI model consumption with focus on speed, cross-platform compatibility, and token-aware processing.

## Development Commands

```bash
# Build and test
cargo build --release    # Optimized build (produces 'rup' binary)
cargo test              # Full test suite including cross-platform tests
cargo fmt && cargo clippy  # Format and lint

# Install locally
cargo install --path .   # Installs as 'rup' command

# Run specific functionality
cargo run -- extract --help     # Line-range extraction
cargo run -- symbols --help     # Symbol extraction via Tree-sitter
cargo run -- chunk --help       # Token-aware chunking
cargo run -- tree --help        # Directory visualization
```

## Architecture Overview

### Core Processing Pipeline (`src/core/`)
- **extract/**: Memory-mapped line-range extraction with Windows drive support
- **symbols.rs**: Tree-sitter AST parsing pipeline (Rust, Python, extensible)  
- **chunk.rs**: Token-aware content chunking using tiktoken-rs for LLM workflows
- **tree.rs**: Gitignore-aware directory traversal with line counting

### Infrastructure Layer (`src/infra/`)
- **io.rs**: Smart file I/O with 1MB memory-mapping threshold for performance
- **line_index.rs**: CRLF/LF-robust line indexing with O(1) byte→line mapping
- **walk.rs**: Parallel directory walking with rayon and gitignore integration
- **config.rs**: Hierarchical TOML/YAML/JSON config with environment variable support

### CLI Architecture (`src/cli.rs`, `src/main.rs`)
- **AppContext pattern**: Global flags (quiet, no-color, dry-run) threaded through all commands
- **Command pattern**: Each subcommand has dedicated Args struct and run function
- **Strategic re-exports** in lib.rs for clean external API (utils kept private)

## Key Design Principles

### Performance-First
- Memory mapping for large files (>1MB threshold)
- Parallel processing with rayon for file walking and symbol extraction
- AST caching with moka to avoid re-parsing
- Pre-allocated capacity for collections based on heuristics

### Cross-Platform Robustness  
- Windows drive letter support in path parsing (`C:\path\file.rs:10-20`)
- CRLF/LF line ending handling with comprehensive test coverage
- Deterministic output with sorted file lists for stable CLI behavior

### LLM Workflow Optimization
- Token-aware chunking with configurable overlap for context preservation
- Symbol-boundary preferences when chunking to maintain code coherence
- JSONL output format for structured symbol data
- Metadata preservation (line numbers, qualified names, visibility)

## Language Support Extension

To add new language parsers:
1. Create parser in `src/parsers/` following the trait pattern
2. Add language detection in symbols pipeline
3. Register in the extractor registry
4. Add comprehensive tests with sample code

Current parsers: Rust (with qualified names), Python (classes/functions/methods)

## Configuration System

- **File hierarchy**: `roughup.toml` → `roughup.yaml` → `roughup.json` → `.roughup.toml`
- **Environment variables**: `ROUGHUP_` prefix support
- **CLI precedence**: Command-line arguments override config files
- **Structured sections**: walk, extract, tree, symbols, chunk with sensible defaults

## Testing Strategy

- Unit tests embedded in modules with `#[cfg(test)]`
- Integration tests in `tests/` directory focusing on cross-platform compatibility  
- Tempfile-based filesystem testing for isolation
- Cross-platform line ending validation (LF/CRLF)

## Important Implementation Notes

- **Symbol extraction** is structured into small focused structs with associated functions only
- **Error handling** uses anyhow for contextual user-friendly messages with graceful fallbacks
- **Memory management** includes smart thresholds and efficient byte slicing for line extraction
- **CLI UX** includes progress bars, colored output, and shell completion generation
- **Binary name** is `rup` (shortened from roughup) for CLI convenience

## Session Persistence and Accuracy Protocols

### **MANDATORY: Start Every Session**
1. **Read TODO.md first** - Always check current roadmap and status
2. **Read Suggestions.md** - Reference the complete implementation specification
3. **Check latest git status** - Understand what changed since last session
4. **Update TODO.md** - Mark completed items, add new discoveries, adjust priorities

### **Work Continuity Rules**
- **Never assume prior context** - Always verify current state before starting work
- **Update TODO.md immediately** - When completing tasks, discovering issues, or changing direction
- **Reference exact line numbers** - Use `file_path:line_number` format for all code discussions
- **Maintain phase discipline** - Complete Phase 1 before moving to Phase 2 features
- **Test as you go** - Every component must have tests before considering it complete

### **Implementation Accuracy Standards**
- **Follow Suggestions.md exactly** - The spec provides precise algorithms, CLI interfaces, and safety mechanisms
- **Maintain existing patterns** - Study current codebase architecture before adding new components
- **Cross-platform compatibility** - Test Windows (CRLF) and Unix (LF) line endings
- **Safety first** - Always implement preview/backup mechanisms before write operations
- **Performance awareness** - Use existing memory mapping and parallel processing patterns

### **Communication Protocols**
- **Reference TODO.md phases** - Always specify which phase and task you're working on
- **Update completion status** - Mark items as completed in TODO.md when truly finished
- **Document blockers** - If stuck, update TODO.md with specific issues and potential solutions
- **Maintain changelog** - Update TODO.md "Last Updated" section with significant changes

### **Quality Gates**
- **P1 (Edit System) Gate** - Must have: edit parsing, conflict detection, preview, backup, tests
- **P2 (Context) Gate** - Must have: symbol indexing, budget management, context assembly, CID system
- **Each component** - Must have: error handling, tests, documentation, cross-platform support

### **Error Recovery**
- **If session context lost** - Start with TODO.md → Suggestions.md → git status workflow
- **If implementation diverges** - Reconcile with Suggestions.md spec and update TODO.md accordingly
- **If tests fail** - Fix immediately before proceeding to next feature

This ensures continuous progress across multiple Claude Code sessions with full context preservation and implementation accuracy.