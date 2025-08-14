# Mentor AI Consultation Request

## Project Context: Roughup - LLM-Optimized CLI Tool

**Project**: High-performance Rust CLI for LLM workflows with bidirectional code editing
**Current Phase**: Phase 2 - Git Integration & CLI Architecture Completion  
**Status**: 85% complete, need architectural guidance for final integration

## Architecture Overview

Roughup implements a hybrid EBNF→Git architecture:
1. **Human-readable EBNF input** for LLM chat workflows
2. **Dual engine system**: Internal (fast, clear errors) + Git (robust, 3-way merge)
3. **Automatic fallback**: `--engine auto` tries internal first, falls back to git on conflicts

### Core Components (Implemented)
- `src/core/edit.rs` - EBNF parser with GUARD-CID system ✅
- `src/core/patch.rs` - EBNF→unified diff converter ✅  
- `src/core/git.rs` - Git apply integration with 3-way merge ✅
- `src/core/apply_engine.rs` - Unified trait architecture ✅

## Specific Technical Challenge

**Current Issue**: Need to refactor `apply_run()` function in `src/core/edit.rs:744-850` to use the new `ApplyEngine` trait instead of direct `EditEngine` usage.

### Current Implementation (Legacy)
```rust
pub fn apply_run(args: ApplyArgs, ctx: &AppContext) -> Result<()> {
    // ... input parsing ...
    
    let engine = EditEngine::new()
        .with_preview(args.preview)
        .with_backup(args.backup)
        .with_force(args.force);
    
    let result = engine.apply(&spec)?;
    // ... result handling ...
}
```

### Target Implementation (New Architecture)
```rust
pub fn apply_run(args: ApplyArgs, ctx: &AppContext) -> Result<()> {
    // 1. Parse input (same)
    // 2. Create appropriate ApplyEngine via factory function
    // 3. Handle preview vs apply modes
    // 4. Implement safe defaults (preview-first, --apply flag requirement)
    // 5. Add proper exit codes
}
```

## Key Architectural Decisions Needed

### 1. Safe UX Design
**Question**: Should we require explicit `--apply` flag to write changes (defaulting to preview)?
- **Current**: `--preview` flag enables preview mode
- **Proposed**: Default to preview, require `--apply` to write
- **Trade-off**: Safety vs. UX convenience

### 2. Engine Selection Integration
**Context**: CLI already has `--engine internal|git|auto` flags
**Need**: Wire these to `create_engine()` factory function

```rust
// From apply_engine.rs:248-281
pub fn create_engine(
    engine_choice: &EngineChoice,
    git_mode: &GitMode, 
    whitespace: &WhitespaceMode,
    backup_enabled: bool,
    force_mode: bool,
    repo_root: PathBuf,
) -> Result<Box<dyn ApplyEngine>>
```

### 3. Git Repository Detection
**Challenge**: Factory needs `repo_root: PathBuf` parameter
**Options**:
- A) Auto-detect git root from current directory
- B) Require explicit `--repo-root` flag
- C) Use current directory, validate later

### 4. Exit Code Strategy
**Requirement**: Standardized exit codes for CI/automation
```
0 = success
2 = conflicts detected
3 = invalid input/syntax
4 = repository issues
5 = internal engine errors
```

### 5. Preview vs Apply Mode Unification
**Current**: Separate `preview_run()` and `apply_run()` functions
**Question**: Should these be unified since ApplyEngine trait has both `check()` and `apply()`?

## Code Context References

### ApplyEngine Trait Interface
```rust
pub trait ApplyEngine {
    fn check(&self, spec: &EditSpec) -> Result<Preview>;
    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport>;
}
```

### CLI Arguments Structure
```rust
pub struct ApplyArgs {
    pub edit_file: Option<PathBuf>,
    pub from_clipboard: bool,
    pub preview: bool,
    pub backup: bool, 
    pub force: bool,
    pub verbose: bool,
    pub engine: ApplyEngine,           // internal|git|auto
    pub git_mode: GitMode,             // 3way|index|worktree
    pub context_lines: usize,
    pub whitespace: WhitespaceMode,    // nowarn|warn|fix
}
```

## Specific Questions for Mentor AI

### 1. Architecture Validation
Is the hybrid trait-based design sound for production use? Any anti-patterns or improvements?

### 2. Safe Defaults Implementation
Best practice for "preview-first" UX in CLI tools? Should `--apply` flag be required?

### 3. Error Handling Strategy
How to best propagate errors from different engines while maintaining user-friendly messages?

### 4. Repository Root Detection
Most robust approach for git repository detection in CLI context?

### 5. Function Refactoring Pattern
Clean way to migrate from direct engine usage to trait-based factory pattern?

## Implementation Constraints

1. **Backward Compatibility**: Existing CLI interface must remain unchanged
2. **Performance**: Internal engine should remain fast path for simple cases  
3. **Safety**: Must prevent data loss, validate before writing
4. **Cross-Platform**: Windows + Unix support required
5. **Error Messages**: Git errors must be mapped to user-friendly messages

## Expected Mentor Response Format

Please provide:
1. **Architecture Assessment** - Design validation and suggestions
2. **Specific Implementation Guidance** - Code patterns and best practices
3. **Safe UX Recommendations** - Preview-first vs. explicit apply flag trade-offs
4. **Error Handling Patterns** - Trait-based error propagation strategies
5. **Repository Detection Logic** - Robust git root finding implementation
6. **Refactoring Roadmap** - Step-by-step migration plan for apply_run()

## Project Goals Alignment

This refactoring completes Phase 2 of our roadmap, enabling:
- Production-ready edit system with enterprise robustness
- Git's 3-way merge handling code drift that breaks line targeting  
- Professional patch preview in all engine modes
- Automatic fallback for maximum reliability

The end goal is a zero-setup tool that works out of the box for LLM-assisted development workflows.