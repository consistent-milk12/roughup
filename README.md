# Roughup (`rup`)

**Privacy-first CLI for smart code extraction and LLM-assisted editing**

Roughup is a blazing-fast Rust CLI that helps you work with Large Language Models on your codebase without sending your code anywhere. Extract relevant context, get LLM suggestions, preview changes, and apply them atomically—all while keeping your code local.

## Key Features

- **100% Local**: No network calls, your code never leaves your machine
- **Smart Context**: Intelligent symbol ranking and token budgeting
- **Lightning Fast**: Sub-second context assembly, memory-mapped I/O
- **Safe Edits**: Atomic operations with automatic backups and rollback
- **Deterministic**: Same input always produces same output
- **LLM Agnostic**: Works with Claude, GPT, local models, or any LLM

## Quick Start

### Installation

```bash
# From crates.io (coming soon)
cargo install roughup

# From source
git clone https://github.com/yourusername/roughup
cd roughup
cargo install --path .
```

### Basic Workflow

```bash
# 1. Index your codebase
rup symbols

# 2. Get smart context for your query
rup context --clipboard "authentication" "login"

# 3. Paste context to LLM, get edit suggestions, copy them

# 4. Preview the changes
rup preview --clipboard

# 5. Apply safely with backup
rup apply --clipboard
```

## Core Workflows

### Smart Context Assembly

Extract the most relevant code for your LLM conversation:

```bash
# Basic context extraction
rup context "MyClass" "handle_request"

# Semantic search with budget control
rup context --semantic --budget 8000 "error handling" "validation"

# Template-specific context (refactor/bugfix/feature)
rup context --template bugfix "authentication" "security"

# Anchor-aware proximity ranking
rup context --anchor src/auth.rs --anchor-line 45 "login" "session"
```

**Output**: Intelligently ranked code snippets that fit your token budget, ready to paste into any LLM.

### Safe Code Editing

Apply LLM suggestions with confidence:

```bash
# Preview changes before applying
rup preview --clipboard

# Apply with automatic backup
rup apply --clipboard

# Force apply with git 3-way merge
rup apply --clipboard --engine git --force
```

**Edit Format**: Use simple EBNF syntax that both humans and tools can understand:

```ebnf
FILE: src/auth.rs
REPLACE lines 10-15:
OLD:
```rust
fn login(user: &str) -> bool {
    // old implementation
}
```

NEW:
```rust
fn login(user: &str, password: &str) -> Result<Session, AuthError> {
    // improved implementation
}
```

### Project Exploration

Understand your codebase structure:

```bash
# Project tree with line counts
rup tree

# Extract specific files or ranges
rup extract src/main.rs:1-50 src/lib.rs:100-200

# Symbol index with visibility info
rup symbols --include-private
```

### Backup & Recovery

Never lose work with session-based backups:

```bash
# List backup sessions
rup backup list

# Show backup details
rup backup show abc123

# Restore from backup
rup backup restore abc123
```

## Use Cases

### Large Refactoring
```bash
# Get context for the subsystem you're refactoring
rup context --template refactor --budget 12000 "DatabaseConnection" "ConnectionPool"

# Apply LLM-suggested changes with preview
rup preview --clipboard
rup apply --clipboard
```

### Bug Investigation
```bash
# Find relevant code around the issue
rup context --template bugfix --anchor src/error.rs "handle_error" "logging"

# Apply fix with git 3-way merge for safety
rup apply --clipboard --engine git
```

### Feature Development
```bash
# Gather context for new feature
rup context --template feature "API" "endpoints" "handlers"

# Apply incrementally with backups
rup apply --clipboard --backup
```

## Available Commands

| Command | Purpose | Example |
|---------|---------|---------|
| `symbols` | Index codebase symbols | `rup symbols` |
| `tree` | Show project structure | `rup tree --depth 3` |
| `context` | Smart context assembly | `rup context --semantic "auth"` |
| `extract` | Extract specific files/ranges | `rup extract src/lib.rs:1-100` |
| `preview` | Preview LLM edit suggestions | `rup preview --clipboard` |
| `apply` | Apply edits with backup | `rup apply --clipboard` |
| `backup` | Manage backup sessions | `rup backup list` |
| `chunk` | Split large files by tokens | `rup chunk large_file.rs` |

## Configuration

Create `roughup.toml` in your project root:

```toml
[symbols]
output_file = "symbols.jsonl"
include_private = false
languages = ["rust", "python"]

[chunk]
model = "gpt-4o"
max_tokens = 4000

[context]
default_budget = 6000
default_template = "freeform"

[extract]
default_context = 3
fence = true

[apply]
engine = "internal"
backup = true
```

## Advanced Features

### Multiple Apply Engines
- **Internal**: Fast, clear error messages
- **Git**: Robust 3-way merge, handles conflicts
- **Auto**: Try internal first, fallback to git

### Template System
- **Refactor**: Focus on structure and organization
- **Bugfix**: Emphasize error handling and edge cases  
- **Feature**: Highlight APIs and implementation patterns
- **Freeform**: Balanced general-purpose context

### Smart Ranking
- **Proximity**: Code near your anchor file/line gets priority
- **Relevance**: Exact matches > prefix > substring > fuzzy
- **Importance**: Public APIs, core types, and functions prioritized
- **Recency**: Recently accessed symbols get boosted

## Integration Examples

### With Claude (Web)
```bash
rup context --clipboard --template refactor "MyService"
# Paste into Claude, get suggestions, copy response
rup apply --clipboard
```

### With Local LLMs
```bash
rup context --json "optimize" > context.json
# Send to your local model API
# Get response and save as edits.ebnf
rup apply edits.ebnf
```

### With Custom Scripts
```bash
#!/bin/bash
rup context --json "$1" | my_llm_client | rup apply --clipboard
```

## Performance

- **Context assembly**: <2s for large codebases
- **Symbol indexing**: <5s for 100k+ lines
- **Edit application**: <300ms for typical changes
- **Memory usage**: <50MB for most projects

## Privacy & Security

- **No network calls**: Everything runs locally
- **No telemetry**: Zero data collection
- **Atomic backups**: Safe rollback for any operation
- **Git integration**: Proper attribution and history

## Roadmap

- [ ] **Language Support**: JavaScript/TypeScript, Go, C++, Java
- [ ] **IDE Plugins**: VSCode, Neovim, Emacs extensions
- [ ] **Context Sharing**: Save and share context templates
- [ ] **Parallel Processing**: Multi-threaded symbol extraction
- [ ] **Advanced Templates**: Project-specific context strategies

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

Licensed under [LICENSE](LICENSE) - see the file for details.

## Acknowledgments

Built with:
- [clap](https://github.com/clap-rs/clap) for CLI interface
- [tree-sitter](https://tree-sitter.github.io/) for parsing
- [tiktoken-rs](https://github.com/zurawiki/tiktoken-rs) for token counting
- [rayon](https://github.com/rayon-rs/rayon) for parallelism

---