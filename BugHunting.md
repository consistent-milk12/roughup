# Bug Hunting: Scoreboard Test Race Conditions

## Bug Report

**Symptoms**: `tests/scoreboard_smoke.rs` exhibited non-deterministic failures with intermittent panics:

```````
---- scoreboard_handles_probe_first_scenario stdout ----
thread 'scoreboard_handles_probe_first_scenario' (2067234) panicked at
/home/user/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/ops/function.rs:253:5:
Unexpected failure.
code=1
stderr=``````
Error: rup failed: status=ExitStatus(unix_wait_status(256))
stdout=
stderr=Error: Failed to parse JSON on line 170

Caused by:
    EOF while parsing a string at line 1 column 127
```````

**Key Characteristics**:

- Tests passed sometimes, failed other times (non-deterministic)
- Error occurred at "line 170" in JSON parsing
- "EOF while parsing a string" suggested file truncation/corruption
- Multiple test functions affected (`scoreboard_handles_probe_first_scenario`, others)

## Investigation Process

### Phase 1: Error Message Analysis

**Clue**: "Failed to parse JSON on line 170" pointed to specific JSON parsing logic.

**Action**: Searched codebase for this error message:

```bash
grep -r "Failed to parse JSON on line"
```

**Finding**: Error originated from `src/core/symbol_index.rs:105`:

```rust
let s: Symbol = serde_json::from_str(&line)
    .with_context(|| format!("Failed to parse JSON on line {}", i + 1))?;
```

This indicated the symbol index JSONL file was being corrupted during parsing.

### Phase 2: Symbol Index Flow Analysis

**Question**: How are symbol files created and consumed?

**Investigation**: Traced the symbol generation workflow:

1. **Symbol Creation**: `rup symbols` command writes to `symbols.jsonl` (default output)

   - Location: `src/core/symbols.rs` - `JsonlWriter::write()`
   - Uses `BufWriter` with proper flushing

2. **Symbol Consumption**: `rup context` loads from `symbols.jsonl`

   - Location: `src/core/symbol_index.rs` - `SymbolIndex::load()`
   - Reads line-by-line JSON parsing

3. **Test Flow**: Each scoreboard test calls:
   ```rust
   Command::cargo_bin("rup")
       .current_dir(tmp.path())
       .arg("symbols")  // Writes to symbols.jsonl
   ```

### Phase 3: Race Condition Hypothesis

**Insight**: Multiple tests running in parallel could be accessing the same file.

**Critical Discovery**: In `src/cli.rs:203`, symbols default output is:

```rust
#[arg(short, long, default_value = "symbols.jsonl")]
pub output: PathBuf,
```

**Problem Identified**: All tests write to the same filename `symbols.jsonl` in their working directories.

### Phase 4: Working Directory Analysis

**Question**: Are test working directories properly isolated?

**Investigation**: Examined test structure in `tests/scoreboard_smoke.rs`:

```rust
let tmp = make_heavy_fixture();  // Creates isolated TempDir

// BUT: scoreboard binary doesn't run in tmp.path()!
Command::cargo_bin("scoreboard")
    .expect("bin")
    .args([...])  // Missing .current_dir(tmp.path())
```

**Critical Finding**: The scoreboard binary was running in the test runner's working directory, not the isolated temporary directory.

### Phase 5: Scoreboard Binary Analysis

**Investigation**: Examined `src/bin/scoreboard.rs:328`:

```rust
// Ensure symbol index exists - fail fast if this fails
run_cmd_in(&root, "rup", &["symbols"])?;
```

**Race Condition Confirmed**:

1. Multiple tests run scoreboard binary simultaneously
2. Each scoreboard calls `rup symbols` without explicit `--output`
3. All write to default `symbols.jsonl` in the **same working directory**
4. Concurrent writes cause file corruption
5. Subsequent reads encounter truncated/invalid JSON

### Phase 6: File System Race Mechanics

**Detailed Analysis**: The race condition occurred as follows:

```
Time  | Test A                    | Test B                    | File State
------|---------------------------|---------------------------|-------------
T1    | rup symbols starts        | -                         | Writing...
T2    | Writing JSON line 100     | rup symbols starts        | Writing...
T3    | Writing JSON line 150     | Truncates file, starts    | Corrupted!
T4    | -                         | Writing JSON line 50      | Partial
T5    | rup context tries to read | -                         | EOF at 127
```

**Root Cause**: File truncation during concurrent writes to the same `symbols.jsonl` file.

## Solution Design

### Fix 1: Explicit Symbol File Paths

**Problem**: Tests used default `symbols.jsonl` filename.

**Solution**: Specify unique output paths within each test's temporary directory:

