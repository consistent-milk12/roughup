//! Filepath: src/core/budgeter.rs

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use moka::sync::Cache;
use tiktoken_rs::{CoreBPE, cl100k_base, get_bpe_from_model, o200k_base};
use xxhash_rust::xxh64::Xxh64;

/// Enhanced priority system with fine-grained scoring
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priority
{
    /// Primary priority level (0-255, higher = more important)
    pub level: u8,
    /// Secondary relevance score (0.0-1.0, higher = more relevant)
    pub relevance: f32,
    /// Tertiary proximity score (0.0-1.0, higher = closer to anchor)
    pub proximity: f32,
}

impl Priority
{
    /// Create a high priority with perfect relevance and proximity
    pub const fn high() -> Self
    {
        Self { level: 200, relevance: 1.0, proximity: 1.0 }
    }

    /// Create a medium priority with good relevance
    pub const fn medium() -> Self
    {
        Self { level: 100, relevance: 0.7, proximity: 0.5 }
    }

    /// Create a low priority with minimal relevance
    pub const fn low() -> Self
    {
        Self { level: 50, relevance: 0.3, proximity: 0.1 }
    }

    /// Create a custom priority with all dimensions, NaN-safe
    pub fn custom(
        level: u8,
        relevance: f32,
        proximity: f32,
    ) -> Self
    {
        fn sane(x: f32) -> f32
        {
            if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
        }
        Self {
            level,
            relevance: sane(relevance),
            proximity: sane(proximity),
        }
    }

    /// Calculate final composite score for ranking
    pub fn composite_score(&self) -> f64
    {
        // Weighted combination: level is primary, relevance secondary, proximity tertiary
        (self.level as f64) * 1000.0
            + (self.relevance as f64) * 100.0
            + (self.proximity as f64) * 10.0
    }
}

impl PartialOrd for Priority
{
    fn partial_cmp(
        &self,
        other: &Self,
    ) -> Option<std::cmp::Ordering>
    {
        Some(self.cmp(other))
    }
}

impl Eq for Priority {}

// Natural ascending order - descending handled at call sites
impl std::cmp::Ord for Priority
{
    fn cmp(
        &self,
        other: &Self,
    ) -> std::cmp::Ordering
    {
        self.level
            .cmp(&other.level)
            .then_with(|| {
                self.relevance
                    .total_cmp(&other.relevance)
            })
            .then_with(|| {
                self.proximity
                    .total_cmp(&other.proximity)
            })
    }
}

// Maintain backward compatibility with the old enum values
impl From<Priority> for u8
{
    fn from(priority: Priority) -> Self
    {
        match priority.level
        {
            200..=255 => 2, // High
            100..=199 => 1, // Medium
            _ => 0,         // Low
        }
    }
}

/// A context item candidate to budget
#[derive(Debug, Clone)]
pub struct Item
{
    /// Unique identifier for the item (e.g., file:line-span or qualified name)
    pub id: String,

    /// Body text of the item
    pub content: String,

    /// Selection priority for budgeting
    pub priority: Priority,

    /// Whether this item must be included (trim if necessary)
    pub hard: bool,

    /// Minimal tokens we should try to keep for this item
    pub min_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct FitResult
{
    /// Items that fit within the budget
    pub items: Vec<FittedItem>,

    /// Total number of tokens used by fitted items
    pub total_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct FittedItem
{
    /// Original, full body (immutable source for all future trims)
    pub full_content: String,
    /// The trimmed or fitted content for this item
    pub content: String,

    /// Unique identifier for the item
    pub id: String,

    /// Number of tokens used by this fitted content
    pub tokens: usize,
}

/// Budget manager backed by tiktoken-rs with token caching
pub struct Budgeter
{
    /// Byte Pair Encoding (BPE) tokenizer for counting tokens
    bpe: CoreBPE,

    /// Token count cache for fast repeated queries
    cache: Cache<u64, usize>,
}

impl Budgeter
{
    /// Create a new Budgeter for a given model or encoding name.
    ///
    /// Supported values include model names (e.g., "gpt-3.5-turbo", "gpt-4") or encoding
    /// names ("cl100k_base", "o200k_base"). Falls back to encoding names if model
    /// lookup fails.
    ///
    /// # Arguments
    /// * `model_or_encoding` - Model or encoding name (case-insensitive).
    ///
    /// # Errors
    /// Returns an error if the model or encoding is unsupported or cannot be loaded.
    pub fn new(model_or_encoding: &str) -> Result<Self>
    {
        let lower = model_or_encoding.to_ascii_lowercase();

        // Try to get BPE from model name first, fallback to encoding name.
        let bpe = match get_bpe_from_model(&lower)
        {
            Ok(b) => b,
            Err(_) =>
            {
                match lower.as_str()
                {
                    "o200k_base" => o200k_base().context("load o200k_base")?,
                    "cl100k_base" => cl100k_base().context("load cl100k_base")?,
                    _ => return Err(anyhow!("Unsupported model/encoding: {model_or_encoding}")),
                }
            }
        };

        // Create Budgeter with a token count cache of 100,000 entries.
        Ok(Self { bpe, cache: Cache::new(100_000) })
    }

    /// Count the number of tokens in the given string, using cache for efficiency.
    /// Uses xxhash64 to hash the string as cache key.
    pub fn count(
        &self,
        s: &str,
    ) -> usize
    {
        // Create a new xxhash64 hasher with seed 0
        let mut hasher = Xxh64::new(0);

        // Feed the string bytes into the hasher
        hasher.update(s.as_bytes());

        // Get the hash digest as cache key
        let key = hasher.digest();

        // Check if the token count is already cached
        if let Some(t) = self
            .cache
            .get(&key)
        {
            return t;
        }

        // Otherwise, encode and count tokens
        let t = self
            .bpe
            .encode_ordinary(s)
            .len();

        // Insert the result into cache
        self.cache
            .insert(key, t);

        t
    }

    /// Fit items into `budget_tokens` deterministically with trimming.
    /// Fit items into `budget_tokens` deterministically with trimming.
    ///
    /// - Items are sorted by priority (descending) and id (ascending) for deterministic
    ///   selection.
    /// - "Hard" items (must be included) are reserved first with their minimal token
    ///   requirement.
    /// - Non-hard items are added fully if they fit, otherwise trimmed if possible.
    /// - If minimal hard items couldn't be placed, attempts to trim existing hard items.
    /// - As a last resort, trims from the lowest priority tail to fit within the budget.
    pub fn fit(
        &self,
        items: Vec<Item>,
        budget_tokens: usize,
    ) -> Result<FitResult>
    {
        self.fit_with_dedupe(items, budget_tokens, None)
    }

    /// Fit items with optional deduplication applied first
    pub fn fit_with_dedupe(
        &self,
        items: Vec<Item>,
        budget_tokens: usize,
        dedupe_config: Option<DedupeConfig>,
    ) -> Result<FitResult>
    {
        // Ensure deterministic order before dedupe to avoid "first kept"
        // depending on upstream caller ordering.
        let mut items = sort_items_stable(items);

        // Apply deduplication after the deterministic sort so that "winner"
        // selection uses (priority desc, id asc) as a tie-breaker.
        if let Some(config) = dedupe_config
        {
            let dedupe_engine = DedupeEngine::with_config(config);
            items = dedupe_engine.dedupe_items_with_budgeter(items, self);
        }
        // Deterministic order: by (priority desc, id asc)
        items.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.id.cmp(&b.id))
        });

