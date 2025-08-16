here’s a tight but thorough “final deep review + roadmap” you can work from. i split it into: must-fixes, near-term improvements, medium-term roadmap, test plan, and small polish. i also included a couple of surgical code snippets where it really matters.

---

# must-fix before cutting a release

## 1) lock contention logic can return while index is still **stale**

- **where**: `ensure_symbols_with_lock` “couldn’t acquire lock” branch.
- **issue**: you return `Ok(())` as soon as `symbols_path.exists()` — but that may be a **pre-existing stale** index. if a second process grabbed the lock to refresh, the first process should not short-circuit on “exists”.
- **fix**: require **freshness** as the condition to stop waiting; otherwise keep polling until either the lock disappears _and_ the index is fresh, or we time out.

```rust
// in the Err(_) arm of ensure_symbols_with_lock
let start = std::time::Instant::now();
loop {
    std::thread::sleep(std::time::Duration::from_millis(LOCKFILE_POLL_INTERVAL_MS));

    // only accept freshness, not mere existence
    if symbols_path.exists() && Self::index_is_fresh(&args.path, symbols_path) {
        return Ok(());
    }
    if !lock_path.exists() {
        break; // lock is gone, we'll fall through to single retry
    }
    if start.elapsed().as_millis() > LOCKFILE_MAX_WAIT_MS as u128 {
        return Err(anyhow::anyhow!(
            "Symbols generation timeout after {}ms", LOCKFILE_MAX_WAIT_MS
        ));
    }
}
// lock disappeared but index not fresh: retry once
Self::ensure_symbols_with_lock(args, ctx, symbols_path)
```

## 2) symlink detection in freshness scan

- **where**: `is_dir_fresh_recursive`
- **issue**: you call `StdFs::metadata(&path)` and then `file_type().is_symlink()`. `metadata()` follows symlinks, so `is_symlink()` will commonly be **false**. you’ll still descend through symlinked dirs.
- **fix**: use `symlink_metadata()`; keep the skip behavior.

```rust
let metadata = match StdFs::symlink_metadata(&path) {
    Ok(m) => m,
    Err(_) => continue,
};
if metadata.file_type().is_symlink() {
    continue;
}
```

## 3) callgraph “files_per_hop” budget isn’t meaningful yet

- **where**: `collect_callgraph_names_bounded`
- **issue**: traversal enqueues callees but **keeps the same `path`** for every hop. limiting “files per hop” is a no-op if you never change files.
- **short-term fix**: rename the counter to `nodes_seen_this_hop` to match semantics (so it doesn’t mislead).
- **near-term (below)**: actually resolve callee → definition file with `SymbolIndex` to leverage the per-hop file cap.

## 4) JSON “error” payload consistency

- **where**: `output_results`
- **issue**: the success JSON (`--json`) never includes `"ok": true`, while error payloads do include `"ok": false`. make schema consistent so downstream parsers don’t special-case.
- **fix**: add `"ok": true` to the success JSON.

---

# near-term improvements (next iteration)

## A) cross-file callgraph expansion (bounded & deterministic)

- **goal**: make `collect_callgraph_names_bounded` follow symbol **definitions** across files, still cheap & seed-oriented.
- **plan**:

  1. After extracting a candidate `name`, consult `SymbolIndex` for a (name → file, line) (best-effort; prefer exact function names first).
  2. When a unique definition is found, push **that path** into the queue (respect `files_per_hop`).
  3. Keep the current lexical fallback if multiple matches or none found.

- **determinism**: sort candidate definitions by repo-relative path, then by line, then pick first.

## B) caching file contents during piece extraction

- **why**: `piece_from_symbol` can read the same file N times; big repos will thrash disk.
- **how**:

  - group symbols by `s.file`, read once via `read_file_smart`, then slice all pieces from the cached text.
  - keep a small LRU if you want to avoid pre-grouping.

## C) smarter **min_tokens** and item sizing

- **now**: hardcoded `min_tokens: 64`.
- **better**:

  - compute a rough token estimate for each piece (chars/4 is fine).
  - set `min_tokens = clamp(ceil(estimated_tokens * 0.25), 32, 128)` so the budgeter has realistic floor space.

## D) bucket tagging heuristics

- **now**: by extension; previously you had some “test / trait / struct / enum / pub fn” hints.
- **plan**:

  - re-add a lightweight classifier:

    - `*_test.rs`, `mod tests`, `#[cfg(test)]` → `SpanTag::Test`
    - files declaring `trait|enum|struct|pub fn` near the top → `SpanTag::Interface`
    - else → `SpanTag::Code`

  - keep it deterministic and regex-only.

## E) tier-driven defaults without the “compiled default” heuristic

- **issue**: today you infer “user override vs default” by comparing to clap’s compiled default (8/256).
- **plan**:

  - change CLI to `Option<usize>` for `--limit` and `--top-per-query`.
  - if `None` and `tier_opt.is_some()` → pick tier values; if `None` and no tier → global defaults.
  - this removes a brittle coupling to clap defaults.

## F) Windows path polish

