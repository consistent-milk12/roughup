//! Context assembly command (Phase 3/3.5)
//!
//! Deterministic, privacy-first extraction of paste-ready context with
//! anchor-aware ranking (anchor file → same dir → others) and token
//! budgeting.
//!
//! This rewrite fixes ordering issues exposed by tests:
//! - test_proximity_scope_influence_on_order
//! - test_scope_bonus_applies_to_file_level_slices
//!
//! Key changes:
//! - Rank AFTER overlap-merge using repo-relative path logic.
//! - Anchor file gets highest priority; same-directory files get scope bonus; remaining
//!   files follow lexicographic path order.
//! - Anchor equality and scope checks are robust to abs/rel path mismatches.

use std::collections::HashSet; // history set
use std::fs; // file IO
use std::path::{Path, PathBuf}; // paths
use std::{borrow::Cow, collections::BTreeSet}; // path views
use std::{cmp::Reverse, collections::VecDeque}; // sort keys

use anyhow::{Context, Result, bail}; // error context
use indicatif::{ProgressBar, ProgressStyle}; // CLI progress
use rayon::prelude::*; // parallel map
use serde::Serialize; // JSON structs

use crate::core::budgeter::{
    Budgeter,
    Item, // token fit
    Priority,
    SpanTag,           // item classification
    TaggedItem,        // enhanced items
    fit_with_buckets,  // bucket orchestrator
    parse_bucket_caps, // CLI parsing
};
use crate::core::symbol_index::{
    LookupOptions, // search
    RankedSymbol,
    SymbolIndex,
};
use crate::core::symbols::Symbol; // symbol def
use crate::infra::io::read_file_smart;
use crate::{
    cli::{
        AppContext,
        ContextArgs, // CLI types
        ContextTemplate,
        TemplateArg,
        TierArg, // tier presets
    },
    infra::config,
}; // fast reads

/// Internal tier representation with helper methods
/// Maps presets to concrete numeric defaults without leaking policy
#[derive(Clone, Copy, Debug)]
enum Tier
{
    A, // Small preset (tight context)
    B, // Medium preset (balanced)
    C, // Large preset (broader sweep)
}

/// Conversion from CLI-layer TierArg to core Tier
impl From<TierArg> for Tier
{
    fn from(t: TierArg) -> Self
    {
        match t
        {
            TierArg::A => Tier::A,
            TierArg::B => Tier::B,
            TierArg::C => Tier::C,
        }
    }
}

/// Implement preset policies in one place for testability
impl Tier
{
    /// Return the default token budget for this tier
    fn budget(self) -> usize
    {
        match self
        {
            Tier::A => 1200, // ~Tier A target
            Tier::B => 3000, // ~Tier B target
            Tier::C => 6000, // ~Tier C target
        }
    }

    /// Return a sensible overall candidate limit for this tier
    fn limit(self) -> usize
    {
        match self
        {
            Tier::A => 96,  // tighter to reinforce small manifests
            Tier::B => 192, // medium breadth
            Tier::C => 256, // original default ceiling
        }
    }

    /// Return a per-query cap to avoid early explosion
    fn top_per_query(self) -> usize
    {
        match self
        {
            Tier::A => 6,  // smaller per query
            Tier::B => 8,  // original default
            Tier::C => 12, // allow broader intake
        }
    }
}