        let mut out: Vec<FittedItem> = Vec::new();
        let mut remaining = budget_tokens;

        // 1) Reserve hard items minimally
        let hard_first: Vec<Item> = items
            .iter()
            .filter(|&i| i.hard)
            .cloned()
            .collect();

        for it in &hard_first
        {
            let need = it
                .min_tokens
                .max(1);

            if remaining < need
            {
                // Not enough budget for this hard item; will try to trim later
                continue;
            }

            let (s, tok) = self.take_prefix(&it.content, need);

            out.push(FittedItem {
                id: it
                    .id
                    .clone(),
                full_content: it
                    .content
                    .clone(),
                content: s,
                tokens: tok,
            });

            remaining = remaining.saturating_sub(tok);
        }

        // 2) Add non-hard items fully while they fit
        for it in items.into_iter()
        {
            if hard_first
                .iter()
                .any(|h| h.id == it.id)
            {
                continue;
            }
            let tok = self.count(&it.content);
            if tok <= remaining
            {
                // Whole item fits in the remaining budget
                out.push(FittedItem {
                    id: it.id,
                    full_content: it
                        .content
                        .clone(),
                    content: it.content,
                    tokens: tok,
                });
                remaining -= tok;
            }
            else if it.min_tokens > 0 && remaining >= it.min_tokens
            {
                // If full item doesn't fit but min_tokens can, fit exactly min_tokens
                // (or remaining if smaller), and do not break if budget remains.
                let want = it
                    .min_tokens
                    .min(remaining);
                let (s, t) = self.take_prefix(&it.content, want);
                out.push(FittedItem {
                    id: it.id,
                    full_content: it
                        .content
                        .clone(),
                    content: s,
                    tokens: t,
                });
                remaining = remaining.saturating_sub(t);
                // Continue to try additional items, preserving priority order.
                if remaining == 0
                {
                    break;
                }
            }
        }

        // 2.5) Hard item reconciliation: ensure ALL hard items are present
        use std::collections::HashMap;
        let meta_hard: HashMap<&str, usize> = hard_first
            .iter()
            .map(|h| {
                (
                    h.id.as_str(),
                    h.min_tokens
                        .max(1),
                )
            })
            .collect();

        // OWN the ids so we don't borrow from `out`
        let mut present: HashSet<String> = out
            .iter()
            .map(|fi| {
                fi.id
                    .clone()
            })
            .collect();

        for h in &hard_first
        {
            // already present?
            if present.contains(&h.id)
            {
                continue;
            }

            let need = meta_hard[&h
                .id
                .as_str()];

            // free space: drop non-hard tails first (update `present`)
            while remaining < need
            {
                if let Some(pos) = out
                    .iter()
                    .rposition(|fi| {
                        !meta_hard.contains_key(
                            fi.id
                                .as_str(),
                        )
                    })
                {
                    remaining = remaining.saturating_add(out[pos].tokens);
                    let removed_id = out[pos]
                        .id
                        .clone();
                    out.remove(pos);
                    present.remove(&removed_id);
                }
                else
                {
                    break;
                }
            }

            // still not enough? trim existing hard items down to their mins
            if remaining < need
            {
                for j in (0..out.len()).rev()
                {
                    let id = out[j]
                        .id
                        .as_str();
                    if let Some(min_need) = meta_hard.get(id)
                        && out[j].tokens > *min_need
                    {
                        let want = *min_need;
                        let (new_text, new_tok) = self.take_prefix(&out[j].full_content, want);
                        remaining = remaining.saturating_add(out[j].tokens - new_tok);
                        out[j].content = new_text;
                        out[j].tokens = new_tok;
                    }

                    if remaining >= need
                    {
                        break;
                    }
                }
            }

            // last resort: drop another NON-HARD tail if any (never drop hard)
            if remaining < need
                && let Some(pos) = out
                    .iter()
                    .rposition(|fi| {
                        !meta_hard.contains_key(
                            fi.id
                                .as_str(),
                        )
                    })
            {
                let last = out.remove(pos);
                remaining = remaining.saturating_add(last.tokens);
                present.remove(&last.id);
            }

            // place the missing hard if we have room
            if remaining >= need
            {
                let (s, t) = self.take_prefix(&h.content, need);
                out.push(FittedItem {
                    id: h
                        .id
                        .clone(),
                    full_content: h
                        .content
                        .clone(),
                    content: s,
                    tokens: t,
                });
                present.insert(h.id.clone());
                remaining = remaining.saturating_sub(t);
            }
        }
        // end reconciliation

        // 3) If we couldn't place minimal hard pieces earlier, attempt to trim existing hard
        //    items
        if remaining == 0
        {
            // Ensure total <= budget (guard against rounding)
            let tot: usize = out
                .iter()
                .map(|x| x.tokens)
                .sum();
            if tot <= budget_tokens
            {
                return Ok(FitResult { items: out, total_tokens: tot });
            }
        }

        // After step 1, expand hard items toward full content in priority order
        for it in &hard_first
        {
            if remaining == 0
            {
                break;
            }
            // Find the fitted entry we already pushed for this id
            if let Some(fi) = out
                .iter_mut()
                .find(|fi| fi.id == it.id)
            {
                let full_tok = self.count(&fi.full_content);
                if fi.tokens < full_tok
                {
                    let want = (fi.tokens + remaining).min(full_tok);
                    let (s, t) = self.take_prefix(&fi.full_content, want);
                    // Increase only if we gained tokens
                    if t > fi.tokens
                    {
                        remaining -= t - fi.tokens;
                        fi.tokens = t;
                        fi.content = s;
                    }
                }
            }
        }

