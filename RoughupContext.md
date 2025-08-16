## Assessment of `rup context` (current state)

- **Baseline symbol lookup works.** Anchoring in `src/core/context.rs` with direct symbol queries returns relevant spans (e.g., `parse_callgraph_arg`, `extract_function_name_at`, helpers).
- **Callgraph requires an index.** `rup context --callgraph …` produced empty results until a symbols index was generated. After `rup symbols --output symbols.jsonl`, the callgraph worked on the test fixture.
- **Anchor accuracy matters.** The fallback anchor logic is correct, but results depend on pointing **inside or very near** a function body. Use `rup extract` to find exact lines before running `--callgraph`.
- **Token discipline.** Tier B pulled \~3k tokens with extra spans; Tier A, tight `--top-per-query` and `--limit`, plus selective queries, kept outputs compact without losing signal.
- **Cross-file edges and score integration** aren’t exercised here; same-file callgraph on the fixture is healthy. For `context.rs`, callgraph produced only core helpers under conservative settings; next steps are precise anchoring and, if needed, increasing depth once the baseline yields neighbors.

---

## Compact starter for new chats: Using `rup context` and `rup extract`

### Minimal workflow

1. **Build symbol index (once per repo or when code changes)**

```bash
rup symbols --output symbols.jsonl
```

2. **Find an exact anchor line** (land inside a function)

```bash
rup extract --annotate --fence --clipboard src/core/context.rs:1375-1465
```

3. **Baseline context (no callgraph, tight caps)**

```bash
rup context --tier A --budget 1200 \
  --anchor src/core/context.rs --anchor-line 1450 \
  --semantic --top-per-query 4 --limit 64 \
  --json --clipboard \
  parse_callgraph_arg extract_function_name_at collect_callgraph_names
```

4. **Add bounded callgraph once baseline returns items**

```bash
rup context --tier A --budget 1200 \
  --anchor src/core/context.rs --anchor-line 1450 \
  --callgraph "depth=1" \
  --symbols symbols.jsonl \
  --top-per-query 4 --limit 64 \
  --json --clipboard \
  extract_function_name_at
```

### Working examples from this session

- **Fixture callgraph (successful)**

```bash
rup context --tier A --budget 1200 \
  --anchor tests/fixtures/callgraph.rs --anchor-line 13 \
  --callgraph "depth=2" \
  --symbols symbols.jsonl \
  --top-per-query 4 --limit 64 \
  --json --clipboard \
  my_method
```

- **Inspect the fixture to choose an anchor**

```bash
rup extract --annotate --fence --clipboard tests/fixtures/callgraph.rs:1-200
```

- **Locate `collect_callgraph_names` and scanner region for anchoring**

```bash
rup extract --annotate --fence --clipboard src/core/context.rs:1540-1620
```

### Zero-items triage (fast checks)

- Did you run `rup symbols` and pass `--symbols symbols.jsonl`?
- Is `--anchor-line` inside a function? If not, adjust using `rup extract`.
- Drop filters first (no `--dedupe` / `--novelty-min`), then re-add after a positive baseline.
- Keep Tier A and small caps (`--top-per-query 4`, `--limit 48–64`) to reduce noise during iteration.