/// Run the `context` command end-to-end
pub fn run(
    args: ContextArgs,
    ctx: &AppContext,
) -> Result<()>
{
    // 1) Load persisted config (best-effort; defaults if missing)
    let cfg = config::load_config().unwrap_or_default();

    // Resolve repository root passed to the command
    let root = args
        .path
        .clone();

    // Determine the symbols index path (CLI wins, else config)
    let symbols_path = if args
        .symbols
        .exists()
    {
        args.symbols
            .clone()
    }
    else
    {
        cfg.symbols
            .output_file
            .into()
    };

    // Resolve model used by the budgeter (CLI wins, else config) - needed for JSON fallback
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| {
            cfg.chunk
                .model
                .clone()
        });

    // Resolve the optional tier declared by the user
    let tier_opt: Option<Tier> = args
        .tier
        .clone()
        .map(Into::into);

    // Compute an effective budget:
    // 1) User --budget wins if present
    // 2) Else tier preset if provided
    // 3) Else previous default (6000)
    let budget = if let Some(b) = args.budget
    {
        b
    }
    else if let Some(tier) = tier_opt
    {
        tier.budget()
    }
    else
    {
        6000
    };

    // Prepare effective caps derived from tier unless the user
    // explicitly provided overrides. We detect an override by
    // checking whether the value is different from the compiled
    // default that Clap attaches (8 and 256 respectively).
    let compiled_default_top_per_query: usize = 8;
    let compiled_default_limit: usize = 256;

    // Compute effective top-per-query honoring explicit override
    let effective_top_per_query = if let Some(tier) = tier_opt
    {
        if args.top_per_query == compiled_default_top_per_query
        {
            tier.top_per_query()
        }
        else
        {
            args.top_per_query
        }
    }
    else
    {
        args.top_per_query
    };

    // Compute effective overall limit honoring explicit override
    let effective_limit = if let Some(tier) = tier_opt
    {
        if args.limit == compiled_default_limit
        {
            tier.limit()
        }
        else
        {
            args.limit
        }
    }
    else
    {
        args.limit
    };

    // Template for context factors (if needed in the future)
    let _template_for_factors = extract_context_template(&args.template);

    // Load query history if present (best-effort; no failure)
    let history = load_history(root.join(".roughup_context_history"));

    // Auto-index if missing and not explicitly disabled
    let no_auto = std::env::var("ROUGHUP_NO_AUTO_INDEX").is_ok();

    if !Path::new(&symbols_path).exists() && !no_auto
    {
        // Best-effort parent dir create (no-op if already there)
        if let Some(parent) = symbols_path.parent()
            && !parent
                .as_os_str()
                .is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        if !ctx.quiet
        {
            eprintln!(
                "(info) symbols index missing; generating at {}",
                symbols_path.display()
            );
        }
        // Build args from config + current path
        let sym_args = crate::cli::SymbolsArgs {
            path: args
                .path
                .clone(),
            languages: cfg
                .symbols
                .languages
                .clone(),
            output: symbols_path.clone(),
            include_private: cfg
                .symbols
                .include_private,
        };
        // Run in-process to avoid PATH/shell issues; force quiet to preserve stdout determinism
        let mut quiet_ctx = ctx.clone();
        quiet_ctx.quiet = true;
        crate::core::symbols::run(sym_args, &quiet_ctx)?;
    }

    // If still missing (disabled or failed), handle gracefully
    if !Path::new(&symbols_path).exists()
    {
        if args.json
        {
            let tier_label = tier_opt.map(|t| {
                match t
                {
                    Tier::A => "A",
                    Tier::B => "B",
                    Tier::C => "C",
                }
            });
            let out = serde_json::json!({
                "model": model,
                "budget": budget,
                "total_tokens": 0,
                "tier": tier_label,
                "effective_limit": effective_limit,
                "effective_top_per_query": effective_top_per_query,
                "items": [],
                "ok": false,
                "reason": "no_symbols"
            });

            println!("{}", out);

            return Ok(());
        }
        else
        {
            bail!(
                "Symbols file not found: {}. Run 'rup symbols' first (or enable auto-index).",
                symbols_path.display()
            );
        }
    }

    // Materialize a SymbolIndex view over symbols.jsonl
    let index = SymbolIndex::load(&symbols_path)?;

    // 1) Collect fail signals if provided; fail closed = ignore on error
    let mut fail_signals: Vec<crate::core::fail_signal::FailSignal> = Vec::new();
    if let Some(path) = args
        .fail_signal
        .as_ref()
    {
        match fs::read_to_string(path)
        {
            Ok(text) =>
            {
                // Auto-detect format via available parsers; fall back to rustc
                // We avoid unwrap/panic: empty on detection failure.
                let parsed = autodetect_and_parse(&text);
                if !parsed.is_empty()
                {
                    fail_signals = parsed;
                }
            }
            Err(_e) =>
            {
                // Graceful degrade: no-op when unreadable
            }
        }
    }

    // Convert history list into a set for O(1) checks
    let hist_set = history
        .as_ref()
        .map(|v| {
            v.iter()
                .cloned()
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    // Capture anchor hints from CLI (file + optional line)
    let anchor_file = args
        .anchor
        .as_deref();
    let anchor_line = args.anchor_line;

    // Build lookup options to drive symbol search
    let opts = LookupOptions {
        // Pass through semantic toggle from CLI
        semantic: args.semantic,
        // Propagate anchor file for proximity scoring
        anchor_file,
        // Propagate anchor line to fine-tune proximity
        anchor_line,
        // Provide history set for downranking repeats
        history: Some(&hist_set),
        // Use the computed effective limit (tier-aware unless overridden)
        limit: effective_limit,
        // Keep kind filter unchanged
        kinds: None,
    };

    // Configure progress UI (hidden in --quiet mode)
    let _pb = if ctx.quiet
    {
        ProgressBar::hidden()
    }
    else
    {
        let pb = ProgressBar::new(
            args.queries
                .len() as u64,
        );
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap(),
        );
        pb
    };

    // --- PATCH: Augment queries with trait-resolve and callgraph (deterministic) ---
    let mut effective_queries: Vec<String> = args
        .queries
        .clone();
    // Trait/impl surface
    if let Some(q) = args
        .trait_resolve
        .as_ref()
        && let Some((ty, method)) = parse_trait_resolve(q)
    {
        effective_queries.push(format!("trait {}", ty));
        effective_queries.push(format!("impl {} for", ty));
        effective_queries.push(format!("{}::{}", ty, method));
    }

    // Callgraph neighbors
    if let Some(spec) = parse_callgraph_arg(
        args.callgraph
            .as_deref(),
        args.anchor
            .as_ref(),
        args.anchor_line,
    ) && let Some((apath, aline)) = spec.anchor
        && let Some(func) = extract_function_name_at(&root, &apath, aline)
    {
        let names = collect_callgraph_names(&root, &apath, aline, &func, spec.depth);

        for n in names
        {
            effective_queries.push(n);
        }
    }

    // Dedup while preserving original order first, then lexicographic for derived
    let mut seen = BTreeSet::new();
    let mut deduped: Vec<String> = Vec::new();

    for q in effective_queries.into_iter()
    {
        if seen.insert(q.clone())
        {
            deduped.push(q);
        }
    }

    // Configure progress UI (hidden in --quiet mode); use deduped len for accuracy
    let pb = if ctx.quiet
    {
        ProgressBar::hidden()
    }
    else
    {
        let pb = ProgressBar::new(deduped.len() as u64);
        let style = ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar());
        pb.set_style(style);
        pb
    };

    // Accumulate top-N ranked hits per query (stable across runs)
    let mut chosen: Vec<RankedSymbol> = Vec::new();

    for q in &deduped
    {
        let mut hits = index.lookup(q, opts.clone());

        if effective_top_per_query > 0 && hits.len() > effective_top_per_query
        {
            hits.truncate(effective_top_per_query);
        }

        chosen.extend(hits);
        pb.inc(1);
        pb.set_message(format!("matched '{}'", q));
    }

    pb.finish_and_clear();

    // Bail early if nothing matched for all queries
    if chosen.is_empty()
    {
        if args.json
        {
            println!(
                "{}",
                serde_json::json!({"ok": false, "reason": "no_matches"})
            );
            return Ok(());
        }
        bail!("No symbols matched queries: {:?}", args.queries);
    }

    // Convert ranked symbols into line-range "pieces"
    let mut pieces: Vec<Piece> = chosen
        .par_iter()
        .map(|r| piece_from_symbol(&root, &r.symbol))
        .collect::<Result<Vec<_>>>()?;

    // Sort by (file, start_line) so merge can coalesce ranges
    pieces.sort_by(|a, b| {
        (
            a.file
                .clone(),
            a.start_line,
        )
            .cmp(&(
                b.file
                    .clone(),
                b.start_line,
            ))
    });

    // Merge adjacent/overlapping pieces per file deterministically
    pieces = merge_overlaps(pieces);

    // Rank merged pieces by our anchor-aware policy:
    // 1) Anchor file first
    // 2) Files inside the anchor directory
    // 3) Others
    // Tie-breakers: path asc, line asc
    pieces.sort_by_key(|p| {
        let is_anchor = anchor_file
            .map(|af| {
                same_file(
                    &root,
                    p.file
                        .as_path(),
                    af,
                )
            })
            .unwrap_or(false) as u8;

        let scope = in_anchor_dir(
            &root,
            anchor_file,
            p.file
                .as_path(),
        ) as u8;

        (
            Reverse(is_anchor), // anchor first
            Reverse(scope),     // same dir next
            p.file
                .clone(), // path asc
            p.start_line,       // line asc
        )
    });

    // Convert ranked pieces to budgeting items with priorities
    let mut items: Vec<Item> = Vec::new();
    for p in &pieces
    {
        let is_anchor = anchor_file
            .map(|af| {
                same_file(
                    &root,
                    p.file
                        .as_path(),
                    af,
                )
            })
            .unwrap_or(false);

        let in_scope = in_anchor_dir(
            &root,
            anchor_file,
            p.file
                .as_path(),
        );

        let pr = if is_anchor
        {
            Priority::high()
        }
        else if in_scope
        {
            Priority::medium()
        }
        else
        {
            Priority::low()
        };

        items.push(Item {
            id: format!(
                "{}:{}-{}",
                p.file
                    .display(),
                p.start_line,
                p.end_line
            ),
            content: render_piece(p, args.fence),
            priority: pr,
            hard: false,
            min_tokens: 64,
        });
    }

    // Synthesize a template header as a hard, high-priority item
    let header = resolve_template_text(&args.template, &args.queries)?;
    let mut all_items = vec![Item {
        id: "__template__".into(),
        content: header,
        priority: Priority::high(),
        hard: true,
        min_tokens: 80,
    }];
    all_items.extend(items);

    // 3) Apply fail-signal boost (deterministic)
    if !fail_signals.is_empty()
    {
        fail_signal_boost(&mut all_items, &fail_signals, &root);
    }

    // Fit items into the token budget using the selected model with optional features
    let budgeter = Budgeter::new(&model)?;

    // Check if bucket caps are specified
    let fit = if let Some(bucket_spec) = &args.buckets
    {
        // Parse bucket specification and use bucket-aware fitting
        let bucket_caps = parse_bucket_caps(bucket_spec)?;

        // Convert regular items to tagged items (basic tagging for now)
        let tagged_items: Vec<TaggedItem> = all_items
            .into_iter()
            .map(|item| {
                let mut tagged = TaggedItem::from(item);
                // Basic heuristic tagging based on item ID patterns
                if tagged
                    .id
                    .contains("test")
                    || tagged
                        .id
                        .contains("_test")
                {
                    tagged
                        .tags
                        .insert(SpanTag::Test);
                }
                else if tagged
                    .id
                    .contains("trait")
                    || tagged
                        .id
                        .contains("struct")
                    || tagged
                        .id
                        .contains("enum")
                    || tagged
                        .id
                        .contains("pub fn")
                {
                    tagged
                        .tags
                        .insert(SpanTag::Interface);
                }
                else
                {
                    tagged
                        .tags
                        .insert(SpanTag::Code);
                }
                tagged
            })
            .collect();

        // Apply bucket fitting with refusal logs
        let bucket_result =
            fit_with_buckets(&budgeter, tagged_items, bucket_caps, args.novelty_min)?;

        // For now, just use the fitted result and ignore refusals (could log them later)
        bucket_result.fitted
    }
    else
    {
        // Use regular fitting with optional deduplication
        let dedupe_config = args
            .dedupe_threshold
            .map(|threshold| {
                crate::core::budgeter::DedupeConfig {
                    jaccard_threshold: threshold.clamp(0.0, 1.0),
                    ..Default::default()
                }
            });
        budgeter.fit_with_dedupe(all_items, budget, dedupe_config)?
    };

    // Build final output content
    let final_content = if args.json
    {
        // Provide tier label if used (as "A"/"B"/"C")
        let tier_label = tier_opt.map(|t| {
            match t
            {
                Tier::A => "A",
                Tier::B => "B",
                Tier::C => "C",
            }
        });

        // Populate enriched JSON so tests can assert settings
        let out = JsonContext {
            model,
            budget,
            total_tokens: fit.total_tokens,
            tier: tier_label,
            effective_limit,
            effective_top_per_query,
            items: fit
                .items
                .iter()
                .map(|fi| {
                    JsonItem {
                        id: fi
                            .id
                            .clone(),
                        tokens: fi.tokens,
                        content: &fi.content,
                    }
                })
                .collect(),
        };
        // Serialize to a compact single line for CLI use
        serde_json::to_string(&out)?
    }
    else
    {
        let mut content = String::new();
        for it in &fit.items
        {
            content.push_str(&it.content);
        }
        content
    };

    // Output to stdout
    print!("{}", final_content);

    // Show token summary for non-JSON output
    if !args.json && !ctx.quiet
    {
        eprintln!("\n— total tokens: {} / {}", fit.total_tokens, budget);
    }

    // Optional clipboard copy
    if args.clipboard
    {
        copy_to_clipboard(&final_content)?;
        if !ctx.quiet
        {
            eprintln!("✓ Copied to clipboard");
        }
    }

    // Persist most-recent query target to history (best-effort)
    if let Some(first) = chosen.first()
    {
        save_history(
            root.join(".roughup_context_history"),
            &first
                .symbol
                .qualified_name,
        )
        .ok();
    }

    // Indicate successful completion
    Ok(())
}