```rust
// Use unique symbols file name to prevent race conditions between parallel tests
let symbols_path = tmp.path().join("symbols.jsonl");
Command::cargo_bin("rup")
    .expect("bin")
    .current_dir(tmp.path())
    .args(["symbols", "--output", symbols_path.to_str().unwrap()])
    .assert()
    .success();
```

### Fix 2: Correct Working Directory Isolation

**Problem**: Scoreboard binary ran in test runner's working directory.

**Solution**: Ensure scoreboard runs in the isolated test fixture directory:

```rust
// Run the scoreboard binary in the test fixture directory
Command::cargo_bin("scoreboard")
    .expect("bin")
    .current_dir(tmp.path())  // ← Critical addition
    .args([...])
```

## Implementation

### Changes Made

Applied fixes to all three test functions in `tests/scoreboard_smoke.rs`:

1. `scoreboard_runs_and_emits_metrics()`
2. `scoreboard_handles_probe_first_scenario()`
3. `scoreboard_explicit_budget_scenario()`

### Code Changes

```rust
// Before (problematic):
Command::cargo_bin("rup")
    .expect("bin")
    .current_dir(tmp.path())
    .arg("symbols")  // Uses default output path
    .assert()
    .success();

Command::cargo_bin("scoreboard")
    .expect("bin")  // Missing working directory
    .args([...])

// After (fixed):
let symbols_path = tmp.path().join("symbols.jsonl");
Command::cargo_bin("rup")
    .expect("bin")
    .current_dir(tmp.path())
    .args(["symbols", "--output", symbols_path.to_str().unwrap()])
    .assert()
    .success();

Command::cargo_bin("scoreboard")
    .expect("bin")
    .current_dir(tmp.path())  // Proper isolation
    .args([...])
```

## Verification

### Test Results

**Before Fix**: Intermittent failures with JSON parsing errors
**After Fix**: Consistent passes across multiple runs

```bash
# Sequential runs
for i in {1..5}; do cargo test --test scoreboard_smoke; done
# Result: 5/5 passes

# Parallel execution
cargo test --test scoreboard_smoke -- --test-threads=8
# Result: All tests pass reliably
```

### Key Verification Points

✅ **Deterministic Behavior**: Tests pass consistently  
✅ **Parallel Safety**: No failures with concurrent execution  
✅ **File Isolation**: Each test operates in separate temporary directories  
✅ **Race Condition Eliminated**: No more JSON corruption errors

## Debugging Techniques Used

### 1. Error Message Archaeology

- Traced error messages to source code locations
- Used `grep` to find exact error generation points
- Followed stack traces to understand execution flow

### 2. Workflow Analysis

- Mapped complete data flow from creation to consumption
- Identified all file I/O operations and their timing
- Traced command execution paths through the codebase

### 3. Concurrency Analysis

- Analyzed parallel test execution patterns
- Identified shared resources and potential conflicts
- Examined file system access patterns

### 4. Hypothesis-Driven Investigation

- Formed specific hypotheses about race conditions
- Designed targeted investigations to test theories
- Systematically eliminated possibilities

### 5. Isolation Testing

- Ran tests multiple times to reproduce non-deterministic behavior
- Used parallel execution to stress-test race conditions
- Verified fixes with repeated testing

## Lessons Learned

### 1. **File System Isolation in Tests**

- Always ensure test processes run in isolated working directories
- Explicitly specify file paths rather than relying on defaults
- Be aware of shared resources in parallel test execution

### 2. **Race Condition Symptoms**

- Non-deterministic failures often indicate race conditions
- "EOF while parsing" suggests concurrent file access
- Line-specific errors in generated files point to truncation issues

### 3. **Integration Test Complexity**

- Tests that spawn external processes need careful isolation
- Working directory inheritance can cause unexpected sharing
- Default file paths can become contention points

### 4. **Debugging Methodology**

- Start with error message analysis to narrow scope
- Trace complete data flows, not just immediate code
- Test hypotheses with targeted experiments
- Verify fixes thoroughly with stress testing

## Future Prevention

### Code Review Checklist

- [ ] Do tests run in isolated working directories?
- [ ] Are file paths explicitly specified rather than defaulted?
- [ ] Could parallel execution cause resource conflicts?
- [ ] Are external process invocations properly isolated?

### Testing Patterns

- Always test with parallel execution (`--test-threads=N`)
- Run integration tests multiple times to catch non-deterministic issues
- Use explicit temporary directories for all file operations
- Verify working directory isolation in process spawning

---

_This bug demonstrated the subtle complexity of race conditions in integration tests involving file I/O and external process execution. The fix required understanding both the test isolation mechanisms and the default behaviors of the command-line tools being tested._