- keep `parse_item_id` using `rfind(':')` (good for `C:\`), but also:

  - for display in headers, consider converting to forward slashes for consistency across OSes (purely cosmetic).
  - ensure rank key uses `rel.to_string_lossy()` consistently (already done).

## G) observability

- add `--trace` flag (or read `RUP_TRACE=1`) to log:

  - final effective settings (budget, tier, limits)
  - \#symbols, #pieces, #merged pieces, #items pre/post-fit
  - reasons for bucket refusals (if available from `fit_with_buckets`)

- keep logs on `stderr`.

---

# medium-term roadmap

## 1) pluggable ranking pipeline

- extract three traits:

  - `PieceMerger` (already implied)
  - `PieceRanker` (anchor/scope/path; optional call-distance augmentation)
  - `ItemTagger`

- let CLI pick strategies (e.g., `--rank=anchor-scope-lex`, `--rank=callgraph+anchor`).
- keeps the core deterministic while making future experiments easy.

## 2) novelty/past-context awareness

- you already downrank repeats via `history`; extend with:

  - `--novelty-min` (already on buckets), mirrored in the base fitter (skip items whose Jaccard similarity to history > threshold).
  - optional MRU **per anchor**: keep a per-anchor LRU to avoid re-emitting the same file slices.

## 3) structured JSON schema versioning

- add `"schema": "context-v1"` to JSON envelopes (success & error).
- document fields (`README`), keep the envelope stable.

## 4) concurrency guardrails

- replace the ad-hoc lockfile with **advisory OS file locks** (e.g., `fs2::FileExt::try_lock_exclusive`) if portability is acceptable; fall back to lockfile + timeout where not available.

## 5) advanced callgraph weighting

- precompute `hops` once (you do), then incorporate into rank **before** budgeting as part of the key (small bounded float); today you blend into `Priority.level`. That’s okay, but a pre-rank (stable) pass can reduce budget pressure earlier.

---

# test plan (unit + integration)

## A) unit tests

- **merge_overlaps**

  - touch-only (`end+1==start`) merges with newline behavior
  - multi-segment overlap with empty “non_overlapping”

- **same_file**

  - abs/rel, `..`, symlinks (requires tempdir & symlink if supported)

- **parse_item_id**

  - unix path, windows `C:\…`, weird file names with colons inside (rare but ensure `rfind` logic works)

- **is_dir_fresh_recursive**

  - stale file detected
  - symlinked dirs/files are skipped (use `symlink_metadata`)

- **parse_callgraph_arg**

  - all fields, defaults, clamps (depth to `MAX_CALLGRAPH_DEPTH`, `edges` bounds), fallback anchor

- **callgraph scanners**

  - ignore `if/for/while/match`
  - detect `foo(`, `obj.foo(` (should capture `foo`)

- **fail_signal_boost**

  - overlapping and non-overlapping items; verify bounded multiplier; stable order with equal boosts

## B) integration tests

- **no_symbols** path:

  - missing index, `--json` and tty output paths

- **no_matches** path:

  - valid index, queries miss; JSON and text

- **tier overrides**

  - set tier B; leave `--limit/--top-per-query` unset ⇒ expect tier values
  - set tier B; explicitly pass flags ⇒ expect flags

- **bucketed fit**

  - bucket spec that forces refusals (ensure reporting/tracing if enabled)

- **callgraph augmentation**

  - anchor set, ensure augmented queries appear; bounded by depth/edges/files_per_hop

- **history effect**

  - run twice on same queries; ensure repeats are downranked or de-selected if budget tight

---

# small polish & UX

- **`--json` success envelope**

  - add `"ok": true`

- **`--quiet`**

  - already respected; make sure warnings during auto-index respect quiet except fatal errors.

- **progress bar**

  - when number of queries is large, occasionally show a short “still working” note every N steps in non-quiet mode.

- **help text**

  - document tier presets and how they influence **budget**, **limit**, **top-per-query**.

- **constants to config**

  - allow overriding `DEFAULT_SCAN_WINDOW`, `FUNCTION_SEARCH_WINDOW` via config/env for power users.

---

# optional micro-patches (safe, quick)

1. **cache files in piece extraction**

```rust
let mut cache: std::collections::HashMap<PathBuf, String> = Default::default();
let mut pieces: Vec<Piece> = col.chosen.par_iter().map(|r| {
    let p = &r.symbol.file;
    let abs = if p.is_absolute() { p.clone() } else { env.root.join(p) };
    let text = cache.entry(abs.clone()).or_insert_with(|| read_file_smart(&abs).unwrap_or_default().into_owned());
    // slice by bytes or lines using `text`
    // ...
}).collect::<Result<_>>()?;
```

2. **adaptive `min_tokens`**

```rust
fn rough_tokens(s: &str) -> usize { (s.len() + 3) / 4 }
let est = rough_tokens(&rendered_piece);
let floor = est.saturating_div(4).clamp(32, 128);
Item { min_tokens: floor, .. }
```

3. **bring back simple Test tagging**

```rust
if t.id.contains("_test.rs") || t.content.contains("#[cfg(test)]") || t.content.contains("mod tests") {
    t.tags.insert(SpanTag::Test);
}
```

---

## summary

- Fix the **freshness vs. existence** race in the lock path, and switch to **symlink_metadata** in the freshness walk — those are the two correctness must-dos.
- In the next pass, make callgraph traversal **cross-file** via `SymbolIndex`, cache file reads during extraction, and polish bucket tagging + adaptive token floors.
- Add consistent JSON `"ok": true`, better observability, and a clear test matrix.