/// One contiguous, file-local slice of source text
#[derive(Debug, Clone)]
struct Piece
{
    /// File path (repo-relative is preferred, but robustly handled)
    file: PathBuf,
    /// 1-based start line of the slice (inclusive)
    start_line: usize,
    /// 1-based end line of the slice (inclusive)
    end_line: usize,
    /// Captured body text for the slice
    body: String,
}

/// Convert a discovered symbol into an extractable piece
fn piece_from_symbol(
    root: &Path,
    s: &Symbol,
) -> Result<Piece>
{
    // Resolve absolute path to read the file contents
    let abs = if s
        .file
        .is_absolute()
    {
        s.file
            .clone()
    }
    else
    {
        root.join(&s.file)
    };

    // Read file content using the buffered helper
    let content = read_file_smart(&abs)?;
    let text = content.as_ref();

    // Prefer byte-span slicing when boundaries are valid UTF-8
    let body = if let Some(seg) = text.get(s.byte_start..s.byte_end)
    {
        seg.to_string()
    }
    else
    {
        // Fall back to conservative line-based slicing
        let start0 = s
            .start_line
            .saturating_sub(1);
        let end0 = s
            .end_line
            .saturating_sub(1);
        text.lines()
            .enumerate()
            .filter_map(|(i, l)| {
                if i >= start0 && i <= end0
                {
                    Some(l)
                }
                else
                {
                    None
                }
            })
            .collect::<Vec<&str>>()
            .join("\n")
    };

    // Return materialized piece with the original file path
    Ok(Piece {
        file: s
            .file
            .clone(),
        start_line: s.start_line,
        end_line: s.end_line,
        body,
    })
}

