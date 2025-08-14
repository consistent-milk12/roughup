# Roughup CLI Guide for Claude Web Chat Generated Using Claude Code

I'm using **Roughup** (`rup`), a privacy-first Rust CLI tool for LLM-assisted code editing. All operations are local-only with atomic backups. Here's how to use it throughout our session:

## Core Workflow Commands

### 1. **Project Structure** (`rup tree --clipboard`)

```bash
rup tree --clipboard
```

- Copies repository structure to clipboard
- Shows file organization and hierarchy
- Use when I need to understand project layout

### 2. **Smart Context Assembly** (`rup context --clipboard`)

```bash
# Basic context extraction
rup context --clipboard "function_name"
rup context --clipboard "ClassName" "method_name"

# With semantic search and template
rup context --clipboard --semantic --template refactor "function_name"
rup context --clipboard --template bugfix "error_handling"
rup context --clipboard --template feature "new_component"

# Budget-aware (default 6000 tokens)
rup context --clipboard --budget 8000 "complex_system"
```

- Extracts relevant code with token budgeting
- Supports exact, fuzzy, and semantic matching
- Templates: `refactor`, `bugfix`, `feature`, `freeform`
- Automatically copies to clipboard for pasting here

### 3. **File Content Extraction** (`rup extract --clipboard`)

```bash
# Extract specific files
rup extract --clipboard src/main.rs src/lib.rs

# Extract with line ranges
rup extract --clipboard src/core/edit.rs:100-200

# Extract multiple ranges from same file
rup extract --clipboard src/parser.rs:1-50,150-200
```

- Extracts specific files or line ranges
- Preserves line numbers for accurate editing
- Use when you need specific file content

### 4. **Edit Preview** (`rup preview`)

```bash
# Preview from clipboard (your EBNF edit spec)
rup preview --clipboard

# Preview from file
rup preview edit_spec.ebnf
```

- Shows exactly what changes will be made
- Validates EBNF syntax before applying
- No modifications until `apply`

### 5. **Apply Changes** (`rup apply --clipboard`)

```bash
# Apply from clipboard (your EBNF edit spec)
rup apply --clipboard

# Safe apply with preview confirmation
rup apply --clipboard --interactive

# Apply from file
rup apply edit_spec.ebnf
```

- Atomic edits with automatic backups
- Validates all operations before applying
- Creates backup session for rollback

## EBNF Edit Format (What You Generate)

When providing edits, use this exact format:

````ebnf
FILE: path/to/file.rs
REPLACE lines 10-15:
OLD:
```rust
fn old_function() {
    println!("old code");
}
````

NEW:

```rust
fn new_function() {
    println!("new code");
}
```

FILE: path/to/another.rs
INSERT at 25:
NEW:

```rust
// New code block
fn helper() {}
```

FILE: path/to/third.rs
DELETE lines 5-8:
OLD:

```rust
// Code to remove
let unused = true;
```

````

## Backup Management

```bash
# List backup sessions
rup backup list

# Show specific backup details
rup backup show <session-id>

# Restore from backup
rup backup restore <session-id>
````

## Advanced Context Options

```bash
# Anchor-based proximity ranking
rup context --clipboard --anchor src/core/main.rs --anchor-line 100 "function"

# Limit candidates and output
rup context --clipboard --top-per-query 5 --limit 50 "search_term"

# JSON output for structured data
rup context --json "function" > context.json
```

## Session Workflow

1. **Start with structure**: `rup tree --clipboard` → paste here
2. **Get context**: `rup context --clipboard --template [type] "targets"` → paste here
3. **You provide EBNF edits** → I copy to clipboard
4. **Preview changes**: `rup preview --clipboard` → verify output
5. **Apply safely**: `rup apply --clipboard`
6. **Repeat as needed** with new context extractions

## Key Benefits

- **Privacy**: All processing local, no network calls
- **Safety**: Atomic operations with automatic backups
- **Speed**: <2s context assembly, <300ms operations
- **Deterministic**: Same input = same output across sessions
- **Smart**: Symbol indexing, relevance ranking, token budgeting

## Error Recovery

- All operations create backups automatically
- Use `rup backup list` to see available restore points
- `rup backup restore <id>` for instant rollback
- Edit conflicts show clear resolution paths

## Performance Notes

- Run `rup symbols` once per session to build index
- Context assembly: <2s typical, <5s for large codebases
- Token estimation includes all major models (GPT-4, Claude, etc.)
- Budget defaults to 6000 tokens (configurable)

---

**Usage Pattern**: Use clipboard commands throughout our session. I'll provide the project context you need, you'll give me EBNF edits, and I'll apply them safely with atomic backups.