        // After assembling `out`, ensure we do not exceed the budget
        let mut total_tokens: usize = out
            .iter()
            .map(|x| x.tokens)
            .sum();
        if total_tokens > budget_tokens
        {
            // Map of id -> (is_hard, min_tokens)
            let mut meta: HashMap<&str, (bool, usize)> = HashMap::new();
            for h in &hard_first
            {
                meta.insert(
                    h.id.as_str(),
                    (
                        true,
                        h.min_tokens
                            .max(1),
                    ),
                );
            }

            // 1) Drop non-hard items from the end until we're under budget or none left
            let mut idx = out.len();
            while total_tokens > budget_tokens && idx > 0
            {
                let k = idx - 1;
                let id = out[k]
                    .id
                    .as_str();

                let is_hard = meta
                    .get(id)
                    .map(|m| m.0)
                    .unwrap_or(false);

                if !is_hard
                {
                    total_tokens = total_tokens.saturating_sub(out[k].tokens);
                    out.remove(k);
                }

                idx = k;
            }

            // 2) If still over budget, trim hard items (never below min_tokens)
            if total_tokens > budget_tokens
            {
                // Build a vector of (index, current_tokens, min_tokens)
                let mut hard_indices: Vec<(usize, usize, usize)> = out
                    .iter()
                    .enumerate()
                    .filter_map(|(j, fi)| {
                        meta.get(
                            fi.id
                                .as_str(),
                        )
                        .map(|(_, min)| (j, fi.tokens, *min))
                    })
                    .collect();

                // Trim from lowest-priority tail first (out is already in priority order)
                hard_indices.reverse();

                let mut excess = total_tokens - budget_tokens;
                for (j, cur, min_needed) in hard_indices
                {
                    if excess == 0
                    {
                        break;
                    }
                    if cur > min_needed
                    {
                        let reducible = cur - min_needed;
                        let cut = reducible.min(excess);

                        // Re-trim this item from full_content (not from current content)
                        let want = cur - cut;
                        let (new_text, new_tok) = self.take_prefix(&out[j].full_content, want);
                        out[j].content = new_text;
                        out[j].tokens = new_tok;

                        excess -= cut;
                        total_tokens -= cut;
                    }
                }

                // If still over budget after all trims, drop the smallest tail item
                if total_tokens > budget_tokens && !out.is_empty()
                {
                    // Remove the last item; this keeps determinism straightforward
                    let last = out
                        .pop()
                        .unwrap();
                    total_tokens = total_tokens.saturating_sub(last.tokens);
                }
            }
        }

        let total_tokens = total_tokens;

        Ok(FitResult { items: out, total_tokens })
    }

    /// Return a prefix with at most `max_tokens` tokens, with a clean ellipsis boundary
    /// Reserve 1 "token slot" for ellipsis if we must trim, so our
    /// emitted tokens never exceed max_tokens due to the "…\n".
    fn take_prefix(
        &self,
        s: &str,
        max_tokens: usize,
    ) -> (String, usize)
    {
        if max_tokens == 0
        {
            return (String::new(), 0);
        }

        // tokenize once
        let ids = self
            .bpe
            .encode_ordinary(s);

        // fits without trim
        if ids.len() <= max_tokens
        {
            return (s.to_string(), ids.len());
        }

        // sentinel ensures a hard boundary; newline guards against BPE merges
        let ellipsis_ids = self
            .bpe
            .encode_ordinary("\n…\n");
        let e = ellipsis_ids.len();

        if max_tokens <= e
        {
            // we can't afford any prefix; show as much of the sentinel as fits
            let take = &ellipsis_ids[..max_tokens];
            let out = self
                .bpe
                .decode(take.to_vec())
                .unwrap_or_default();
            return (out, max_tokens);
        }

        let cap = max_tokens - e;
        let mut combined = Vec::with_capacity(cap + e);
        combined.extend_from_slice(&ids[..cap]);
        combined.extend_from_slice(&ellipsis_ids);

        let out = self
            .bpe
            .decode(combined.clone())
            .unwrap_or_default();
        (out, cap + e)
    }
}

/// Symbol relevance calculator for context assembly
#[derive(Debug, Clone)]
pub struct SymbolRanker
{
    /// Anchor file for proximity calculations
    anchor_file: Option<std::path::PathBuf>,
    /// Anchor line for fine-grained proximity
    anchor_line: Option<usize>,
}

impl SymbolRanker
{
    /// Create a new symbol ranker with optional anchor
    pub fn new(
        anchor_file: Option<&std::path::Path>,
        anchor_line: Option<usize>,
    ) -> Self
    {
        Self {
            anchor_file: anchor_file.map(|p| p.to_path_buf()),
            anchor_line,
        }
    }

    /// Calculate priority for a symbol based on multiple factors
    pub fn calculate_priority(
        &self,
        symbol: &crate::core::symbols::Symbol,
        query: &str,
        context_factors: &ContextFactors,
    ) -> Priority
    {
        let level = self.calculate_level(symbol, context_factors);
        let relevance = self.calculate_relevance(symbol, query);
        let proximity = self.calculate_proximity(symbol);

        Priority::custom(level, relevance, proximity)
    }

    /// Calculate base priority level (0-255)
    fn calculate_level(
        &self,
        symbol: &crate::core::symbols::Symbol,
        factors: &ContextFactors,
    ) -> u8
    {
        let mut score = 100u8; // Start with medium baseline

        // Boost for public symbols (API surface)
        if matches!(
            symbol.visibility,
            Some(crate::core::symbols::Visibility::Public)
        )
        {
            score = score.saturating_add(30);
        }

        // Boost for important symbol kinds
        match symbol.kind
        {
            crate::core::symbols::SymbolKind::Function => score = score.saturating_add(20),
            crate::core::symbols::SymbolKind::Class | crate::core::symbols::SymbolKind::Struct =>
            {
                score = score.saturating_add(25)
            }
            crate::core::symbols::SymbolKind::Module => score = score.saturating_add(15),
            _ =>
            {}
        }

        // Boost for symbols in anchor file
        if let Some(ref anchor) = self.anchor_file
            && symbol
                .file
                .ends_with(anchor)
        {
            score = score.saturating_add(50);
        }

        // Boost for recently accessed symbols
        if factors.recently_accessed
        {
            score = score.saturating_add(20);
        }

        // Penalty for test files (usually less relevant for context)
        if symbol
            .file
            .to_string_lossy()
            .contains("test")
        {
            score = score.saturating_sub(30);
        }

        score
    }

    /// Calculate relevance score based on query match quality (0.0-1.0)
    fn calculate_relevance(
        &self,
        symbol: &crate::core::symbols::Symbol,
        query: &str,
    ) -> f32
    {
        let name = &symbol.name;
        let qualified_name = symbol
            .qualified_name
            .as_str();

        // Exact match is perfect
        if name == query || qualified_name == query
        {
            return 1.0;
        }

        // Prefix match is very good
        if name.starts_with(query) || qualified_name.starts_with(query)
        {
            return 0.9;
        }

        // Substring match is good
        if name.contains(query) || qualified_name.contains(query)
        {
            return 0.7;
        }

        // Case-insensitive fuzzy matching
        let query_lower = query.to_lowercase();
        let name_lower = name.to_lowercase();
        let qualified_lower = qualified_name.to_lowercase();

        if name_lower.contains(&query_lower) || qualified_lower.contains(&query_lower)
        {
            return 0.5;
        }

        // Fuzzy/partial matching (simple implementation)
        let score = fuzzy_match_score(&query_lower, &name_lower)
            .max(fuzzy_match_score(&query_lower, &qualified_lower));

        score * 0.4 // Scale down fuzzy matches
    }