/// Merge per-file overlapping/adjacent pieces deterministically
fn merge_overlaps(v: Vec<Piece>) -> Vec<Piece>
{
    // Fast exit for empty input
    if v.is_empty()
    {
        return v;
    }

    // Prepare rolling output vector
    let mut out: Vec<Piece> = Vec::new();

    // Seed with the first piece (sorted upstream)
    let mut cur = v[0].clone();

    // Walk subsequent pieces and merge where appropriate
    for p in v
        .into_iter()
        .skip(1)
    {
        // Merge only within the same file and touching ranges
        if p.file == cur.file && p.start_line <= cur.end_line + 1
        {
            // Only extend if new piece extends beyond current range
            if p.end_line > cur.end_line
            {
                // Calculate overlap: lines already covered by current piece
                // Count overlap only when the new piece actually starts
                // inside the current range. If it merely touches
                // (p.start_line == cur.end_line + 1), we skip 0 lines.
                let overlap_lines: usize = if p.start_line <= cur.end_line
                {
                    cur.end_line - p.start_line + 1
                }
                else
                {
                    0
                };

                // Split new body to exclude overlapping lines
                let new_lines: Vec<&str> = p
                    .body
                    .lines()
                    .collect();
                let non_overlapping = if overlap_lines < new_lines.len()
                {
                    new_lines[overlap_lines..].join("\n")
                }
                else
                {
                    String::new()
                };

                if !non_overlapping.is_empty()
                {
                    // Insert newline if current body lacks terminator
                    if !cur
                        .body
                        .ends_with('\n')
                    {
                        cur.body
                            .push('\n');
                    }
                    cur.body
                        .push_str(&non_overlapping);
                }

                cur.end_line = p.end_line;
            }
        }
        else
        {
            // Flush the accumulated piece and start a new one
            out.push(cur);
            cur = p;
        }
    }

    // Flush the final accumulated piece
    out.push(cur);

    // Return merged vector
    out
}

