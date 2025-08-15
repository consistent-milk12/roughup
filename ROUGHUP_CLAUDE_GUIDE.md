# Roughup Context Assembly Guide for Web Chat AI

I'm using **Roughup** (`rup`), a privacy-first Rust CLI for LLM-assisted development. This guide focuses on **context assembly** workflow for web chat sessions.

## Session Startup Protocol

**Required Reading Order**:

1. **TODO.md** - Current project priorities and session goals
2. **lib.rs** - Architecture blueprint and module relationships
3. **Context Assembly** - Use this guide for all subsequent context needs

```bash
# Always start here for project understanding
rup context --budget 4000 TODO.md lib.rs --clipboard
```

## Core Context Assembly Patterns

### **Pattern 1: Feature Development Context**

```bash
# Before implementing any new feature
rup context --budget 4000 --template feature "new_feature_name" "related_module" --semantic --clipboard

# Example: Adding conflict resolution
rup context --budget 4000 --template feature "resolve" "conflict" "SmartMerge" --semantic --clipboard
```

### **Pattern 2: Bug Investigation Context**

```bash
# When fixing bugs or understanding error flows
rup context --budget 3000 --template bugfix "error_function" "failure_path" --clipboard

# Example: Fix apply engine issues
rup context --budget 3000 --template bugfix "apply_run" "ApplyEngine" "conflict" --clipboard
```

### **Pattern 3: Refactoring Context**

```bash
# Before refactoring existing code
rup context --budget 5000 --template refactor "target_module" "affected_functions" --semantic --clipboard

# Example: Refactor edit engine
rup context --budget 5000 --template refactor "EditEngine" "apply" "parse_edit_spec" --semantic --clipboard
```

### **Pattern 4: Architecture Understanding**

```bash
# Deep-dive into system architecture
rup context --budget 6000 --template freeform "core_concept" "integration_point" --semantic --clipboard

# Example: Understanding backup system
rup context --budget 6000 --template freeform "BackupManager" "session" "atomic" --semantic --clipboard
```

## Advanced Context Strategies

### **Multi-Round Context Assembly**

```bash
# Round 1: High-level architecture
rup context --budget 2000 "module_name" --semantic --clipboard

# Round 2: Implementation details
rup context --budget 3000 --anchor src/core/target.rs "specific_function" --clipboard

# Round 3: Integration points
rup context --budget 2000 "related_trait" "interface" --semantic --clipboard
```

### **Anchor-Based Proximity**

```bash
# Focus context around specific files/locations
rup context --budget 4000 --anchor src/core/edit.rs --anchor-line 1200 "resolve" "conflict" --clipboard

# Multiple anchors for cross-module context
rup context --budget 5000 --anchor src/cli.rs --anchor src/main.rs "command" "dispatch" --clipboard
```

### **Budget-Optimized Context**

```bash
# Large context for complex tasks
rup context --budget 8000 --template feature "complex_system" --semantic --clipboard

# Focused context for quick fixes
rup context --budget 2000 "specific_function" --clipboard

# Balanced context for typical development
rup context --budget 4000 "target_area" --semantic --clipboard
```

## Context Quality Validation

### **Template Selection Guide**

- **`feature`**: New functionality, additions, enhancements
- **`bugfix`**: Error investigation, fixes, debugging
- **`refactor`**: Code restructuring, optimization, cleanup
- **`freeform`**: Architecture understanding, exploration

### **Semantic Search Benefits**

```bash
# Without semantic: exact string matching only
rup context --budget 4000 "apply" --clipboard

# With semantic: conceptually related code included
rup context --budget 4000 "apply" --semantic --clipboard
```

### **Budget Guidelines**

- **2000-3000**: Quick context, specific functions
- **4000-5000**: Standard development context
- **6000-8000**: Complex features, architecture deep-dives
- **8000+**: Large refactoring, system-wide changes

## Mandatory Development Workflow

1. **Context First**: Always use `rup context` before implementing
2. **Quality Over Speed**: Ask "why" and "where" before coding
3. **Architecture Alignment**: Use lib.rs keywords for context terms
4. **Performance Awareness**: Respect <2s context, <300ms rollback SLAs

```bash
# Example: Before adding any struct/enum/function/trait
rup context --budget 4000 --template feature "similar_pattern" "integration_point" --semantic --clipboard
```

## Web Chat Integration

### **Optimal Session Flow**

1. **Startup Context**:

   ```bash
   rup context --budget 4000 TODO.md lib.rs --clipboard
   ```

2. **Feature Context**:

   ```bash
   rup context --budget 4000 --template feature "target" "related" --semantic --clipboard
   ```

3. **Implementation Context**:

   ```bash
   rup context --budget 3000 --anchor src/target/file.rs "specific_function" --clipboard
   ```

4. **Validation Context**:
   ```bash
   rup context --budget 2000 "test" "validation" --semantic --clipboard
   ```

### **Context Refresh Triggers**

- **Every 5-7 exchanges**: Refresh context for current area
- **New task/feature**: Fresh context assembly with appropriate template
- **Error investigation**: Bugfix template with error-related terms
- **Architecture questions**: Freeform template with system concepts

## Performance & Quality Targets

- **Assembly Time**: <2s typical, <5s for large contexts
- **Deterministic Output**: Same terms = same context across runs
- **Token Accuracy**: Precise budgeting with model-specific encoding
- **Relevance Ranking**: Priority system with proximity/semantic scoring

## Integration with Development

```bash
# Validate context quality during development
rup context --budget 4000 --template feature "implementation_area" --semantic --json | jq '.total_tokens'

# Test different budget allocations
rup context --budget 2000 "target" --clipboard  # Quick context
rup context --budget 6000 "target" --semantic --clipboard  # Deep context
```

---

**Core Principle**: Use `rup context` extensively with proper budgeting and templates. This serves dual purposes: ensures quality implementation aligned with existing architecture AND validates our flagship context assembly functionality during development.

**Remember**: Context assembly is not just for getting code - it's for understanding the "why" and "where" that drives quality-first development.
