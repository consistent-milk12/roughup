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

use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    time::Instant,
}; // path views
use std::{collections::VecDeque, path::Path}; // sort keys
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
}; // history set
use std::{fs as StdFs, time::Duration}; // file IO

use anyhow::{Context, Result, bail}; // error context
use rayon::prelude::*; // parallel map
use serde::Serialize; // JSON structs

use crate::cli::{
    AppContext,
    ContextArgs, // CLI types
    ContextTemplate,
    TemplateArg,
    TierArg, // tier presets
};
use crate::core::symbol_index::{
    LookupOptions, // search
    RankedSymbol,
    SymbolIndex,
};
use crate::core::symbols::Symbol; // symbol def
use crate::{
    cli_ext::anchor_cmd::{AnchorArgs, OutputFormat, validate_anchor_with_hints},
    infra::io::read_file_smart,
};
use camino::Utf8Path;
use crate::{
    core::{
        budgeter::{
            Budgeter,
            Item,
            Priority,
            SpanTag,
            TaggedItem, // budget tags
            fit_with_buckets,
            parse_bucket_caps,
        },
        fail_signal::FailSignal,
    },
    infra::config::Config,
}; // fast reads

// Constants for bounded operations and scanning
const DEFAULT_SCAN_WINDOW: usize = 120;
const DEFAULT_FILES_PER_HOP: usize = 20;
const DEFAULT_EDGES_LIMIT: usize = 500;
const MAX_CALLGRAPH_DEPTH: u8 = 6;
const DEFAULT_CALL_SCAN_WINDOW: usize = 128;
const MAX_FRESHNESS_DEPTH: usize = 5;
const LOCKFILE_POLL_INTERVAL_MS: u64 = 200;
const LOCKFILE_MAX_WAIT_MS: u64 = 10_000;
const FUNCTION_SEARCH_WINDOW: usize = 80;

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

/// Prepared environment for a context run.
#[expect(unused, reason = "TODO: MARKED FOR USE")]
struct ContextEnvironment
{
    root: PathBuf,
    cfg: Config,
    symbols_path: PathBuf,
    model: String,
    tier_opt: Option<Tier>,
    budget: usize,
    effective_limit: usize,
    effective_top_per_query: usize,
    args: ContextArgs,
    ctx: AppContext,
    history: Option<Vec<String>>,
    hist_set: HashSet<String>,
}

/// Collected intermediate artifacts from symbol search phase.
#[expect(unused, reason = "TODO: MARKED FOR USE")]
struct Collected
{
    deduped_queries: Vec<String>,
    chosen: Vec<RankedSymbol>,
    fail_signals: Vec<FailSignal>,
    anchor_file: Option<PathBuf>,
    anchor_line: Option<usize>,
}

/// Final assembly output (pre-rendered string + token count).
struct Assembled
{
    final_content: String,
    total_tokens: usize,
    first_symbol_name: Option<String>,
}

pub struct ContextAssembler;