/// Render a piece as paste-ready text, with optional code fences
fn render_piece(
    p: &Piece,
    fence: bool,
) -> String
{
    // Derive a language hint from the file extension
    let lang = p
        .file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Choose fenced or plain framing based on the flag
    if fence
    {
        format!(
            "// File: {} (lines {}-{})\n```{}\n{}\n```\n\n",
            p.file
                .display(),
            p.start_line,
            p.end_line,
            lang,
            p.body
        )
    }
    else
    {
        format!(
            "// File: {} (lines {}-{})\n{}\n\n",
            p.file
                .display(),
            p.start_line,
            p.end_line,
            p.body
        )
    }
}

/// Render the selected template header text
fn render_template(
    t: ContextTemplate,
    queries: &[String],
) -> String
{
    match t
    {
        ContextTemplate::Refactor =>
        {
            format!(
                "### Task\nRefactor the target symbols: {}.\n\n### Constraints\n- Preserve \
                 behavior; improve structure and readability.\n- Keep public APIs stable.\n\n",
                queries.join(", ")
            )
        }
        ContextTemplate::Bugfix =>
        {
            format!(
                "### Task\nFind and fix the defect related to: {}.\n\n### Notes\n- Write concise \
                 changes; avoid unrelated edits.\n\n",
                queries.join(", ")
            )
        }
        ContextTemplate::Feature =>
        {
            format!(
                "### Task\nImplement the feature touching: {}.\n\n### Acceptance\n- Add or update \
                 tests if present.\n\n",
                queries.join(", ")
            )
        }
        ContextTemplate::Freeform => String::new(),
    }
}

/// Resolve template text from either preset or file path
fn resolve_template_text(
    arg: &Option<TemplateArg>,
    queries: &[String],
) -> Result<String>
{
    match arg
    {
        Some(TemplateArg::Preset(p)) =>
        {
            // Use existing preset renderer
            Ok(render_template(*p, queries))
        }
        Some(TemplateArg::Path(p)) =>
        {
            let raw = fs::read_to_string(p)
                .with_context(|| format!("failed to read template file {}", p.display()))?;
            Ok(normalize_eol(&raw))
        }
        None =>
        {
            // default preset if --template omitted; keep prior behavior
            Ok(render_template(ContextTemplate::Freeform, queries))
        }
    }
}

/// Simple EOL normalizer to keep manifest byte-identical across OSes
fn normalize_eol(s: &str) -> String
{
    let mut out = s.replace("\r\n", "\n");
    if !out.ends_with('\n')
    {
        out.push('\n');
    }
    out
}

/// Extract ContextTemplate for ranking factors
fn extract_context_template(arg: &Option<TemplateArg>) -> Option<ContextTemplate>
{
    match arg
    {
        Some(TemplateArg::Preset(p)) => Some(*p),
        Some(TemplateArg::Path(_)) => Some(ContextTemplate::Freeform), // treat file paths as
        // freeform
        None => Some(ContextTemplate::Freeform),
    }
}