    /// Calculate proximity score based on location relative to anchor (0.0-1.0)
    fn calculate_proximity(
        &self,
        symbol: &crate::core::symbols::Symbol,
    ) -> f32
    {
        let Some(ref anchor_file) = self.anchor_file
        else
        {
            return 0.5; // No anchor = neutral proximity
        };

        // Same file = highest proximity
        if symbol
            .file
            .ends_with(anchor_file)
        {
            // If we have anchor line, consider line distance
            if let Some(anchor_line) = self.anchor_line
            {
                let line_distance = (symbol.start_line as i32 - anchor_line as i32).abs() as f32;
                // Closer lines get higher scores (allow deeper range as suggested)
                return (1.0_f32 - (line_distance / 1000.0).min(0.6)).max(0.4);
            }
            return 1.0;
        }

        // Same directory = good proximity
        if let (Some(symbol_parent), Some(anchor_parent)) = (
            symbol
                .file
                .parent(),
            anchor_file.parent(),
        )
        {
            if symbol_parent == anchor_parent
            {
                return 0.7;
            }

            // Calculate directory distance
            let symbol_components: Vec<_> = symbol_parent
                .components()
                .collect();
            let anchor_components: Vec<_> = anchor_parent
                .components()
                .collect();

            let common_prefix = symbol_components
                .iter()
                .zip(anchor_components.iter())
                .take_while(|(a, b)| a == b)
                .count();

            let total_depth = symbol_components
                .len()
                .max(anchor_components.len());
            if total_depth > 0
            {
                return (common_prefix as f32 / total_depth as f32) * 0.6;
            }
        }

        0.2 // Different directories = low proximity
    }
}

/// Additional context factors for ranking
#[derive(Debug, Clone, Default)]
pub struct ContextFactors
{
    /// Whether this symbol was recently accessed
    pub recently_accessed: bool,
    /// Template context (refactor, bugfix, feature)
    pub template: Option<crate::cli::ContextTemplate>,
    /// Current phase of development
    pub development_phase: Option<DevelopmentPhase>,
}

/// Development phase affects symbol importance
#[derive(Debug, Clone, Copy)]
pub enum DevelopmentPhase
{
    /// Early development - focus on core types and public APIs
    Early,
    /// Feature development - focus on implementation details
    Feature,
    /// Bug fixing - focus on error handling and edge cases
    Bugfix,
    /// Refactoring - focus on structure and organization
    Refactor,
}

/// Simple fuzzy matching score (0.0-1.0)
fn fuzzy_match_score(
    pattern: &str,
    text: &str,
) -> f32
{
    if pattern.is_empty() || text.is_empty()
    {
        return 0.0;
    }

    let mut pattern_chars = pattern
        .chars()
        .peekable();
    let mut text_chars = text.chars();
    let mut matches = 0;
    let mut consecutive_matches = 0;
    let mut max_consecutive = 0;

    while let Some(pattern_char) = pattern_chars.peek()
    {
        if let Some(text_char) = text_chars.next()
        {
            if *pattern_char == text_char
            {
                pattern_chars.next();
                matches += 1;
                consecutive_matches += 1;
                max_consecutive = max_consecutive.max(consecutive_matches);
            }
            else
            {
                consecutive_matches = 0;
            }
        }
        else
        {
            break;
        }
    }

    if matches == 0
    {
        return 0.0;
    }

    // Score based on match ratio with bonus for consecutive matches
    let match_ratio = matches as f32 / pattern.len() as f32;
    let consecutive_bonus = (max_consecutive as f32 / pattern.len() as f32) * 0.3;

    (match_ratio + consecutive_bonus).min(1.0)
}

// Sorts items by (priority desc, id asc) deterministically before
// deduplication, so "first kept" is stable across OS/arch and runs.
fn sort_items_stable(mut items: Vec<Item>) -> Vec<Item>
{
    // Stable sort by our total order to fix dedupe input order.
    items.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.id.cmp(&b.id))
    });
    items
}

// Chooses which of two near-duplicates to keep deterministically.
// Order: higher priority → (meaningfully) fewer tokens → lower id.
// If token counts differ by less than a small tolerance, treat as equal
// to avoid flipping winners on whitespace-only edits; fall back to id asc.
fn keep_left_if_better(
    a: &Item,
    b: &Item,
    budgeter: &Budgeter,
) -> bool
{
    // 1) priority first (desc)
    match a
        .priority
        .cmp(&b.priority)
    {
        std::cmp::Ordering::Greater => return true,
        std::cmp::Ordering::Less => return false,
        std::cmp::Ordering::Equal =>
        {}
    }

    // 2) token counts with tolerance
    let a_tok = budgeter.count(&a.content);
    let b_tok = budgeter.count(&b.content);

    // small diffs are noise; adjust as needed
    const TOKEN_DELTA_MIN: usize = 4;

    if a_tok + TOKEN_DELTA_MIN <= b_tok
    {
        // a is meaningfully smaller
        return true;
    }
    if b_tok + TOKEN_DELTA_MIN <= a_tok
    {
        // b is meaningfully smaller
        return false;
    }

    // 3) stable tie-break: lower id wins (matches pre-sort by id asc)
    a.id < b.id
}

// Generates winnowed fingerprints using a sliding window over hashed
// n-gram tokens. This accelerates near-duplicate detection and honors
// config.hash_window.
fn fingerprints(
    tokens: &[u64],
    window: usize,
) -> Vec<u64>
{
    // Early return if window is too small or larger than tokens.
    if window == 0 || tokens.is_empty() || window > tokens.len()
    {
        return Vec::new();
    }
    // Winnowing: pick the minimum hash in each window; if ties, pick
    // the rightmost to improve stability under insertions.
    let mut out = Vec::new();
    let mut min_idx = 0usize;
    for i in 0..=tokens
        .len()
        .saturating_sub(window)
    {
        // Recompute min when sliding beyond previous min.
        if min_idx < i
        {
            min_idx = i;
            for j in i..i + window
            {
                if tokens[j] <= tokens[min_idx]
                {
                    min_idx = j;
                }
            }
            out.push(tokens[min_idx]);
            continue;
        }
        // Compare new entrant at the right edge.
        let j = i + window - 1;
        if tokens[j] <= tokens[min_idx]
        {
            min_idx = j;
        }
        // Record the current window minimum.
        out.push(tokens[min_idx]);
    }
    out
}