impl ContextAssembler
{
    /// Run the `context` command end-to-end
    pub fn run(
        args: ContextArgs,
        ctx: &AppContext,
    ) -> Result<()>
    {
        // Phase 1: prepare environment (config, paths, budgets, index)
        let env = Self::prepare_context(args, ctx)?;

        // Phase 2: collect symbols (queries, callgraph, lookups, fail signals)
        let collected = Self::collect_symbols(&env)?;

        // Phase 3: assemble pieces (merge, rank, budget fit, format)
        let assembled = Self::assemble_pieces(&env, &collected)?;

        // Phase 4: output (stdout/json, clipboard, history)
        Self::output_results(&env, &collected, &assembled)
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
                    "### Task\nFind and fix the defect related to: {}.\n\n### Notes\n- Write \
                     concise changes; avoid unrelated edits.\n\n",
                    queries.join(", ")
                )
            }

            ContextTemplate::Feature =>
            {
                format!(
                    "### Task\nImplement the feature touching: {}.\n\n### Acceptance\n- Add or \
                     update tests if present.\n\n",
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
                Ok(Self::render_template(*p, queries))
            }

            Some(TemplateArg::Path(p)) =>
            {
                let raw = StdFs::read_to_string(p)
                    .with_context(|| format!("failed to read template file {}", p.display()))?;
                Ok(Self::normalize_eol(&raw))
            }

            None =>
            {
                // default preset if --template omitted; keep prior behavior
                Ok(Self::render_template(ContextTemplate::Freeform, queries))
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
    #[expect(unused, reason = "TODO: MARKED FOR USE")]
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
        StdFs::read_to_string(path)
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
        let mut lines = Self::load_history(path.clone()).unwrap_or_default();

        if !lines.contains(&qname.to_string())
        {
            lines.insert(0, qname.to_string());
        }

        while lines.len() > 100
        {
            lines.pop();
        }

        let body = lines.join("\n") + "\n";

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                StdFs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
        }

        StdFs::write(path, body).context("write history")
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

    /// Determine if two paths refer to the same file with robust canonicalization
    fn same_file(
        root: &Path,
        a: &Path,
        b: &Path,
    ) -> bool
    {
        let ra = Self::resolve_path(root, a);
        let rb = Self::resolve_path(root, b);

        // Try canonicalization first for symlink/.. robustness
        match (
            std::fs::canonicalize(&ra).ok(),
            std::fs::canonicalize(&rb).ok(),
        )
        {
            (Some(x), Some(y)) => x == y,
            // Fallback to repo-relative comparison if canonicalization fails
            _ => Self::rel(root, a).as_ref() == Self::rel(root, b).as_ref(),
        }
    }

    /// Helper to resolve paths against repository root
    fn resolve_path(
        root: &Path,
        p: &Path,
    ) -> PathBuf
    {
        if p.is_absolute()
        {
            p.to_path_buf()
        }
        else
        {
            root.join(p)
        }
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
                if Self::same_file(root, file, anchor)
                {
                    return false;
                }

                // Normalize to repo-relative for consistent comparison
                let rel_file = Self::rel(root, file);
                let rel_dir = Self::rel(root, dir);

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
    fn autodetect_and_parse(text: &str) -> Vec<FailSignal>
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
        signals: &[FailSignal],
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
            let (item_file, start_line, end_line) = if let Some(parsed) =
                Self::parse_item_id(&item.id, root)
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
                if Self::same_file(root, &item_file, signal_file)
                {
                    for &signal_line in line_hits.iter()
                    {
                        let distance =
                            Self::distance_to_span(signal_line as u32, start_line, end_line);
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

    // =========================== Helper Methods =======================

    /// Check if symbols index is fresh compared to source files
    fn index_is_fresh(
        root: &Path,
        symbols_path: &Path,
    ) -> bool
    {
        let symbols_metadata = match StdFs::metadata(symbols_path)
        {
            Ok(m) => m,
            Err(_) => return false,
        };

        let symbols_mtime = symbols_metadata
            .modified()
            .unwrap_or(std::time::UNIX_EPOCH);

        Self::is_dir_fresh_recursive(root, symbols_mtime, 0)
    }

    /// Helper for recursive directory freshness checking
    fn is_dir_fresh_recursive(
        dir: &Path,
        symbols_mtime: std::time::SystemTime,
        depth: usize,
    ) -> bool
    {
        const SKIP_DIRS: &[&str] = &[
            "target",
            "node_modules",
            ".git",
            "build",
            "dist",
            ".next",
            "venv",
            "__pycache__",
            ".vscode",
            ".idea",
        ];

        if depth > MAX_FRESHNESS_DEPTH
        {
            return true;
        }

        let entries = match StdFs::read_dir(dir)
        {
            Ok(e) => e,
            Err(_) => return true,
        };

        for entry in entries
        {
            let entry = match entry
            {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if SKIP_DIRS.contains(&file_name)
            {
                continue;
            }

            // Skip symlinks to avoid loops and performance issues
            let metadata = match StdFs::symlink_metadata(&path)
            {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata
                .file_type()
                .is_symlink()
            {
                continue;
            }

            if path.is_dir()
            {
                if !Self::is_dir_fresh_recursive(&path, symbols_mtime, depth + 1)
                {
                    return false;
                }
            }
            else if let Some(ext) = path
                .extension()
                .and_then(|e| e.to_str())
                && matches!(ext, "rs" | "py" | "js" | "ts" | "tsx" | "go" | "cpp" | "h")
                && let Ok(mtime) = metadata.modified()
                && mtime > symbols_mtime
            {
                return false;
            }
        }

        true
    }

    /// Race-free symbols generation with lockfile and timeout
    fn ensure_symbols_with_lock(
        args: &crate::cli::SymbolsArgs,
        ctx: &AppContext,
        symbols_path: &Path,
    ) -> Result<()>
    {
        let lock_path = symbols_path.with_extension("lock");

        // Try to create lockfile atomically
        match StdFs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) =>
            {
                // We got the lock, check freshness after acquiring
                if symbols_path.exists() && Self::index_is_fresh(&args.path, symbols_path)
                {
                    // Already fresh, no need to regenerate
                    let _ = StdFs::remove_file(&lock_path);
                    return Ok(());
                }

                // Generate symbols
                let result = crate::core::symbols::run(args.clone(), ctx);

                // Always clean up lock
                let _ = StdFs::remove_file(&lock_path);

                result
            }
            Err(_) =>
            {
                // Lock exists, poll with timeout
                let start = Instant::now();

                loop
                {
                    std::thread::sleep(Duration::from_millis(LOCKFILE_POLL_INTERVAL_MS));

                    // Check if symbols appeared or lock disappeared
                    if symbols_path.exists() && Self::index_is_fresh(&args.path, symbols_path)
                    {
                        return Ok(());
                    }

                    if !lock_path.exists()
                    {
                        // Lock gone but no symbols - try again
                        break;
                    }

                    // Timeout check
                    if start
                        .elapsed()
                        .as_millis()
                        > LOCKFILE_MAX_WAIT_MS as u128
                    {
                        return Err(anyhow::anyhow!(
                            "Symbols generation timeout after {}ms",
                            LOCKFILE_MAX_WAIT_MS
                        ));
                    }
                }

                // Lock disappeared but no symbols - retry once
                Self::ensure_symbols_with_lock(args, ctx, symbols_path)
            }
        }
    }

    // =========================== Phase Implementation ================

    // =========================== Phase 1 ================================

    fn prepare_context(
        args: ContextArgs,
        ctx: &AppContext,
    ) -> Result<ContextEnvironment>
    {
        // Load config (best effort)
        let cfg = crate::infra::config::load_config().unwrap_or_default();

        // Resolve root, symbols path, model
        let root = args
            .path
            .clone();
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
                .clone()
                .into()
        };
        let model = args
            .model
            .clone()
            .unwrap_or_else(|| {
                cfg.chunk
                    .model
                    .clone()
            });

        // Resolve tier
        let tier_opt: Option<Tier> = args
            .tier
            .clone()
            .map(Into::into);

        // Budget selection
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

        // Effective caps (prefer args; else tier; else compiled defaults)
        // Note: If possible, make these Option<usize> in CLI to avoid heuristics.
        let compiled_default_top_per_query: usize = 8;
        let compiled_default_limit: usize = 256;

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

        // History (best effort)
        let history = Self::load_history(root.join(".rup/context_history"));
        let hist_set = history
            .as_ref()
            .map(|v| {
                v.iter()
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();

        // Auto-index (race-free) if missing or stale
        let no_auto = std::env::var("ROUGHUP_NO_AUTO_INDEX").is_ok();
        if !Path::new(&symbols_path).exists() && !no_auto
        {
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
            if let Err(e) = Self::ensure_symbols_with_lock(&sym_args, ctx, &symbols_path)
                && !ctx.quiet
            {
                eprintln!("(warn) auto symbols generation failed: {e}");
            }
        }
        else if Path::new(&symbols_path).exists()
            && !Self::index_is_fresh(&args.path, &symbols_path)
            && !no_auto
        {
            if !ctx.quiet
            {
                eprintln!("(info) symbols index stale; regenerating");
            }
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
            let _ = Self::ensure_symbols_with_lock(&sym_args, ctx, &symbols_path);
        }

        // If still missing, return JSON stub or bail later in output phase
        Ok(ContextEnvironment {
            root,
            cfg,
            symbols_path,
            model,
            tier_opt,
            budget,
            effective_limit,
            effective_top_per_query,
            args,
            ctx: ctx.clone(),
            history,
            hist_set,
        })
    }

    // =========================== Phase 2 ================================

    fn collect_symbols(env: &ContextEnvironment) -> Result<Collected>
    {
        // Guard: if symbols are missing, keep going; assemble/output phase
        // will format a consistent error JSON or bail in text mode.
        // Load index now; if missing, we return an empty chosen list.
        let index = match SymbolIndex::load(&env.symbols_path)
        {
            Ok(ix) => ix,
            Err(_) =>
            {
                return Ok(Collected {
                    deduped_queries: Vec::new(),
                    chosen: Vec::new(),
                    fail_signals: Vec::new(),
                    anchor_file: env
                        .args
                        .anchor
                        .clone(),
                    anchor_line: env
                        .args
                        .anchor_line,
                });
            }
        };

        // Validate anchor positioning if --hint-anchors is enabled
        if env.args.hint_anchors
        {
            if let (Some(anchor_path), Some(anchor_line)) = (&env.args.anchor, env.args.anchor_line)
            {
                let anchor_str = format!("{}:{}", anchor_path.display(), anchor_line);
                let anchor_args = AnchorArgs {
                    hint_anchors: true,
                    why: None,
                    format: OutputFormat::Text,
                };
                
                match validate_anchor_with_hints(
                    Utf8Path::from_path(&env.root).unwrap_or_else(|| Utf8Path::new(".")), 
                    &anchor_str, 
                    &anchor_args
                )
                {
                    Ok(Some(validated_fn)) =>
                    {
                        eprintln!("✓ Anchor validated: {} at {}:{}-{}", 
                                 validated_fn.qualified_name, 
                                 validated_fn.file, 
                                 validated_fn.start_line, 
                                 validated_fn.end_line);
                    }
                    Ok(None) =>
                    {
                        eprintln!("⚠ Anchor validation returned no function");
                    }
                    Err(e) =>
                    {
                        eprintln!("⚠ Anchor validation failed: {}", e);
                        // Continue processing - validation is advisory
                    }
                }
            }
            else if env.args.anchor.is_some() || env.args.anchor_line.is_some()
            {
                eprintln!("⚠ Incomplete anchor: both --anchor FILE and --anchor-line LINE are required for validation");
            }
        }

        // Fail signals (best effort, autodetect)
        let mut fail_signals: Vec<FailSignal> = Vec::new();
        if let Some(path) = env
            .args
            .fail_signal
            .as_ref()
            && let Ok(text) = std::fs::read_to_string(path)
        {
            let parsed = Self::autodetect_and_parse(&text);

            if !parsed.is_empty()
            {
                fail_signals = parsed;
            }
        }

        // Build effective queries (base + trait-resolve + callgraph)
        let mut effective_queries: Vec<String> = env
            .args
            .queries
            .clone();

        if let Some(q) = env
            .args
            .trait_resolve
            .as_ref()
            && let Some((ty, method)) = Self::parse_trait_resolve(q)
        {
            effective_queries.push(format!("trait {}", ty));
            effective_queries.push(format!("impl {} for", ty));
            effective_queries.push(format!("{}::{}", ty, method));
        }

        if let Some(spec) = CallGraph::parse_callgraph_arg(
            env.args
                .callgraph
                .as_deref(),
            env.args
                .anchor
                .as_ref(),
            env.args
                .anchor_line,
        ) && let Some((ref apath, aline)) = spec.anchor
            && let Some(_func) = CallGraph::extract_function_name_at(&env.root, apath, aline)
        {
            let names = CallGraph::collect_callgraph_names_bounded(&env.root, &spec);

            for n in names
            {
                effective_queries.push(n);
            }
        }

        // Deduplicate while preserving order
        let mut seen = std::collections::BTreeSet::new();
        let mut deduped: Vec<String> = Vec::new();

        for q in effective_queries.into_iter()
        {
            if seen.insert(q.clone())
            {
                deduped.push(q);
            }
        }

        // Progress bar
        let pb = if env
            .ctx
            .quiet
        {
            indicatif::ProgressBar::hidden()
        }
        else
        {
            let pb = indicatif::ProgressBar::new(deduped.len() as u64);
            let style = indicatif::ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar());
            pb.set_style(style);
            pb
        };

        // Lookup options (borrow-free: use owned data and clones)
        let anchor_file = env
            .args
            .anchor
            .clone();
        let anchor_line = env
            .args
            .anchor_line;
        let opts = LookupOptions {
            semantic: env
                .args
                .semantic,
            anchor_file: anchor_file.as_deref(),
            anchor_line,
            history: Some(&env.hist_set),
            limit: env.effective_limit,
            kinds: None,
        };

        // Accumulate chosen
        let mut chosen: Vec<RankedSymbol> = Vec::new();
        for q in &deduped
        {
            let mut hits = index.lookup(q, opts.clone());
            if env.effective_top_per_query > 0 && hits.len() > env.effective_top_per_query
            {
                hits.truncate(env.effective_top_per_query);
            }
            chosen.extend(hits);
            pb.inc(1);
            pb.set_message(format!("matched '{}'", q));
        }
        pb.finish_and_clear();

        Ok(Collected {
            deduped_queries: deduped,
            chosen,
            fail_signals,
            anchor_file,
            anchor_line,
        })
    }

    // =========================== Phase 3 ================================

    fn assemble_pieces(
        env: &ContextEnvironment,
        col: &Collected,
    ) -> Result<Assembled>
    {
        // Missing symbols index or no matches: defer to output phase.
        if col
            .chosen
            .is_empty()
            && !Path::new(&env.symbols_path).exists()
        {
            return Ok(Assembled {
                final_content: String::new(),
                total_tokens: 0,
                first_symbol_name: None,
            });
        }
        if col
            .chosen
            .is_empty()
        {
            // Build a consistent JSON/text in output phase
            return Ok(Assembled {
                final_content: String::new(),
                total_tokens: 0,
                first_symbol_name: None,
            });
        }

        // Convert to pieces
        let mut pieces: Vec<Piece> = col
            .chosen
            .par_iter()
            .map(|r| Self::piece_from_symbol(&env.root, &r.symbol))
            .collect::<Result<Vec<_>>>()?;

        // Sort by (file, start_line) for deterministic merge
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

        // Merge overlaps
        pieces = Self::merge_overlaps(pieces);

        // Rank: anchor first, then same-dir, then by normalized path+line
        pieces.sort_by_key(|p| {
            let is_anchor = col
                .anchor_file
                .as_deref()
                .map(|af| {
                    Self::same_file(
                        &env.root,
                        p.file
                            .as_path(),
                        af,
                    )
                })
                .unwrap_or(false) as u8;
            let in_scope = Self::in_anchor_dir(
                &env.root,
                col.anchor_file
                    .as_deref(),
                p.file
                    .as_path(),
            ) as u8;
            let rel = Self::rel(
                &env.root,
                p.file
                    .as_path(),
            )
            .as_ref()
            .to_string_lossy()
            .to_string();
            (
                std::cmp::Reverse(is_anchor),
                std::cmp::Reverse(in_scope),
                rel,
                p.start_line,
            )
        });

        // Build Items
        let mut items: Vec<Item> = Vec::new();
        for p in &pieces
        {
            let is_anchor = col
                .anchor_file
                .as_deref()
                .map(|af| {
                    Self::same_file(
                        &env.root,
                        p.file
                            .as_path(),
                        af,
                    )
                })
                .unwrap_or(false);
            let in_scope = Self::in_anchor_dir(
                &env.root,
                col.anchor_file
                    .as_deref(),
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
                content: Self::render_piece(
                    p,
                    env.args
                        .fence,
                ),
                priority: pr,
                hard: false,
                min_tokens: 64,
            });
        }

        // Template header
        let header = Self::resolve_template_text(
            &env.args
                .template,
            &env.args
                .queries,
        )?;
        let mut all_items = vec![Item {
            id: "__template__".into(),
            content: header,
            priority: Priority::high(),
            hard: true,
            min_tokens: 80,
        }];
        all_items.extend(items);

        // Fail-signal boost
        if !col
            .fail_signals
            .is_empty()
        {
            Self::fail_signal_boost(&mut all_items, &col.fail_signals, &env.root);
        }

        // Call-distance scoring when anchor is available
        if let (Some(anchor_path), Some(anchor_line)) = (&col.anchor_file, col.anchor_line)
            && let Some(anchor_fn) =
                CallGraph::extract_function_name_at(&env.root, anchor_path, anchor_line)
        {
            let hops = CallGraphHopper::collect_callgraph_hops(
                &env.root,
                anchor_path,
                anchor_line,
                &anchor_fn,
                2,
            );

            let w_call = 0.12f32; // Keep ≤ 0.15 for bounded contribution
            for item in &mut all_items
            {
                if let Some((file, start, _end)) = Self::parse_item_id(&item.id, &env.root)
                {
                    let cd = CallGraphHopper::score_from_call_distance_for_span(
                        &env.root,
                        &file,
                        start as usize,
                        &hops,
                        w_call,
                    );

                    if cd > 0.0
                    {
                        let old = item.priority;

                        // Blend with existing priority using bounded boost
                        let level = ((old.level as f32) * (1.0 + cd)).min(255.0) as u8;
                        item.priority = Priority::custom(level, old.relevance, old.proximity);
                    }
                }
            }
        }

        // Budget
        let budgeter = Budgeter::new(&env.model)?;

        // Fit with or without buckets
        let fit = if let Some(bucket_spec) = &env
            .args
            .buckets
        {
            let bucket_caps = parse_bucket_caps(bucket_spec)?;
            let tagged_items: Vec<TaggedItem> = all_items
                .into_iter()
                .map(|item| {
                    let mut t = TaggedItem::from(item);
                    // Minimal tag by extension (see review)
                    let ext = Path::new(
                        t.id.split(':')
                            .next()
                            .unwrap_or(""),
                    )
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                    if matches!(ext, "rs" | "ts" | "tsx" | "h" | "hpp" | "hh")
                    {
                        t.tags
                            .insert(SpanTag::Interface);
                    }
                    else
                    {
                        t.tags
                            .insert(SpanTag::Code);
                    }
                    t
                })
                .collect();
            fit_with_buckets(
                &budgeter,
                tagged_items,
                bucket_caps,
                env.args
                    .novelty_min,
            )?
            .fitted
        }
        else
        {
            let dedupe_config = env
                .args
                .dedupe_threshold
                .map(|thr| {
                    crate::core::budgeter::DedupeConfig {
                        jaccard_threshold: thr.clamp(0.0, 1.0),
                        ..Default::default()
                    }
                });
            budgeter.fit_with_dedupe(all_items, env.budget, dedupe_config)?
        };

        // Build final content (JSON or plain)
        let final_content = if env
            .args
            .json
        {
            let tier_label = env
                .tier_opt
                .map(|t| {
                    match t
                    {
                        Tier::A => "A",
                        Tier::B => "B",
                        Tier::C => "C",
                    }
                });
            let out = JsonContext {
                model: env
                    .model
                    .clone(),
                budget: env.budget,
                total_tokens: fit.total_tokens,
                tier: tier_label,
                effective_limit: env.effective_limit,
                effective_top_per_query: env.effective_top_per_query,
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
            serde_json::to_string(&out)?
        }
        else
        {
            let mut s = String::new();
            for it in &fit.items
            {
                s.push_str(&it.content);
            }
            s
        };

        let first_symbol_name = col
            .chosen
            .first()
            .map(|r| {
                r.symbol
                    .qualified_name
                    .clone()
            });

        Ok(Assembled {
            final_content,
            total_tokens: fit.total_tokens,
            first_symbol_name,
        })
    }

    // =========================== Phase 4 ================================

    fn output_results(
        env: &ContextEnvironment,
        col: &Collected,
        asm: &Assembled,
    ) -> Result<()>
    {
        // Missing index: emit consistent JSON or bail
        if !Path::new(&env.symbols_path).exists() && asm.total_tokens == 0
        {
            if env
                .args
                .json
            {
                let tier_label = env
                    .tier_opt
                    .map(|t| {
                        match t
                        {
                            Tier::A => "A",
                            Tier::B => "B",
                            Tier::C => "C",
                        }
                    });
                let out = serde_json::json!({
                    "model": env.model,
                    "budget": env.budget,
                    "total_tokens": 0,
                    "tier": tier_label,
                    "effective_limit": env.effective_limit,
                    "effective_top_per_query": env.effective_top_per_query,
                    "items": [],
                    "ok": false,
                    "reason": "no_symbols"
                });
                println!("{}", out);
                return Ok(());
            }

            bail!(
                "Symbols file not found: {}. Run 'rup symbols' first (or enable auto-index).",
                env.symbols_path
                    .display()
            );
        }

        // No matches
        if col
            .chosen
            .is_empty()
            && asm.total_tokens == 0
        {
            if env
                .args
                .json
            {
                let tier_label = env
                    .tier_opt
                    .map(|t| {
                        match t
                        {
                            Tier::A => "A",
                            Tier::B => "B",
                            Tier::C => "C",
                        }
                    });
                let out = serde_json::json!({
                    "model": env.model,
                    "budget": env.budget,
                    "total_tokens": 0,
                    "tier": tier_label,
                    "effective_limit": env.effective_limit,
                    "effective_top_per_query": env.effective_top_per_query,
                    "items": [],
                    "ok": false,
                    "reason": "no_matches"
                });
                println!("{}", out);
                return Ok(());
            }
            bail!(
                "No symbols matched queries: {:?}",
                env.args
                    .queries
            );
        }

        // Emit
        print!("{}", asm.final_content);

        // Token summary
        if !env
            .args
            .json
            && !env
                .ctx
                .quiet
        {
            eprintln!("\n— total tokens: {} / {}", asm.total_tokens, env.budget);
        }

        // Clipboard (optional)
        if env
            .args
            .clipboard
        {
            Self::copy_to_clipboard(&asm.final_content)?;
            if !env
                .ctx
                .quiet
            {
                eprintln!("Copied to clipboard");
            }
        }

        // History
        if let Some(name) = &asm.first_symbol_name
        {
            Self::save_history(
                env.root
                    .join(".rup/context_history"),
                name,
            )
            .ok();
        }

        Ok(())
    }
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

pub struct CallgraphSpec
{
    pub anchor: Option<(PathBuf, usize)>,
    pub depth: u8,
    pub files_per_hop: usize,
    pub edges_limit: usize,
}

pub struct CallGraph;

impl CallGraph
{
    /// Bounded callgraph collection with constraints and file caching
    pub fn collect_callgraph_names_bounded(
        root: &Path,
        spec: &CallgraphSpec,
    ) -> Vec<String>
    {
        let Some((anchor_path, anchor_line)) = &spec.anchor
        else
        {
            return Vec::new();
        };
        let Some(anchor_fn) = Self::extract_function_name_at(root, anchor_path, *anchor_line)
        else
        {
            return Vec::new();
        };

        // BFS state with bounds enforcement
        let mut out = BTreeSet::new();
        let mut q: VecDeque<(String, PathBuf, usize, u8)> =
            VecDeque::from([(anchor_fn.clone(), anchor_path.clone(), *anchor_line, 0)]);
        let mut edges_used = 0usize;
        let mut files_seen_this_hop = 0usize;
        let mut last_hop = 0u8;

        // File content cache to avoid repeated reads
        let mut cache: HashMap<PathBuf, String> = Default::default();

        while let Some((_name, path, line, d)) = q.pop_front()
        {
            // Reset file counter when hop depth increases
            if d > last_hop
            {
                files_seen_this_hop = 0;
                last_hop = d;
            }

            // Apply bounds: depth, total edges, files per hop
            if d >= spec.depth || edges_used >= spec.edges_limit
            {
                continue;
            }
            if files_seen_this_hop >= spec.files_per_hop
            {
                continue;
            }
            files_seen_this_hop += 1;

            // Cached file read
            let full_path = root.join(&path);
            let text = if let Some(t) = cache.get(&full_path)
            {
                t
            }
            else
            {
                let content = StdFs::read_to_string(&full_path).unwrap_or_default();

                cache.insert(full_path.clone(), content);
                cache
                    .get(&full_path)
                    .unwrap()
            };

            let lines: Vec<&str> = text
                .lines()
                .collect();
            if lines.is_empty()
            {
                continue;
            }

            let idx = line
                .saturating_sub(1)
                .min(
                    lines
                        .len()
                        .saturating_sub(1),
                );
            let lo = idx.saturating_sub(DEFAULT_SCAN_WINDOW);
            let hi = (idx + DEFAULT_SCAN_WINDOW).min(
                lines
                    .len()
                    .saturating_sub(1),
            );

            // Extract function calls from the window
            let mut names = BTreeSet::new();
            for scan_line in &lines[lo..=hi]
            {
                let mut j = 0usize;
                let b = scan_line.as_bytes();

                while j < b.len()
                {
                    if let Some((name, k)) = Self::take_ident(b, j)
                    {
                        let mut m = k;
                        while m < b.len() && Self::is_space(Some(b[m]))
                        {
                            m += 1;
                        }

                        // Check for function call pattern
                        if m < b.len()
                            && b[m] == b'('
                            && name != "if"
                            && name != "for"
                            && name != "while"
                            && name != "match"
                        {
                            names.insert(name);
                        }
                        j = k + 1;
                        continue;
                    }
                    j += 1;
                }
            }

            // Add discovered calls to queue, respecting edge limit
            for c in names
            {
                if edges_used >= spec.edges_limit
                {
                    break;
                }
                if out.insert(c.clone())
                {
                    q.push_back((c, path.clone(), line, d + 1));
                    edges_used += 1;
                }
            }
        }

        out.into_iter()
            .collect()
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
            if let Some(calls) = Self::scan_calls_near(root, &path, line, DEFAULT_SCAN_WINDOW)
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
        let text = StdFs::read_to_string(root.join(path)).ok()?;
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

        for code_line in &lines[lo..=hi]
        {
            let mut j = 0usize;
            let b = code_line.as_bytes();

            while j < b.len()
            {
                if let Some((name, k)) = Self::take_ident(b, j)
                {
                    let mut m = k;

                    while m < b.len() && Self::is_space(Some(b[m]))
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

    /// Parse callgraph arg "anchor=path:line depth=N files_per_hop=N edges=N".
    pub fn parse_callgraph_arg(
        raw: Option<&str>,
        fallback_path: Option<&PathBuf>,
        fallback_line: Option<usize>,
    ) -> Option<CallgraphSpec>
    {
        let mut depth: u8 = 1;
        let mut files_per_hop: usize = DEFAULT_FILES_PER_HOP;
        let mut edges_limit: usize = DEFAULT_EDGES_LIMIT;
        let mut anchor: Option<(PathBuf, usize)> = None;

        if let Some(s) = raw
        {
            for token in s.split_whitespace()
            {
                if let Some(rest) = token.strip_prefix("depth=")
                {
                    if let Ok(n) = rest.parse::<u8>()
                    {
                        depth = n.clamp(1, MAX_CALLGRAPH_DEPTH); // Clamp for SLA
                    }
                }
                else if let Some(rest) = token.strip_prefix("files_per_hop=")
                {
                    if let Ok(n) = rest.parse::<usize>()
                    {
                        files_per_hop = n.clamp(1, 50);
                    }
                }
                else if let Some(rest) = token.strip_prefix("edges=")
                {
                    if let Ok(n) = rest.parse::<usize>()
                    {
                        edges_limit = n.clamp(50, 1000);
                    }
                }
                else if let Some(rest) = token.strip_prefix("anchor=")
                    && let Some((p, l)) = Self::parse_path_line(rest)
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

        Some(CallgraphSpec { anchor, depth, files_per_hop, edges_limit })
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
        let text = StdFs::read_to_string(root.join(path)).ok()?;
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
        let lo = idx.saturating_sub(FUNCTION_SEARCH_WINDOW);
        let hi = (idx + FUNCTION_SEARCH_WINDOW).min(
            lines
                .len()
                .saturating_sub(1),
        );

        for i in (lo..=idx).rev()
        {
            if let Some(name) = Self::parse_fn_decl(lines[i])
            {
                return Some(name);
            }
        }

        for code_line in &lines[idx..=hi]
        {
            if let Some(name) = Self::parse_fn_decl(code_line)
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
                && Self::is_space(
                    bytes
                        .get(i + 2)
                        .copied(),
                )
            {
                let mut j = i + 2;

                while j < bytes.len() && Self::is_space(Some(bytes[j]))
                {
                    j += 1;
                }

                let (name, k) = Self::take_ident(bytes, j)?;

                let mut m = k;

                while m < bytes.len() && Self::is_space(Some(bytes[m]))
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
}

pub struct CallGraphHopper;

impl CallGraphHopper
{
    /// Collects callgraph hops from an anchor function up to a specified depth using
    /// BStdFs.
    ///
    /// # Arguments
    ///
    /// * `root` - Root of the repository.
    /// * `anchor_path` - File path containing the anchor function.
    /// * `anchor_line` - 1-based line number inside the anchor function.
    /// * `anchor_fn` - Name of the anchor function.
    /// * `depth` - Maximum expansion depth.
    ///
    /// # Returns
    ///
    /// A map from function name to its minimum observed hop distance from the anchor.
    pub fn collect_callgraph_hops(
        root: &Path,
        anchor_path: &Path,
        anchor_line: usize,
        anchor_fn: &str,
        depth: u8,
    ) -> BTreeMap<String, u8>
    {
        // Map from function name to its minimum observed distance.
        let mut hops: BTreeMap<String, u8> = BTreeMap::new();

        // Work queue for BStdFs: (name, path, line, hop).
        let mut queue: VecDeque<(String, PathBuf, usize, u8)> = VecDeque::new();

        // Seed the queue with the anchor at hop 0.
        queue.push_back((
            anchor_fn.to_string(),
            anchor_path.to_path_buf(),
            anchor_line,
            0,
        ));

        // Standard BStdFs over cheap callsite scans
        while let Some((name, path, line, h)) = queue.pop_front()
        {
            // If we already have an equal or better hop skip expansion
            if let Some(&best) = hops.get(&name)
                && best <= h
            {
                continue;
            }

            // Record the best hop for this function name.
            hops.insert(name.clone(), h);

            // Stop expanding once the depth budget is reached.
            if h >= depth
            {
                continue;
            }

            // Scan for calles near the current locus
            if let Some(mut calls) =
                CallGraph::scan_calls_near(root, &path, line, DEFAULT_CALL_SCAN_WINDOW)
            {
                // Sort to keep traversal deterministic
                calls.sort();

                // Enqueue each callee with hop + 1 at the same locus
                for c in calls
                {
                    queue.push_back((c, path.clone(), line, h + 1));
                }
            }
        }

        // Ensure the anchor itself is present with hop 0
        hops.entry(anchor_fn.to_string())
            .or_insert(0);

        // Return the hop map for downstream scoring
        hops
    }

    // ================= hop-affinity transform + bounded weight ===================

    /// Turn a hop count into a [0, 1] affinity with smooth decay
    /// hop = 0 -> 1.0, hop=1 -> 0.5, hop=2 -> 0.3333, etc.
    /// This is simple, monotone, and numerically stable.
    #[inline(always)]
    pub fn call_distance_from_hop(hop: u8) -> f32
    {
        1.0f32 / (1.0f32 + hop as f32)
    }

    /// Computes a score for a function based on its callgraph distance from the anchor.
    ///
    /// # Arguments
    /// - `fn_name`: Function name whose span is being ranked.
    /// - `hops`: Precomputed function->hop map from the anchor.
    /// - `w_call`: Requested weight (will be clamped to [0.0, 0.15]).
    ///
    /// # Returns
    /// The bounded contribution to the score.
    pub fn score_from_call_distance_for_fn(
        // Function name whose span is being ranked.
        fn_name: &str,
        // Precomputed function->hop map from the anchor.
        hops: &BTreeMap<String, u8>,
        // Requested weight (will be clamped to [0.0, 0.15])
        w_call: f32,
    ) -> f32
    {
        // Look up the minimum hop for this function name.
        let hop = match hops.get(fn_name)
        {
            Some(&h) => h,

            None => return 0.0,
        };

        // Convert hop to affinity in [0, 1]
        let affinity = Self::call_distance_from_hop(hop);

        // Clamp the requested weight to a conservative maximum.
        let w = w_call.clamp(0.0, 0.15);

        // Return the bounded contribution.
        affinity * w
    }

    /// Computes a score for a span based on its callgraph distance from the anchor.
    /// If the owner function name is not known at the call site, it is derived from
    /// (path, line). If no function is found, returns 0.0.
    ///
    /// # Arguments
    /// - `root`: Repository root for file reads.
    /// - `path`: Path of the file that contains the span.
    /// - `line`: 1-based line number inside the file for the span.
    /// - `hops`: Precomputed function->hop map from the anchor.
    /// - `w_call`: Requested weight (will be clamped to [0.0, 0.15]).
    ///
    /// # Returns
    /// The bounded contribution to the score.
    pub fn score_from_call_distance_for_span(
        root: &Path,
        path: &Path,
        line: usize,
        hops: &BTreeMap<String, u8>,
        w_call: f32,
    ) -> f32
    {
        // Try to extract the nearest enclosing function name
        let owner = match CallGraph::extract_function_name_at(root, path, line)
        {
            Some(n) => n,

            None => return 0.0,
        };

        // Delegate to the function-name scoring helper
        Self::score_from_call_distance_for_fn(&owner, hops, w_call)
    }
}