/// Load the MRU-style query history from disk (best-effort)
fn load_history(path: PathBuf) -> Option<Vec<String>>
{
    fs::read_to_string(path)
        .ok()
        .map(|s| {
            s.lines()
                .map(|l| {
                    l.trim()
                        .to_string()
                })
                .filter(|l| !l.is_empty())
                .collect()
        })
}

/// Persist the most recent query to history (best-effort)
fn save_history(
    path: PathBuf,
    qname: &str,
) -> Result<()>
{
    let mut lines = load_history(path.clone()).unwrap_or_default();
    if !lines.contains(&qname.to_string())
    {
        lines.insert(0, qname.to_string());
    }
    while lines.len() > 100
    {
        lines.pop();
    }
    let body = lines.join("\n") + "\n";
    fs::write(path, body).context("write history")
}

/// JSON item emitted under --json mode
#[derive(Serialize)]
struct JsonItem<'a>
{
    /// Stable identifier: "path:start-end" for deterministic parsing
    id: String,
    /// Token cost for this item under the chosen model
    tokens: usize,
    /// Full rendered text content for downstream tools
    content: &'a str,
}

/// JSON envelope emitted under --json mode
/// Augmented JSON context type to surface the effective tier
/// and the derived limits used for this run. This keeps existing
/// consumers working while enabling targeted tests.
#[derive(Serialize)]
struct JsonContext<'a>
{
    /// Name of tokenizer/model used to count tokens
    model: String,
    /// Budget passed to the budgeter (after tier/preset logic)
    budget: usize,
    /// Total tokens after fit() was computed
    total_tokens: usize,
    /// Optional tier label ("A"|"B"|"C") when a preset was used
    #[serde(skip_serializing_if = "Option::is_none")]
    tier: Option<&'a str>,
    /// Effective global candidate limit applied this run
    effective_limit: usize,
    /// Effective top-per-query applied this run
    effective_top_per_query: usize,
    /// Items emitted in the final context payload
    items: Vec<JsonItem<'a>>,
}

/// Normalize a path into a comparable, repo-relative form
fn rel<'a>(
    root: &Path,
    p: &'a Path,
) -> Cow<'a, Path>
{
    // Join relative paths to root to avoid mixed forms
    let abs = if p.is_absolute()
    {
        Cow::Borrowed(p)
    }
    else
    {
        Cow::Owned(root.join(p))
    };
    // Strip the root prefix when possible for stable comparison
    match abs.strip_prefix(root)
    {
        Ok(stripped) => Cow::Owned(stripped.to_path_buf()),
        Err(_) => abs,
    }
}

/// Determine if two paths refer to the same file (repo-relative)
fn same_file(
    root: &Path,
    a: &Path,
    b: &Path,
) -> bool
{
    rel(root, a).as_ref() == rel(root, b).as_ref()
}

// Clipboard support
fn copy_to_clipboard(s: &str) -> Result<()>
{
    let mut cb = arboard::Clipboard::new().context("clipboard init")?;
    cb.set_text(s.to_string())
        .context("clipboard set")?;
    Ok(())
}

/// Determine if `file` resides inside the directory of `anchor_file`
fn in_anchor_dir(
    root: &Path,
    anchor_file: Option<&Path>,
    file: &Path,
) -> bool
{
    if let Some(anchor) = anchor_file
    {
        if let Some(dir) = anchor.parent()
        {
            // Skip if file is the anchor itself
            if same_file(root, file, anchor)
            {
                return false;
            }
            // Normalize to repo-relative for consistent comparison
            let rel_file = rel(root, file);
            let rel_dir = rel(root, dir);
            rel_file.starts_with(rel_dir.as_ref())
        }
        else
        {
            false
        }
    }
    else
    {
        false
    }
}

/// Minimal local auto-detect that defers to registered parsers in fail_signal.rs.
/// This uses a conservative contract: try known parsers; return first non-empty.
fn autodetect_and_parse(text: &str) -> Vec<crate::core::fail_signal::FailSignal>
{
    use crate::core::fail_signal::FailSignalParser;

    // The packet states RustcParser exists; attempt it first.
    // If more parsers are exported, insert here in fixed order for determinism.
    let parsers: [&dyn FailSignalParser; 1] = [&crate::core::fail_signal::RustcParser];
    for p in parsers
    {
        let out = p.parse(text);
        if !out.is_empty()
        {
            return out;
        }
    }
    Vec::new()
}

