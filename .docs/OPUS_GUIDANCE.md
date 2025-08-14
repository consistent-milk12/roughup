Here’s a tight but deep pass on what you’ve built so far, plus concrete upgrades and shellable extraction commands to interrogate the repo.

# 1) Architecture assessment

- The trait split is solid. You’ve cleanly separated preview/apply behind `ApplyEngine` with `InternalEngine`, `GitEngineWrapper`, and `HybridEngine` (auto) orchestrating fallback. This is a production-ready direction and keeps the “fast path” internal while enabling 3-way resilience via Git. &#x20;
- `apply_run()` is already refactored to the new model: preview-first by default, repo auto-discovery, and engine factory use. Nice. &#x20;
- Exit code mapping is centralized and used at the CLI boundary: predictable for CI. &#x20;
- Parser is robust (fenced blocks with variable backticks, explicit unterminated-fence check). Good. &#x20;
- Internal editor preserves CRLF/EOF newline and performs atomic writes via a temp file—great for cross-platform safety. &#x20;

🔧 Improvement opportunities

- **Auto engine when no repo**: Right now `--engine auto` errors if no Git repo is present; it should degrade to internal-only instead of failing. The check treats Auto the same as Git.&#x20;
- **Unify preview UX**: `preview_run()` still bypasses the new engines and doesn’t render unified diffs (TODO left). Consider routing `preview` subcommand through the new path.&#x20;
- **Context lines & whitespace**: CLI `context_lines` isn’t threaded into `PatchConfig`/git apply; defaults of `3` are hard-coded. Wire the user’s value through. &#x20;
- **Conflict formatting**: You’re collecting `Vec<String>` (Debug) for conflicts. Prefer the structured summary you already wrote in git: `render_conflict_summary`. &#x20;

# 2) Specific implementation guidance (drop-in patterns)

## A. Make `--engine auto` work without a repo

Replace the “need repo” gating to allow `Auto` when no repo is found; just construct an `InternalEngine` in that case.

```rust
// before
let need_repo = matches!(args.engine,
    crate::cli::ApplyEngine::Git | crate::cli::ApplyEngine::Auto);
if need_repo && repo_root.is_none() { /* error */ }

// after
let engine: Box<dyn ApplyEngine> = match (&args.engine, repo_root.clone()) {
    (crate::cli::ApplyEngine::Git, None) => {
        return Err(ApplyCliError::Repo(
            "Git engine requires a repository. Use --engine=internal or init a repo.".into()
        ).into());
    }
    (crate::cli::ApplyEngine::Auto, None) => {
        // degrade gracefully to internal-only auto
        create_engine(
            &crate::cli::ApplyEngine::Internal,
            &args.git_mode, &args.whitespace,
            args.backup, args.force, cwd.clone(),
        ).map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {e}")))?
    }
    _ => create_engine(
        &args.engine, &args.git_mode, &args.whitespace,
        args.backup, args.force, repo_root.unwrap_or_else(|| cwd.clone()),
    ).map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {e}")))?,
};
```

(References for current behavior and factory: )

## B. Thread `context_lines` everywhere

Use the CLI’s `context_lines` to parameterize both patch generation and Git.

- In `create_engine`, pass `context_lines: args.context_lines as u8` into `GitOptions`. (Currently 3).&#x20;
- In both `InternalEngine` and `GitEngineWrapper` preview/apply paths, use a `PatchConfig { context_lines: args.context_lines, .. }` instead of `PatchConfig::default()`. (Right now both hard-code default). &#x20;
- Optional: for Git, include `-U <n>` to honor user’s context at apply time.

## C. Route `preview_run` through the new engine

`preview_run()` can reuse the same parsing + `engine.check()` pipeline so users see the exact unified diff they’ll get at apply time.

```rust
pub fn preview_run(args: PreviewArgs, ctx: &AppContext) -> Result<()> {
    let input = if args.from_clipboard { get_clipboard_content()? }
        else if let Some(file_path) = args.edit_file {
            fs::read_to_string(&file_path)
                .with_context(|| format!("Failed to read edit file: {:?}", file_path))?
        } else { anyhow::bail!("Must specify --from-clipboard or a file") };

    let spec = EditEngine::new().parse_edit_spec(&input)
        .context("Failed to parse edit specification")?;

    let cwd = std::env::current_dir().context("cwd")?;
    let repo_root = discover_repo_root(args.repo_root.clone(), &cwd)?;
    let engine = create_engine(&args.engine, &args.git_mode, &args.whitespace,
                               /*backup*/ false, /*force*/ args.force,
                               repo_root.unwrap_or_else(|| cwd))?;

    let preview = engine.check(&spec).map_err(normalize_err)?;
    if !ctx.quiet {
        if !preview.patch_content.is_empty() { println!("{}", preview.patch_content); }
        println!("{}", preview.summary);
        if !preview.conflicts.is_empty() {
            eprintln!("{}", crate::core::git::render_conflict_summary(
                &[] /* translate preview.conflicts into GitConflict if available */
            ));
        }
    }
    Ok(())
}
```