// Hashes text into u64 shingles with normalization and ngram_size.
// Use a stable, seeded hasher for cross-OS determinism.
fn hashed_shingles(
    text: &str,
    n: usize,
) -> Vec<u64>
{
    // Normalize once to avoid quadratic work.
    let norm = normalize_for_ngrams(text);
    let words: Vec<&str> = norm
        .split_whitespace()
        .collect();
    if words.len() < n
    {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(
        words
            .len()
            .saturating_sub(n)
            + 1,
    );
    for win in words.windows(n)
    {
        let mut h = Xxh64::new(0);
        for w in win
        {
            h.update(w.as_bytes());
            h.update(&[0xFF]);
        }
        out.push(h.digest());
    }
    out
}

// Normalization used by hashed_shingles (mirrors existing normalize,
// but avoids String allocation per line).
fn normalize_for_ngrams(s: &str) -> String
{
    s.lines()
        .map(|line| {
            let line = if let Some(pos) = line.find("//")
            {
                &line[..pos]
            }
            else
            {
                line
            };
            line.trim()
                .replace('\t', " ")
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// Route hashed shingle extraction by mode
fn hashed_shingles_mode(
    text: &str,
    n: usize,
    mode: NgramMode,
) -> Vec<u64>
{
    match mode
    {
        NgramMode::Word => hashed_shingles(text, n),
        NgramMode::Char => hashed_char_ngrams(text, n),
    }
}

// Build u64 hashes for character n-grams over normalized text
// Uses bytes for performance; adequate for source code domains
fn hashed_char_ngrams(
    text: &str,
    n: usize,
) -> Vec<u64>
{
    // Normalize first to collapse whitespace/comments similarly to
    // the word path, keeping determinism
    let norm = normalize_for_ngrams(text);
    let bytes = norm.as_bytes();
    if bytes.len() < n
    {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(bytes.len() - n + 1);
    for win in bytes.windows(n)
    {
        let mut h = Xxh64::new(0);
        h.update(win);
        out.push(h.digest());
    }
    out
}

// Computes SimHash for a text via hashed shingles. Suitable for quick
// Hamming-distance screening on large spans.
fn simhash_64(
    bits: usize,
    shingles: &[u64],
) -> u64
{
    // Limit bits to 64; SimHash vector accumulator.
    let m = bits.min(64);
    let mut acc = vec![0i64; m];

    for &h in shingles
    {
        for (b, acc_val) in acc
            .iter_mut()
            .enumerate()
            .take(m)
        {
            let bit = ((h >> b) & 1) as i64;
            *acc_val += if bit == 1 { 1 } else { -1 };
        }
    }

    // Convert accumulator signs back to bits.
    let mut out = 0u64;

    for (b, val) in acc
        .iter()
        .enumerate()
        .take(m)
    {
        if *val >= 0
        {
            out |= 1u64 << b;
        }
    }

    out
}

// Returns true if two SimHashes are within the Hamming threshold.
fn simhash_close(
    a: u64,
    b: u64,
    max_hd: u32,
) -> bool
{
    (a ^ b).count_ones() <= max_hd
}

// Deduplication Engine v2 (Phase 4 Week 2 A2)
//
// AST-aware shingles with SimHash fallback for duplicate content detection
// Implements Jaccard 4-gram similarity with rolling hash prefilter

/// N-gram granularity selector for similarity calculation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NgramMode
{
    /// Word n-grams (stable across punctuation changes)
    Word,
    /// Character n-grams (more sensitive; boosts DCR on templates)
    Char,
}

/// Configuration for deduplication engine
#[derive(Debug, Clone)]
pub struct DedupeConfig
{
    /// Minimum Jaccard similarity threshold for considering items duplicates
    pub jaccard_threshold: f64,

    /// Size of n-grams for similarity calculation (default: 4)
    pub ngram_size: usize,

    /// Whether to mark interface spans as non-dedupe
    pub preserve_interfaces: bool,

    /// Rolling hash window size for prefiltering
    pub hash_window: usize,

    /// N-gram granularity mode (default: Word for backward compatibility)
    pub ngram_mode: NgramMode,

    /// Whether to enable char n-gram fallback when primary mode is Word
    pub char_fallback: bool,
}

impl Default for DedupeConfig
{
    fn default() -> Self
    {
        Self {
            jaccard_threshold: 0.7, // 70% similarity threshold
            ngram_size: 4,
            preserve_interfaces: true,
            hash_window: 64,
            ngram_mode: NgramMode::Word, // Default to word for backward compatibility
            char_fallback: true,         // default on for production DCR
        }
    }
}

// Helper: Jaccard similarity over u64 shingles
fn jaccard_u64(
    a: &[u64],
    b: &[u64],
) -> f64
{
    use std::collections::HashSet;
    if a.is_empty() && b.is_empty()
    {
        return 1.0;
    }
    let aa: HashSet<u64> = a
        .iter()
        .copied()
        .collect();
    let bb: HashSet<u64> = b
        .iter()
        .copied()
        .collect();
    let inter = aa
        .intersection(&bb)
        .count();
    let union = aa
        .union(&bb)
        .count();
    if union == 0
    {
        0.0
    }
    else
    {
        inter as f64 / union as f64
    }
}

/// Deduplication engine with AST-aware processing
pub struct DedupeEngine
{
    config: DedupeConfig,
}

impl DedupeEngine
{
    /// Create new deduplication engine with default config
    pub fn new() -> Self
    {
        Self { config: DedupeConfig::default() }
    }

    /// Create new deduplication engine with custom config
    pub fn with_config(config: DedupeConfig) -> Self
    {
        Self { config }
    }

    /// Deduplicate items using Jaccard 4-gram similarity
    pub fn dedupe_items(
        &self,
        items: Vec<Item>,
    ) -> Vec<Item>
    {
        if items.len() <= 1
        {
            return items;
        }

        // Enforce deterministic order before dedupe to avoid "first kept"
        // depending on upstream caller ordering.
        let items = sort_items_stable(items);

        let mut seen_hashes = HashSet::new();
        let mut prefiltered = Vec::new();

        // Rolling hash prefilter for exact matches
        for item in items
        {
            let hash = self.compute_rolling_hash(&item.content);
            if !seen_hashes.contains(&hash)
            {
                seen_hashes.insert(hash);
                prefiltered.push(item);
            }
        }

        // Jaccard similarity deduplication
        let mut kept_items = Vec::new();

        for item in prefiltered
        {
            let should_keep = if self.is_interface_item(&item)
                && self
                    .config
                    .preserve_interfaces
            {
                true // Always keep interface items
            }
            else
            {
                !self.is_duplicate(&item, &kept_items)
            };

            if should_keep
            {
                kept_items.push(item);
            }
        }

        kept_items
    }

    /// Deduplicate items with budgeter for priority-aware tie-breaking
    pub fn dedupe_items_with_budgeter(
        &self,
        items: Vec<Item>,
        budgeter: &Budgeter,
    ) -> Vec<Item>
    {
        if items.len() <= 1
        {
            return items;
        }

        // Enforce deterministic order before dedupe to avoid "first kept"
        // depending on upstream caller ordering.
        let items = sort_items_stable(items);

        let mut seen_hashes = HashSet::new();
        let mut prefiltered = Vec::new();

        // Rolling hash prefilter for exact matches
        for item in items
        {
            let hash = self.compute_rolling_hash(&item.content);
            if !seen_hashes.contains(&hash)
            {
                seen_hashes.insert(hash);
                prefiltered.push(item);
            }
        }

        // Jaccard similarity deduplication with priority-aware selection
        let mut kept_items = Vec::new();

        for item in prefiltered
        {
            let should_keep = if self.is_interface_item(&item)
                && self
                    .config
                    .preserve_interfaces
            {
                true // Always keep interface items
            }
            else
            {
                !self.is_duplicate_with_better_selection(&item, &mut kept_items[..], budgeter)
            };

            if should_keep
            {
                kept_items.push(item);
            }
        }

        kept_items
    }

    /// Check if item is an interface/signature (should be preserved)
    fn is_interface_item(
        &self,
        item: &Item,
    ) -> bool
    {
        // Heuristic: items with trait, interface, or signature keywords
        let content = item
            .content
            .to_lowercase();
        content.contains("trait ")
            || content.contains("interface ")
            || content.contains("pub fn ")
            || content.contains("pub struct ")
            || content.contains("pub enum ")
            || content.contains("pub trait ")
    }

    /// Check if item is duplicate of any item in the kept list
    fn is_duplicate(
        &self,
        item: &Item,
        kept_items: &[Item],
    ) -> bool
    {
        for kept in kept_items
        {
            if self.near_duplicate_hashed(&item.content, &kept.content)
            {
                return true;
            }
        }
        false
    }

    /// Check if item is duplicate with priority-aware selection
    pub fn is_duplicate_with_better_selection(
        &self,
        item: &Item,
        kept_items: &mut [Item],
        budgeter: &Budgeter,
    ) -> bool
    {
        for kept in kept_items.iter_mut()
        {
            if self.near_duplicate_hashed(&item.content, &kept.content)
            {
                // Deterministic tie-break
                if keep_left_if_better(item, kept, budgeter)
                {
                    *kept = item.clone();
                }

                return true;
            }
        }

        false
    }

    /// Fast prefilter using windowed fingerprints
    fn should_compare_detailed(
        &self,
        item_hashes: &[u64],
        kept_hashes: &[u64],
    ) -> bool
    {
        let w = self
            .config
            .hash_window;

        // If windowing is disabled or inputs are too small, don't block —
        // allow detailed comparison
        if w == 0
            || item_hashes.is_empty()
            || kept_hashes.is_empty()
            || w > item_hashes.len()
            || w > kept_hashes.len()
        {
            return true;
        }

        let item_fp = fingerprints(item_hashes, w);
        let kept_fp = fingerprints(kept_hashes, w);

        // No signal? Fall back to detailed comparison
        if item_fp.is_empty() || kept_fp.is_empty()
        {
            return true;
        }

        let item_set: std::collections::HashSet<u64> = item_fp
            .into_iter()
            .collect();
        let kept_set: std::collections::HashSet<u64> = kept_fp
            .into_iter()
            .collect();

        let inter = item_set
            .intersection(&kept_set)
            .count();
        let union = item_set
            .union(&kept_set)
            .count();

        if union == 0
        {
            return true; // still no signal → allow detailed
        }

        let quick_j = inter as f64 / union as f64;
        quick_j
            >= (self
                .config
                .jaccard_threshold
                * 0.5)
    }

    /// Try primary mode (word or char), then char fallback with a slightly
    /// lower threshold to handle identifier/punctuation edits
    fn near_duplicate_hashed(
        &self,
        a: &str,
        b: &str,
    ) -> bool
    {
        let n = self
            .config
            .ngram_size;

        // Primary mode
        let ah = hashed_shingles_mode(
            a,
            n,
            self.config
                .ngram_mode,
        );
        let bh = hashed_shingles_mode(
            b,
            n,
            self.config
                .ngram_mode,
        );

        if self.should_compare_detailed(&ah, &bh)
        {
            let j = jaccard_u64(&ah, &bh);
            if j >= self
                .config
                .jaccard_threshold
            {
                return true;
            }
        }

        // char fallback ONLY if enabled and primary mode is Word
        if self
            .config
            .char_fallback
            && self
                .config
                .ngram_mode
                == NgramMode::Word
        {
            let ac = hashed_char_ngrams(a, n);
            let bc = hashed_char_ngrams(b, n);

            if self.should_compare_detailed(&ac, &bc)
            {
                // Slightly more permissive than primary threshold
                let fallback = (self
                    .config
                    .jaccard_threshold
                    * 0.9)
                    .min(0.55);
                let j = jaccard_u64(&ac, &bc);
                if j >= fallback
                {
                    return true;
                }
            }
        }

        // Optional: SimHash for long spans (kept cheap)
        if ah.len() > 2000 && bh.len() > 2000
        {
            let sa = simhash_64(64, &ah);
            let sb = simhash_64(64, &bh);
            if simhash_close(sa, sb, 4)
            {
                return true;
            }
        }

        false
    }

    /// Normalize content for comparison (remove comments, whitespace variations)
    fn normalize_content(
        &self,
        content: &str,
    ) -> String
    {
        content
            .lines()
            .map(|line| {
                // Remove line comments
                let line = if let Some(pos) = line.find("//")
                {
                    &line[..pos]
                }
                else
                {
                    line
                };
                // Normalize whitespace
                line.trim()
                    .replace('\t', " ")
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Compute rolling hash for exact duplicate detection
    pub fn compute_rolling_hash(
        &self,
        content: &str,
    ) -> u64
    {
        let mut hasher = Xxh64::new(0);
        hasher.update(
            self.normalize_content(content)
                .as_bytes(),
        );
        hasher.digest()
    }
}

impl Default for DedupeEngine
{
    fn default() -> Self
    {
        Self::new()
    }
}

/// Bucket configuration for hard caps (A3 requirement)
#[derive(Debug, Clone)]
pub struct BucketCaps
{
    pub code: usize,
    pub interfaces: usize,
    pub tests: usize,
}

/// Refusal log entry for items that couldn't be fitted
#[derive(Debug, Clone)]
pub struct Refusal
{
    pub id: String,
    pub reason: String, // e.g., "bucket-cap", "novelty-floor"
    pub bucket: String, // "code" | "interfaces" | "tests"
}

/// Result of bucketed fitting with refusal logs
#[derive(Debug, Clone)]
pub struct BucketFit
{
    pub fitted: FitResult,
    pub refusals: Vec<Refusal>,
}

/// SpanTag for AST-aware item classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpanTag
{
    Interface, // fn/struct/trait signature blocks
    Test,      // *_test or test module files
    Doc,       // rustdoc/comment-only spans
    Impl,      // impl blocks
    Code,      // general code spans
}

/// Enhanced Item with optional tags for bucket classification
#[derive(Debug, Clone)]
pub struct TaggedItem
{
    pub id: String,
    pub content: String,
    pub priority: Priority,
    pub hard: bool,
    pub min_tokens: usize,
    pub tags: HashSet<SpanTag>,
}

impl From<Item> for TaggedItem
{
    fn from(item: Item) -> Self
    {
        Self {
            id: item.id,
            content: item.content,
            priority: item.priority,
            hard: item.hard,
            min_tokens: item.min_tokens,
            tags: HashSet::new(), // Default to no tags
        }
    }
}

impl From<TaggedItem> for Item
{
    fn from(tagged: TaggedItem) -> Self
    {
        Self {
            id: tagged.id,
            content: tagged.content,
            priority: tagged.priority,
            hard: tagged.hard,
            min_tokens: tagged.min_tokens,
        }
    }
}

/// Fits items under per-bucket hard caps and produces refusal logs.
/// Ensures total within ±5% of requested caps and preserves global
/// deterministic order via per-bucket stable sorts.
pub fn fit_with_buckets(
    budgeter: &Budgeter,
    items: Vec<TaggedItem>,
    caps: BucketCaps,
    novelty_min: Option<f64>,
) -> Result<BucketFit>
{
    let mut refusals = Vec::new();

    // 1) Partition deterministically using tags
    let (mut code_items, mut interface_items, mut test_items) = partition_by_tags(items);

    // 2) Apply novelty filter before fit if specified
    if let Some(threshold) = novelty_min
    {
        // Build TF-IDF index from all content for novelty scoring
        let all_documents: Vec<String> = code_items
            .iter()
            .chain(interface_items.iter())
            .chain(test_items.iter())
            .map(|item| {
                item.content
                    .clone()
            })
            .collect();

        let tfidf = TfidfIndex::new(&all_documents);

        // Apply novelty filtering to each bucket
        let (code_filtered, code_refusals) = filter_by_novelty(&tfidf, code_items, threshold);
        let (interface_filtered, interface_refusals) =
            filter_by_novelty(&tfidf, interface_items, threshold);
        let (test_filtered, test_refusals) = filter_by_novelty(&tfidf, test_items, threshold);

        code_items = code_filtered;
        interface_items = interface_filtered;
        test_items = test_filtered;

        refusals.extend(code_refusals);
        refusals.extend(interface_refusals);
        refusals.extend(test_refusals);
    }

    // 3) Call `fit` separately with each cap and track refusals
    let code_items_orig = code_items.clone();
    let code_fit = budgeter.fit(
        code_items
            .into_iter()
            .map(Into::into)
            .collect(),
        caps.code,
    )?;

    let interface_items_orig = interface_items.clone();
    let interface_fit = budgeter.fit(
        interface_items
            .into_iter()
            .map(Into::into)
            .collect(),
        caps.interfaces,
    )?;

    let test_items_orig = test_items.clone();
    let test_fit = budgeter.fit(
        test_items
            .into_iter()
            .map(Into::into)
            .collect(),
        caps.tests,
    )?;

    // Track items that didn't make it into each bucket
    let fitted_code_ids: std::collections::HashSet<_> = code_fit
        .items
        .iter()
        .map(|item| &item.id)
        .collect();
    for item in &code_items_orig
    {
        if !fitted_code_ids.contains(&item.id)
        {
            refusals.push(Refusal {
                id: item
                    .id
                    .clone(),
                reason: "bucket-cap-exceeded".to_string(),
                bucket: "code".to_string(),
            });
        }
    }

    let fitted_interface_ids: std::collections::HashSet<_> = interface_fit
        .items
        .iter()
        .map(|item| &item.id)
        .collect();
    for item in &interface_items_orig
    {
        if !fitted_interface_ids.contains(&item.id)
        {
            refusals.push(Refusal {
                id: item
                    .id
                    .clone(),
                reason: "bucket-cap-exceeded".to_string(),
                bucket: "interfaces".to_string(),
            });
        }
    }

    let fitted_test_ids: std::collections::HashSet<_> = test_fit
        .items
        .iter()
        .map(|item| &item.id)
        .collect();
    for item in &test_items_orig
    {
        if !fitted_test_ids.contains(&item.id)
        {
            refusals.push(Refusal {
                id: item
                    .id
                    .clone(),
                reason: "bucket-cap-exceeded".to_string(),
                bucket: "tests".to_string(),
            });
        }
    }

    // 4) Apply bucket-local trimming before merge
    let mut code_items = code_fit.items;
    let mut interface_items = interface_fit.items;
    let mut test_items = test_fit.items;

    // Helper that trims the tail of a bucket deterministically
    fn trim_bucket_tail(
        items: &mut Vec<FittedItem>,
        target: usize,
    )
    {
        // Stable policy: Sort by tokens desc then id desc,
        // so pops remove least valuable first
        items.sort_by(|a, b| {
            b.tokens
                .cmp(&a.tokens)
                .then(b.id.cmp(&a.id))
        });
        // Remove until within target
        let mut total = items
            .iter()
            .map(|x| x.tokens)
            .sum::<usize>();
        while total > target
        {
            if let Some(last) = items.pop()
            {
                total = total.saturating_sub(last.tokens);
            }
            else
            {
                break;
            }
        }
    }

    // Enforce per-bucket caps strictly before merge
    let code_total = code_items
        .iter()
        .map(|x| x.tokens)
        .sum::<usize>();
    let interface_total = interface_items
        .iter()
        .map(|x| x.tokens)
        .sum::<usize>();
    let test_total = test_items
        .iter()
        .map(|x| x.tokens)
        .sum::<usize>();

    if code_total > caps.code
    {
        trim_bucket_tail(&mut code_items, caps.code);
    }
    if interface_total > caps.interfaces
    {
        trim_bucket_tail(&mut interface_items, caps.interfaces);
    }
    if test_total > caps.tests
    {
        trim_bucket_tail(&mut test_items, caps.tests);
    }

    // Merge after bucket-local trims
    let mut all_items = Vec::new();
    all_items.extend(code_items);
    all_items.extend(interface_items);
    all_items.extend(test_items);

    let total_tokens = all_items
        .iter()
        .map(|item| item.tokens)
        .sum::<usize>();
    let expected_total = caps.code + caps.interfaces + caps.tests;

    // 5) Validate ±5% compliance
    let tolerance = (expected_total as f64 * 0.05) as usize;
    if total_tokens > expected_total + tolerance
    {
        // Trim lowest-priority tail following stable rule
        all_items.sort_by(|a, b| {
            b.id.cmp(&a.id) // Reverse lexicographic as tie-breaker for now
        });

        let mut current_total = total_tokens;
        while current_total > expected_total + tolerance && !all_items.is_empty()
        {
            if let Some(removed) = all_items.pop()
            {
                current_total = current_total.saturating_sub(removed.tokens);
                refusals.push(Refusal {
                    id: removed.id,
                    reason: "budget-overflow".to_string(),
                    bucket: "mixed".to_string(),
                });
            }
        }
    }

    let total_tokens = all_items
        .iter()
        .map(|item| item.tokens)
        .sum();
    let fitted = FitResult { items: all_items, total_tokens };

    Ok(BucketFit { fitted, refusals })
}

/// Partition items by their tags into code/interface/test buckets
fn partition_by_tags(items: Vec<TaggedItem>)
-> (Vec<TaggedItem>, Vec<TaggedItem>, Vec<TaggedItem>)
{
    let mut code_items = Vec::new();
    let mut interface_items = Vec::new();
    let mut test_items = Vec::new();

    for item in items
    {
        if item
            .tags
            .contains(&SpanTag::Test)
        {
            test_items.push(item);
        }
        else if item
            .tags
            .contains(&SpanTag::Interface)
        {
            interface_items.push(item);
        }
        else
        {
            code_items.push(item);
        }
    }

    (code_items, interface_items, test_items)
}

/// Parse bucket specification string like "code=60,interfaces=20,tests=20"
pub fn parse_bucket_caps(spec: &str) -> Result<BucketCaps>
{
    let mut code = 0;
    let mut interfaces = 0;
    let mut tests = 0;

    for part in spec.split(',')
    {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=')
        {
            let key = key.trim();
            let value = value
                .trim()
                .parse::<usize>()
                .with_context(|| format!("Invalid bucket cap value: {}", value))?;

            match key
            {
                "code" => code = value,
                "interfaces" => interfaces = value,
                "tests" => tests = value,
                _ => return Err(anyhow!("Unknown bucket type: {}", key)),
            }
        }
        else
        {
            return Err(anyhow!("Invalid bucket specification format: {}", part));
        }
    }

    Ok(BucketCaps { code, interfaces, tests })
}

/// TF-IDF index for computing novelty scores (A4 requirement)
#[derive(Debug, Clone)]
pub struct TfidfIndex
{
    /// Document frequency for each term (how many docs contain this term)
    doc_freq: HashMap<String, usize>,
    /// Total number of documents in the corpus
    total_docs: usize,
    /// Maximum IDF value for normalization
    max_idf: f64,
}

impl TfidfIndex
{
    /// Create a new TF-IDF index from document corpus
    pub fn new(documents: &[String]) -> Self
    {
        let mut doc_freq = HashMap::new();
        let total_docs = documents.len();

        // Count document frequency for each term
        for doc in documents
        {
            let tokens = tokenize_repo_style(doc);
            let unique_tokens: HashSet<String> = tokens
                .into_iter()
                .collect();

            for token in unique_tokens
            {
                *doc_freq
                    .entry(token)
                    .or_insert(0) += 1;
            }
        }

        // Calculate maximum IDF for normalization
        let min_df = doc_freq
            .values()
            .min()
            .copied()
            .unwrap_or(1);
        let max_idf = if min_df > 0
        {
            (total_docs as f64 / min_df as f64).ln()
        }
        else
        {
            0.0
        };

        Self { doc_freq, total_docs, max_idf }
    }

    /// Get IDF score for a term
    pub fn idf(
        &self,
        term: &str,
    ) -> Option<f64>
    {
        self.doc_freq
            .get(term)
            .map(|&df| {
                if df > 0
                {
                    (self.total_docs as f64 / df as f64).ln()
                }
                else
                {
                    0.0
                }
            })
    }

    /// Normalize IDF score to [0,1] range
    pub fn normalize(
        &self,
        idf_score: f64,
    ) -> f64
    {
        if self.max_idf > 0.0
        {
            (idf_score / self.max_idf).clamp(0.0, 1.0)
        }
        else
        {
            0.0
        }
    }
}

// Common code-ish tokens that shouldn't contribute to novelty.
// Keep this tiny and language-agnostic; expand only if needed.
const CODE_STOPWORDS: &[&str] = &[
    "fn",
    "function",
    "struct",
    "impl",
    "implementation",
    "module",
    "class",
    "pub",
    "public",
    "trait",
    "interface",
    "method",
    "test",
    "assert",
    "std",
    "fmt",
    "debug",
    "display",
    "derive",
    "clone",
    "copy",
    "eq",
];

/// Tokenize text using repository-style conventions:
/// - split on any non-alphanumeric (except '_')
/// - lowercase
/// - drop very short tokens
/// - drop common boilerplate "stopwords"
fn tokenize_repo_style(text: &str) -> Vec<String>
{
    let mut toks: Vec<String> = Vec::new();
    let mut cur = String::new();

    for ch in text.chars()
    {
        if ch.is_alphanumeric() || ch == '_'
        {
            cur.push(ch.to_ascii_lowercase());
        }
        else if !cur.is_empty()
        {
            if cur.len() >= 2 && !CODE_STOPWORDS.contains(&cur.as_str())
            {
                toks.push(std::mem::take(&mut cur));
            }
            else
            {
                cur.clear();
            }
        }
    }
    if !cur.is_empty() && cur.len() >= 2 && !CODE_STOPWORDS.contains(&cur.as_str())
    {
        toks.push(cur);
    }

    toks
}

/// Computes a crude novelty score in [0,1] from TF-IDF features.
/// You can refine by using top-k rare tokens or entropy per span.
pub fn novelty_score(
    tfidf: &TfidfIndex,
    text: &str,
) -> f64
{
    // Example: mean IDF of top-k tokens, normalized to [0,1].
    let tokens = tokenize_repo_style(text);
    let mut idfs: Vec<f64> = tokens
        .iter()
        .filter_map(|t| tfidf.idf(t))
        .collect();

    if idfs.is_empty()
    {
        return 0.0;
    }

    idfs.sort_by(|a, b| {
        b.partial_cmp(a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let k = idfs
        .len()
        .min(16);
    let mean_topk = idfs[..k]
        .iter()
        .sum::<f64>()
        / k as f64;

    // Normalize against global IDF range if you track it.
    tfidf.normalize(mean_topk)
}

/// Filters items below `--novelty-min` with rationale logging.
pub fn filter_by_novelty(
    tfidf: &TfidfIndex,
    items: Vec<TaggedItem>,
    min_score: f64,
) -> (Vec<TaggedItem>, Vec<Refusal>)
{
    let mut kept = Vec::new();
    let mut refusals = Vec::new();

    for item in items
    {
        let score = novelty_score(tfidf, &item.content);
        if score >= min_score
        {
            kept.push(item);
        }
        else
        {
            let bucket = if item
                .tags
                .contains(&SpanTag::Test)
            {
                "tests"
            }
            else if item
                .tags
                .contains(&SpanTag::Interface)
            {
                "interfaces"
            }
            else
            {
                "code"
            };

            refusals.push(Refusal {
                id: item
                    .id
                    .clone(),
                reason: format!("novelty-floor<{:.2}", min_score),
                bucket: bucket.to_string(),
            });
        }
    }

    (kept, refusals)
}