/// Boost priorities for items proximal to fail signals.
/// Deterministic: stable boost, stable sort by (priority desc, id asc).
/// Complexity: O(n log n) over items.
fn fail_signal_boost(
    items: &mut [Item],
    signals: &[crate::core::fail_signal::FailSignal],
    root: &Path,
)
{
    if signals.is_empty()
    {
        return;
    }

    // Defensive local snapshot to keep iteration deterministic
    // and avoid borrowing complexities.
    let sigs: Vec<_> = signals
        .iter()
        .map(|s| {
            // FailSignal contract: file, line_hits, severity
            let w = match s.severity
            {
                crate::core::fail_signal::Severity::Error => 3.0_f32,
                crate::core::fail_signal::Severity::Warn => 1.5_f32,
                crate::core::fail_signal::Severity::Info => 1.0_f32,
            };
            (
                s.file
                    .clone(),
                &s.line_hits,
                w,
            )
        })
        .collect();

    // Apply boosts
    for item in items.iter_mut()
    {
        // Skip template items
        if item
            .id
            .starts_with("__")
        {
            continue;
        }

        // Parse item ID to extract file path and line range
        // Format: "path/to/file.rs:start-end"
        let (item_file, start_line, end_line) = if let Some(parsed) = parse_item_id(&item.id, root)
        {
            parsed
        }
        else
        {
            continue;
        };

        let mut boost = 0.0_f32;
        for (signal_file, line_hits, weight) in &sigs
        {
            if same_file(root, &item_file, signal_file)
            {
                for &signal_line in line_hits.iter()
                {
                    let distance = distance_to_span(signal_line as u32, start_line, end_line);
                    // Inverse-distance weighting; bounded, stable.
                    // 1/(1+d) avoids div by zero and extreme spikes.
                    let local = *weight / (1.0_f32 + distance as f32);
                    // Cap aggregate to keep TVE guardrails; prevents outsized impact.
                    boost += local.min(2.0_f32);
                }
            }
        }

        if boost > 0.0
        {
            // Apply a gentle multiplier + additive nudge to preserve ordering when equal.
            let old_priority = item.priority;
            let multiplier = (1.0_f32 + (boost * 0.15_f32)).min(1.5_f32);
            let additive = (boost * 0.05_f32).min(0.5_f32);

            item.priority = Priority::custom(
                (old_priority.level as f32 * multiplier + additive * 255.0).min(255.0) as u8,
                (old_priority.relevance * multiplier + additive * 0.1).min(1.0),
                (old_priority.proximity * multiplier + additive * 0.1).min(1.0),
            );
        }
    }
}

/// Parse item ID to extract file path and line range
/// Returns (file_path, start_line, end_line) or None if parsing fails
fn parse_item_id(
    id: &str,
    root: &Path,
) -> Option<(PathBuf, u32, u32)>
{
    // Format: "path/to/file.rs:start-end"
    let colon_pos = id.rfind(':')?;
    let file_part = &id[..colon_pos];
    let line_part = &id[colon_pos + 1..];

    // Parse line range "start-end"
    let dash_pos = line_part.find('-')?;
    let start_str = &line_part[..dash_pos];
    let end_str = &line_part[dash_pos + 1..];

    let start_line: u32 = start_str
        .parse()
        .ok()?;
    let end_line: u32 = end_str
        .parse()
        .ok()?;

    // Convert to absolute path if relative
    let file_path = if Path::new(file_part).is_absolute()
    {
        PathBuf::from(file_part)
    }
    else
    {
        root.join(file_part)
    };

    Some((file_path, start_line, end_line))
}

/// Calculate distance from a line to a span
fn distance_to_span(
    line: u32,
    start: u32,
    end: u32,
) -> u32
{
    if line < start
    {
        start - line
    }
    else if line > end
    {
        line.saturating_sub(end)
    }
    else
    {
        0
    }
}

// --- PATCH: Augment queries with trait-resolve and callgraph ---
pub fn parse_trait_resolve(s: &str) -> Option<(String, String)>
{
    let mut parts = s.splitn(2, "::");
    let ty = parts
        .next()?
        .trim();
    let method = parts
        .next()?
        .trim();
    if ty.is_empty() || method.is_empty()
    {
        return None;
    }
    Some((ty.to_string(), method.to_string()))
}

pub struct CallgraphSpec
{
    pub anchor: Option<(PathBuf, usize)>,
    pub depth: u8,
}

/// Parse callgraph arg "anchor=path:line depth=N".
pub fn parse_callgraph_arg(
    raw: Option<&str>,
    fallback_path: Option<&PathBuf>,
    fallback_line: Option<usize>,
) -> Option<CallgraphSpec>
{
    let mut depth: u8 = 1;
    let mut anchor: Option<(PathBuf, usize)> = None;
    if let Some(s) = raw
    {
        for token in s.split_whitespace()
        {
            if let Some(rest) = token.strip_prefix("depth=")
            {
                if let Ok(n) = rest.parse::<u8>()
                {
                    depth = n.clamp(1, 3); // Clamp for SLA
                }
            }
            else if let Some(rest) = token.strip_prefix("anchor=")
                && let Some((p, l)) = parse_path_line(rest)
            {
                anchor = Some((p, l));
            }
        }
    }

    if anchor.is_none()
        && let (Some(p), Some(l)) = (fallback_path, fallback_line)
    {
        anchor = Some((p.clone(), l));
    }

    anchor.as_ref()?;

    Some(CallgraphSpec { anchor, depth })
}