(Shows where the current preview path diverges: )

## D. Improve conflict messaging in `apply_run`

After `engine.check()`, use your formatter for a single, readable block.

```rust
if !preview.conflicts.is_empty() && !ctx.quiet {
    // If the engine is Git or Auto, you can render structured hints:
    println!("{}", crate::core::git::render_conflict_summary(
        /* map preview.conflicts -> GitConflict if using git */
    ));
}
```

(Formatter exists here: )

## E. Safer backup naming

You already switched to timestamped names and atomic replacement via tempfiles—great. Keep that; it’s more robust than plain `rename` when overwriting on Windows. &#x20;

# 3) Safe UX recommendations

- **Preview-first, require `--apply` to write**: You’re already doing this. Keep it—it’s the right default for LLM-generated edits. Also print a single clear hint when falling back to preview (you do).&#x20;
- **Optional confirmation**: In interactive TTY and `--apply` without `--yes`, consider prompting “Apply N changes to M files? \[y/N]”. For CI, skip prompt if `!stdin_isatty()` or `RUP_ASSUME_YES=1`.
- **Unified diff consistency**: Always show the same diff in preview that Git will attempt to apply (thread `context_lines`, whitespace policy).

# 4) Error handling patterns

- **Typed errors over string-matching**: `normalize_err` buckets by substring; replace with typed error variants emitted by engines (e.g., `EngineError::Conflicts`, `EngineError::Repo`). Then map 1:1 into `ApplyCliError`. For Git, convert parsed `GitConflict` into `ApplyCliError::Conflicts` with a readable summary. (Current string matching: )
- **Exit codes**: You’ve centralized mapping (`exit_code_for`) and a single `finish_with_exit()` call site—keep this pattern. &#x20;

# 5) Repo detection logic

Your `discover_repo_root()` is pragmatic and cross-platform enough: explicit > `git rev-parse --show-toplevel` > upward search for `.git/`. Keep it. Add a fast path for worktrees (they still appear as a `.git` file pointing at `worktrees/<name>`): treat a **file** named `.git` with `gitdir: ...` as valid too. (Current logic stops at directory `.git` only.)&#x20;

# 6) Refactoring roadmap (apply_run & friends)

1. **Finish the CLI surface**

   - Ensure `ApplyArgs` has `apply: bool` (your code uses it) while keeping existing `--preview` for back-compat; interpreting neither flag as preview is fine since you already default to preview. (You’re already warning once.)&#x20;

2. **Thread user knobs end-to-end**

   - Wire `context_lines` + whitespace policy to both patch gen and Git apply. (Defaults currently hard-coded.) &#x20;

3. **Preview unification**

   - Move `preview_run()` to reuse `engine.check()` + unified diff rendering. (Currently legacy path.)&#x20;

4. **Auto-engine robustness**

   - Allow `Auto` without a repo (degrade to internal). (Current behavior is to error.)&#x20;

5. **Conflicts UX**

   - Swap `Vec<String>` conflict lists for your structured `render_conflict_summary`. (Today you dump `Debug` strings.) &#x20;

6. **Tests**

   - Add end-to-end tests: synthesize a tiny repo, generate a patch via `PatchConfig{ context_lines: N }`, run `git --version` gating, then `engine.apply()` and assert working tree & index shape.

### C) Optional: minimal reproducible e2e test scaffolding

```bash
# Create a temp repo, apply a tiny patch, show index/worktree state
bash -euxo pipefail <<'EOF'
tmp=$(mktemp -d); cd "$tmp"; git init -q
printf "a\nb\nc\n" > t.txt; git add t.txt; git commit -m init -q
cat > edit.rup <<'RUP'
FILE: t.txt
REPLACE lines 2-2:
OLD:
```

b

```
NEW:
```

bee

```
RUP
# Wire your compiled binary path here:
# ./roughup apply --edit-file edit.rup --engine git --apply --git-mode 3way
EOF
```

---

## A few small nits spotted

- In `operation_to_hunk` (Replace), `old_count` calculation uses a `min` with a derived value—double-check edge cases near file ends; consider direct `(context_end - context_start + 1)`.&#x20;
- `apply_run` prints conflicts before returning `Conflicts` (good), but when using Git engines you can include file-specific hints via `render_conflict_summary` for a better DX. &#x20;

---

### Verdict

You’re \~there. The architecture is sound; the safety defaults are correct; and the Git integration is thoughtfully wrapped. If you: (1) allow Auto without a repo, (2) thread `context_lines` end-to-end, (3) unify the preview path, and (4) lean on your structured conflict summaries, you’ll have a genuinely production-grade Phase 2.
