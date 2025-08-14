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
//! - Anchor file gets highest priority; same-directory files get scope
//!   bonus; remaining files follow lexicographic path order.
//! - Anchor equality and scope checks are robust to abs/rel path
//!   mismatches.

use anyhow::{Context, Result, bail}; // error context
use indicatif::{ProgressBar, ProgressStyle}; // CLI progress
use rayon::prelude::*; // parallel map
use serde::Serialize; // JSON structs
use std::borrow::Cow; // path views
use std::cmp::Reverse; // sort keys
use std::collections::HashSet; // history set
use std::fs; // file IO
use std::path::{Path, PathBuf}; // paths

use crate::cli::{
    AppContext,
    ContextArgs, // CLI types
    ContextTemplate,
};
use crate::core::budgeter::{
    Budgeter,
    Item, // token fit
    Priority,
};
use crate::core::symbol_index::{
    LookupOptions, // search
    RankedSymbol,
    SymbolIndex,
};
use crate::core::symbols::Symbol; // symbol def
use crate::infra::io::read_file_smart; // fast reads

/// Run the `context` command end-to-end
pub fn run(args: ContextArgs, ctx: &AppContext) -> Result<()> {
    // Load persisted config (best-effort; defaults if missing)
    let cfg = crate::infra::config::load_config().unwrap_or_default();

    // Resolve repository root passed to the command
    let root = args.path.clone();

    // Determine the symbols index path (CLI wins, else config)
    let symbols_path = if args.symbols.exists() {
        args.symbols.clone()
    } else {
        cfg.symbols.output_file.into()
    };

    // Fail immediately if symbols index is unavailable
    if !Path::new(&symbols_path).exists() {
        bail!(
            "Symbols file not found: {}. Run 'rup symbols' first.",
            symbols_path.display()
        );
    }

    // Resolve model used by the budgeter (CLI wins, else config)
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| cfg.chunk.model.clone());

    // Resolve token budget (default conservative)
    let budget = args.budget.unwrap_or(6000);

    // Capture template selection for header rendering
    let template = args.template.clone();

    // Load query history if present (best-effort; no failure)
    let history = load_history(root.join(".roughup_context_history"));

    // Materialize a SymbolIndex view over symbols.jsonl
    let index = SymbolIndex::load(&symbols_path)?;

    // Convert history list into a set for O(1) checks
    let hist_set = history
        .as_ref()
        .map(|v| v.iter().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();

    // Capture anchor hints from CLI (file + optional line)
    let anchor_file = args.anchor.as_deref();
    let anchor_line = args.anchor_line;

    // Build lookup options to drive symbol search
    let opts = LookupOptions {
        semantic: args.semantic,
        anchor_file,
        anchor_line,
        history: Some(&hist_set),
        limit: args.limit,
        kinds: None,
    };

    // Configure progress UI (hidden in --quiet mode)
    let pb = if ctx.quiet {
        ProgressBar::hidden()
    } else {
        let pb = ProgressBar::new(args.queries.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] \
                     [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap(),
        );
        pb
    };

    // Accumulate top-N ranked hits per query (stable across runs)
    let mut chosen: Vec<RankedSymbol> = Vec::new();
    for q in &args.queries {
        let mut hits = index.lookup(q, opts.clone());
        if args.top_per_query > 0 && hits.len() > args.top_per_query {
            hits.truncate(args.top_per_query);
        }
        chosen.extend(hits);
        pb.inc(1);
        pb.set_message(format!("matched '{}'", q));
    }
    pb.finish_and_clear();

    // Bail early if nothing matched for all queries
    if chosen.is_empty() {
        if args.json {
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
    pieces.sort_by(|a, b| (a.file.clone(), a.start_line).cmp(&(b.file.clone(), b.start_line)));

    // Merge adjacent/overlapping pieces per file deterministically
    pieces = merge_overlaps(pieces);

    // Rank merged pieces by our anchor-aware policy:
    // 1) Anchor file first
    // 2) Files inside the anchor directory
    // 3) Others
    // Tie-breakers: path asc, line asc
    pieces.sort_by_key(|p| {
        let is_anchor = anchor_file
            .map(|af| same_file(&root, p.file.as_path(), af))
            .unwrap_or(false) as u8;

        let scope = in_anchor_dir(&root, anchor_file, p.file.as_path()) as u8;

        (
            Reverse(is_anchor), // anchor first
            Reverse(scope),     // same dir next
            p.file.clone(),     // path asc
            p.start_line,       // line asc
        )
    });

    // Convert ranked pieces to budgeting items with priorities
    let mut items: Vec<Item> = Vec::new();
    for p in &pieces {
        let is_anchor = anchor_file
            .map(|af| same_file(&root, p.file.as_path(), af))
            .unwrap_or(false);

        let in_scope = in_anchor_dir(&root, anchor_file, p.file.as_path());

        let pr = if is_anchor {
            Priority::High
        } else if in_scope {
            Priority::Medium
        } else {
            Priority::Low
        };

        items.push(Item {
            id: format!("{}:{}-{}", p.file.display(), p.start_line, p.end_line),
            content: render_piece(p, args.fence),
            priority: pr,
            hard: false,
            min_tokens: 64,
        });
    }

    // Synthesize a template header as a hard, high-priority item
    let header = render_template(template, &args.queries);
    let mut all_items = vec![Item {
        id: "__template__".into(),
        content: header,
        priority: Priority::High,
        hard: true,
        min_tokens: 80,
    }];
    all_items.extend(items);

    // Fit items into the token budget using the selected model
    let budgeter = Budgeter::new(&model)?;
    let fit = budgeter.fit(all_items, budget)?;

    // Build final output content
    let final_content = if args.json {
        let out = JsonContext {
            model,
            budget,
            total_tokens: fit.total_tokens,
            items: fit
                .items
                .iter()
                .map(|fi| JsonItem {
                    id: fi.id.clone(),
                    tokens: fi.tokens,
                    content: &fi.content,
                })
                .collect(),
        };
        serde_json::to_string(&out)?
    } else {
        let mut content = String::new();
        for it in &fit.items {
            content.push_str(&it.content);
        }
        content
    };

    // Output to stdout
    print!("{}", final_content);

    // Show token summary for non-JSON output
    if !args.json && !ctx.quiet {
        eprintln!("\n— total tokens: {} / {}", fit.total_tokens, budget);
    }

    // Optional clipboard copy
    if args.clipboard {
        copy_to_clipboard(&final_content)?;
        if !ctx.quiet {
            eprintln!("✓ Copied to clipboard");
        }
    }

    // Persist most-recent query target to history (best-effort)
    if let Some(first) = chosen.first() {
        save_history(
            root.join(".roughup_context_history"),
            &first.symbol.qualified_name,
        )
        .ok();
    }

    // Indicate successful completion
    Ok(())
}

/// One contiguous, file-local slice of source text
#[derive(Debug, Clone)]
struct Piece {
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
fn piece_from_symbol(root: &Path, s: &Symbol) -> Result<Piece> {
    // Resolve absolute path to read the file contents
    let abs = if s.file.is_absolute() {
        s.file.clone()
    } else {
        root.join(&s.file)
    };

    // Read file content using the buffered helper
    let content = read_file_smart(&abs)?;
    let text = content.as_ref();

    // Prefer byte-span slicing when boundaries are valid UTF-8
    let body = if let Some(seg) = text.get(s.byte_start..s.byte_end) {
        seg.to_string()
    } else {
        // Fall back to conservative line-based slicing
        let start0 = s.start_line.saturating_sub(1);
        let end0 = s.end_line.saturating_sub(1);
        text.lines()
            .enumerate()
            .filter_map(|(i, l)| {
                if i >= start0 && i <= end0 {
                    Some(l)
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>()
            .join("\n")
    };

    // Return materialized piece with the original file path
    Ok(Piece {
        file: s.file.clone(),
        start_line: s.start_line,
        end_line: s.end_line,
        body,
    })
}

/// Merge per-file overlapping/adjacent pieces deterministically
fn merge_overlaps(v: Vec<Piece>) -> Vec<Piece> {
    // Fast exit for empty input
    if v.is_empty() {
        return v;
    }

    // Prepare rolling output vector
    let mut out: Vec<Piece> = Vec::new();

    // Seed with the first piece (sorted upstream)
    let mut cur = v[0].clone();

    // Walk subsequent pieces and merge where appropriate
    for p in v.into_iter().skip(1) {
        // Merge only within the same file and touching ranges
        if p.file == cur.file && p.start_line <= cur.end_line + 1 {
            // Only extend if new piece extends beyond current range
            if p.end_line > cur.end_line {
                // Calculate overlap: lines already covered by current piece
                // Count overlap only when the new piece actually starts
                // inside the current range. If it merely touches
                // (p.start_line == cur.end_line + 1), we skip 0 lines.
                let overlap_lines: usize = if p.start_line <= cur.end_line {
                    cur.end_line - p.start_line + 1
                } else {
                    0
                };

                // Split new body to exclude overlapping lines
                let new_lines: Vec<&str> = p.body.lines().collect();
                let non_overlapping = if overlap_lines < new_lines.len() {
                    new_lines[overlap_lines..].join("\n")
                } else {
                    String::new()
                };

                if !non_overlapping.is_empty() {
                    // Insert newline if current body lacks terminator
                    if !cur.body.ends_with('\n') {
                        cur.body.push('\n');
                    }
                    cur.body.push_str(&non_overlapping);
                }

                cur.end_line = p.end_line;
            }
        } else {
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
fn render_piece(p: &Piece, fence: bool) -> String {
    // Derive a language hint from the file extension
    let lang = p.file.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Choose fenced or plain framing based on the flag
    if fence {
        format!(
            "// File: {} (lines {}-{})\n```{}\n{}\n```\n\n",
            p.file.display(),
            p.start_line,
            p.end_line,
            lang,
            p.body
        )
    } else {
        format!(
            "// File: {} (lines {}-{})\n{}\n\n",
            p.file.display(),
            p.start_line,
            p.end_line,
            p.body
        )
    }
}

/// Render the selected template header text
fn render_template(t: ContextTemplate, queries: &[String]) -> String {
    match t {
        ContextTemplate::Refactor => format!(
            "### Task\nRefactor the target symbols: {}.\n\n\
             ### Constraints\n- Preserve behavior; improve \
             structure and readability.\n- Keep public APIs \
             stable.\n\n",
            queries.join(", ")
        ),
        ContextTemplate::Bugfix => format!(
            "### Task\nFind and fix the defect related to: {}.\n\n\
             ### Notes\n- Write concise changes; avoid unrelated \
             edits.\n\n",
            queries.join(", ")
        ),
        ContextTemplate::Feature => format!(
            "### Task\nImplement the feature touching: {}.\n\n\
             ### Acceptance\n- Add or update tests if present.\n\n",
            queries.join(", ")
        ),
        ContextTemplate::Freeform => String::new(),
    }
}

/// Load the MRU-style query history from disk (best-effort)
fn load_history(path: PathBuf) -> Option<Vec<String>> {
    fs::read_to_string(path).ok().map(|s| {
        s.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    })
}

/// Persist the most recent query to history (best-effort)
fn save_history(path: PathBuf, qname: &str) -> Result<()> {
    let mut lines = load_history(path.clone()).unwrap_or_default();
    if !lines.contains(&qname.to_string()) {
        lines.insert(0, qname.to_string());
    }
    while lines.len() > 100 {
        lines.pop();
    }
    let body = lines.join("\n") + "\n";
    fs::write(path, body).context("write history")
}

/// JSON item emitted under --json mode
#[derive(Serialize)]
struct JsonItem<'a> {
    /// Stable identifier: "path:start-end" for deterministic parsing
    id: String,
    /// Token cost for this item under the chosen model
    tokens: usize,
    /// Full rendered text content for downstream tools
    content: &'a str,
}

/// JSON envelope emitted under --json mode
#[derive(Serialize)]
struct JsonContext<'a> {
    /// Name of the model used for token counting
    model: String,
    /// Requested token budget for the assembly
    budget: usize,
    /// Actual total token count after fitting
    total_tokens: usize,
    /// Ordered list of items to include
    items: Vec<JsonItem<'a>>,
}

/// Normalize a path into a comparable, repo-relative form
fn rel<'a>(root: &Path, p: &'a Path) -> Cow<'a, Path> {
    // Join relative paths to root to avoid mixed forms
    let abs = if p.is_absolute() {
        Cow::Borrowed(p)
    } else {
        Cow::Owned(root.join(p))
    };
    // Strip the root prefix when possible for stable comparison
    match abs.strip_prefix(root) {
        Ok(stripped) => Cow::Owned(stripped.to_path_buf()),
        Err(_) => abs,
    }
}

/// Determine if two paths refer to the same file (repo-relative)
fn same_file(root: &Path, a: &Path, b: &Path) -> bool {
    rel(root, a).as_ref() == rel(root, b).as_ref()
}

// Clipboard support
fn copy_to_clipboard(s: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("clipboard init")?;
    cb.set_text(s.to_string()).context("clipboard set")?;
    Ok(())
}

/// Determine if `file` resides inside the directory of `anchor_file`
fn in_anchor_dir(root: &Path, anchor_file: Option<&Path>, file: &Path) -> bool {
    if let Some(anchor) = anchor_file {
        if let Some(dir) = anchor.parent() {
            // Skip if file is the anchor itself
            if same_file(root, file, anchor) {
                return false;
            }
            // Normalize to repo-relative for consistent comparison
            let rel_file = rel(root, file);
            let rel_dir = rel(root, dir);
            rel_file.starts_with(rel_dir.as_ref())
        } else {
            false
        }
    } else {
        false
    }
}