pub fn parse_path_line(s: &str) -> Option<(PathBuf, usize)>
{
    let (p, l) = s.rsplit_once(':')?;
    let line = l
        .parse::<usize>()
        .ok()?;
    Some((PathBuf::from(p), line))
}

/// Extract a likely function name around the given line.
pub fn extract_function_name_at(
    root: &Path,
    path: &Path,
    line: usize,
) -> Option<String>
{
    let text = fs::read_to_string(root.join(path)).ok()?;
    let lines: Vec<&str> = text
        .lines()
        .collect();

    if lines.is_empty()
    {
        return None;
    }

    let idx = line
        .saturating_sub(1)
        .min(
            lines
                .len()
                .saturating_sub(1),
        );
    let lo = idx.saturating_sub(80);
    let hi = (idx + 80).min(
        lines
            .len()
            .saturating_sub(1),
    );

    for i in (lo..=idx).rev()
    {
        if let Some(name) = parse_fn_decl(lines[i])
        {
            return Some(name);
        }
    }

    for line in &lines[idx..=hi]
    {
        if let Some(name) = parse_fn_decl(line)
        {
            return Some(name);
        }
    }
    None
}

fn parse_fn_decl(line: &str) -> Option<String>
{
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 3 <= bytes.len()
    {
        if bytes[i] == b'f'
            && bytes[i + 1] == b'n'
            && is_space(
                bytes
                    .get(i + 2)
                    .copied(),
            )
        {
            let mut j = i + 2;
            while j < bytes.len() && is_space(Some(bytes[j]))
            {
                j += 1;
            }
            let (name, k) = take_ident(bytes, j)?;
            let mut m = k;
            while m < bytes.len() && is_space(Some(bytes[m]))
            {
                m += 1;
            }
            if m < bytes.len() && bytes[m] == b'('
            {
                return Some(name);
            }
        }
        i += 1;
    }
    None
}

fn is_space(b: Option<u8>) -> bool
{
    matches!(b, Some(b' ' | b'\t'))
}

fn take_ident(
    bytes: &[u8],
    mut i: usize,
) -> Option<(String, usize)>
{
    if i >= bytes.len()
    {
        return None;
    }
    let first = bytes[i];
    if !((first == b'_') || (first as char).is_ascii_alphabetic())
    {
        return None;
    }
    let start = i;
    i += 1;
    while i < bytes.len()
    {
        let c = bytes[i] as char;
        if c.is_ascii_alphanumeric() || bytes[i] == b'_'
        {
            i += 1;
        }
        else
        {
            break;
        }
    }
    let name = String::from_utf8_lossy(&bytes[start..i]).into_owned();
    Some((name, i))
}

/// Collect neighbor function names up to a small depth using quick scans.
pub fn collect_callgraph_names(
    root: &Path,
    anchor_path: &Path,
    anchor_line: usize,
    anchor_fn: &str,
    depth: u8,
) -> Vec<String>
{
    let mut out = BTreeSet::new();
    let mut frontier: VecDeque<(String, PathBuf, usize, u8)> = VecDeque::new();
    frontier.push_back((
        anchor_fn.to_string(),
        anchor_path.to_path_buf(),
        anchor_line,
        0,
    ));
    while let Some((_name, path, line, d)) = frontier.pop_front()
    {
        if d >= depth
        {
            continue;
        }
        if let Some(calls) = scan_calls_near(root, &path, line, 120)
        {
            for c in calls
            {
                if out.insert(c.clone())
                {
                    frontier.push_back((c, path.clone(), line, d + 1));
                }
            }
        }
    }
    out.into_iter()
        .collect()
}

fn scan_calls_near(
    root: &Path,
    path: &Path,
    line: usize,
    window: usize,
) -> Option<Vec<String>>
{
    let text = fs::read_to_string(root.join(path)).ok()?;
    let lines: Vec<&str> = text
        .lines()
        .collect();

    if lines.is_empty()
    {
        return Some(Vec::new());
    }

    let idx = line
        .saturating_sub(1)
        .min(
            lines
                .len()
                .saturating_sub(1),
        );
    let lo = idx.saturating_sub(window);
    let hi = (idx + window).min(
        lines
            .len()
            .saturating_sub(1),
    );

    let mut out = BTreeSet::new();

    for line in &lines[lo..=hi]
    {
        let mut j = 0usize;
        let b = line.as_bytes();

        while j < b.len()
        {
            if let Some((name, k)) = take_ident(b, j)
            {
                let mut m = k;

                while m < b.len() && is_space(Some(b[m]))
                {
                    m += 1;
                }

                if m < b.len()
                    && b[m] == b'('
                    && name != "if"
                    && name != "for"
                    && name != "while"
                    && name != "match"
                {
                    out.insert(name);
                }

                j = k + 1;

                continue;
            }

            j += 1;
        }
    }
    Some(
        out.into_iter()
            .collect(),
    )
}
